use bandastation_atmos::{
    catalog::*,
    cell::{Cell, WorldAction},
    network::Tile,
    phase::{Error, Store, Write},
    Context, Effect, Holder, Mixture,
};

fn register(
    store: &mut Store,
    cell: Cell,
    coordinates: [i32; 3],
    thermal: bool,
) -> Result<u32, Error> {
    let tile = Tile::new(coordinates, &cell.air, thermal);
    store.register(cell, tile)
}

fn air(gas: usize, amount: f32) -> Mixture {
    let mut air = Mixture::new(293.15, CELL_VOLUME, true);
    air.set(gas, amount);
    air
}
fn cell() -> Cell {
    Cell::new(
        air(NITROGEN, 10.0),
        Context {
            holder: Holder::OpenTurf,
            ..Context::default()
        },
    )
}

#[test]
fn debug_tracker_cannot_change_a_running_snapshot() {
    let mut store = Store::default();
    register(&mut store, cell(), [1, 1, 1], false).unwrap();
    assert!(store.snapshot().schedule.max_shares.is_none());
    store.track_max_share(true).unwrap();
    let snapshot = store.snapshot();
    let job = store.begin().unwrap();
    assert_eq!(store.track_max_share(false), Err(Error::Busy));
    store.finish(job.calculate()).unwrap();
    assert!(snapshot.schedule.max_shares.as_ref().unwrap().is_empty());
    assert!(store
        .snapshot()
        .schedule
        .max_shares
        .as_ref()
        .unwrap()
        .contains_key(&1));
    store.track_max_share(false).unwrap();
    assert!(store.snapshot().schedule.max_shares.is_none());
}

#[test]
fn safe_commit_preserves_results_and_its_explicit_archive() {
    let mut store = Store::default();
    let id = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let mut working = air(OXYGEN, 7.0);
    working.results[0] = Some(3.0);
    let mut archived = air(NITROGEN, 2.0).archive();
    archived.temperature = 150.0;
    store
        .write(Write::Commit(id, working, archived, true))
        .unwrap();
    let snapshot = store.snapshot();
    assert_eq!(snapshot.cells[&id].air.results[0], Some(3.0));
    assert_eq!(snapshot.schedule.tiles[&id].archived.moles[NITROGEN], 2.0);
    assert_eq!(snapshot.schedule.tiles[&id].archived.temperature, 150.0);
    assert_eq!(snapshot.cells[&id].air.volume, CELL_VOLUME);
}

#[test]
fn queued_ignition_survives_publication_and_external_deletion_clears_it() {
    let mut store = Store::default();
    let mut fuel = cell();
    fuel.air = air(PLASMA, 10.0);
    fuel.air.set(OXYGEN, 100.0);
    let id = register(&mut store, fuel, [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store
        .write(Write::Expose(id, 1200.0, CELL_VOLUME, false))
        .unwrap();
    assert!(store.snapshot().cells[&id].hotspot.is_none());
    store.finish(job.calculate()).unwrap();
    assert!(store.snapshot().cells[&id].hotspot.is_some());
    assert_eq!(store.snapshot().schedule.hotspots, [id]);
    assert!(store
        .take_commands()
        .unwrap()
        .iter()
        .any(|c| matches!(c.effect.action, WorldAction::FireExposure { .. })));
    store.write(Write::ClearHotspot(id)).unwrap();
    assert!(store.snapshot().cells[&id].hotspot.is_none());
    assert!(store.snapshot().schedule.hotspots.is_empty());
}

#[test]
fn external_conduction_admission_uses_dms_already_checked_live_values() {
    let mut store = Store::default();
    let id = register(&mut store, cell(), [1, 1, 1], true).unwrap();
    // DM can have changed the live temperature before its normal consider_superconductivity guard.
    // Its numeric packet is imported later; admission must not recheck the stale native frame.
    store.write(Write::Control(id, 4)).unwrap();
    assert_eq!(store.snapshot().schedule.conducting, [id]);
}

#[test]
fn reads_stay_stable_and_a_write_during_work_survives_publication() {
    let mut store = Store::default();
    let id = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let before = store.snapshot();
    let job = store.begin().unwrap();
    store.write(Write::Release(id, air(OXYGEN, 3.0))).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[OXYGEN], 0.0);
    let completed = std::thread::spawn(move || job.calculate()).join().unwrap();
    store.finish(completed).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[OXYGEN], 3.0);
    assert_eq!(before.cells[&id].air.moles[OXYGEN], 0.0);
}

#[test]
fn queued_replacements_and_releases_keep_their_order() {
    let mut store = Store::default();
    let id = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.write(Write::Release(id, air(OXYGEN, 5.0))).unwrap();
    store
        .write(Write::Replace(id, air(CARBON_DIOXIDE, 7.0)))
        .unwrap();
    store.write(Write::Release(id, air(OXYGEN, 2.0))).unwrap();
    store.finish(job.calculate()).unwrap();
    let snapshot = store.snapshot();
    assert_eq!(snapshot.cells[&id].air.moles[OXYGEN], 2.0);
    assert_eq!(snapshot.cells[&id].air.moles[CARBON_DIOXIDE], 7.0);
    assert_eq!(snapshot.cells[&id].air.moles[NITROGEN], 0.0);
}

#[test]
fn removed_cells_drop_their_effects_and_cannot_be_resurrected() {
    let mut store = Store::default();
    let mut frozen = cell();
    frozen.air = air(WATER_VAPOR, 1.0);
    frozen.air.temperature = 190.0;
    let id = register(&mut store, frozen, [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.write(Write::Remove(id)).unwrap();
    store.write(Write::Release(id, air(OXYGEN, 2.0))).unwrap();
    store.finish(job.calculate()).unwrap();
    assert!(!store.snapshot().cells.contains_key(&id));
    assert!(store.take_commands().unwrap().is_empty());
    assert_ne!(register(&mut store, cell(), [1, 1, 1], false).unwrap(), id);
    assert_eq!(
        store.write(Write::Release(id, air(OXYGEN, 2.0))),
        Err(Error::MissingCell)
    );
}

#[test]
fn effects_are_delivered_once_before_another_calculation() {
    let mut store = Store::default();
    let mut frozen = cell();
    frozen.air = air(WATER_VAPOR, 1.0);
    frozen.air.temperature = 190.0;
    register(&mut store, frozen, [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    assert!(matches!(store.begin(), Err(Error::Busy)));
    store.finish(job.calculate()).unwrap();
    assert!(matches!(store.begin(), Err(Error::EffectsPending)));
    let commands = store.take_commands().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(matches!(
        commands[0].effect.action,
        WorldAction::Reaction(Effect::Freeze)
    ));
    assert!(matches!(
        commands[1].effect.action,
        WorldAction::TurfExposure { .. }
    ));
    assert!(store.take_commands().unwrap().is_empty());
    assert!(store.begin().is_ok());
}

#[test]
fn closing_during_work_rejects_the_late_result() {
    let mut store = Store::default();
    register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.close();
    assert_eq!(store.finish(job.calculate()), Err(Error::Closed));
    assert!(store.snapshot().cells.is_empty());
}

#[test]
fn externally_added_reactant_participates_only_in_the_next_calculation() {
    let mut store = Store::default();
    let mut fuel = cell();
    fuel.air = air(PLASMA, 1.0);
    fuel.air.temperature = 700.0;
    let id = register(&mut store, fuel, [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.write(Write::Release(id, air(OXYGEN, 10.0))).unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[PLASMA], 1.0);
    assert_eq!(store.snapshot().cells[&id].air.moles[OXYGEN], 10.0);
    assert!(store
        .take_commands()
        .unwrap()
        .iter()
        .all(|c| matches!(c.effect.action, WorldAction::TurfExposure { .. })));
    let next = store.begin().unwrap();
    store.finish(next.calculate()).unwrap();
    assert!(store.snapshot().cells[&id].air.moles[PLASMA] < 1.0);
}

#[test]
fn cancellation_preserves_external_writes_but_rejects_the_old_worker_result() {
    let mut store = Store::default();
    let id = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.write(Write::Release(id, air(OXYGEN, 3.0))).unwrap();
    store.abort();
    assert_eq!(store.snapshot().cells[&id].air.moles[OXYGEN], 3.0);
    assert_eq!(store.finish(job.calculate()), Err(Error::StaleJob));
    assert_eq!(store.snapshot().cells[&id].air.moles[OXYGEN], 3.0);
}

#[test]
fn connected_cells_share_in_the_worker_and_external_gas_wakes_a_sleeping_region() {
    let mut store = Store::default();
    let a = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let b = register(&mut store, cell(), [2, 1, 1], false).unwrap();
    store.write(Write::Neighbors(a, vec![b])).unwrap();
    assert!(matches!(store.begin(), Err(Error::InvalidTopology)));
    store.write(Write::Neighbors(b, vec![a])).unwrap();
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    assert!(store.snapshot().schedule.active.is_empty());
    store.take_commands().unwrap();
    store.write(Write::Release(a, air(NITROGEN, 20.0))).unwrap();
    assert_eq!(store.snapshot().schedule.active, [a]);
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().cells[&a].air.moles[NITROGEN], 20.0);
    assert_eq!(store.snapshot().cells[&b].air.moles[NITROGEN], 20.0);
}

#[test]
fn removal_during_work_clears_links_and_queued_writes_cannot_restore_them() {
    let mut store = Store::default();
    let a = register(&mut store, cell(), [1, 1, 1], false).unwrap();
    let b = register(&mut store, cell(), [2, 1, 1], false).unwrap();
    store.write(Write::Neighbors(a, vec![b])).unwrap();
    store.write(Write::Neighbors(b, vec![a])).unwrap();
    let job = store.begin().unwrap();
    store.write(Write::Remove(b)).unwrap();
    store.write(Write::Neighbors(a, vec![b])).unwrap();
    store.finish(job.calculate()).unwrap();
    assert!(!store.snapshot().cells.contains_key(&b));
    assert!(store.snapshot().schedule.tiles[&a].neighbors.is_empty());
    assert!(store.take_commands().unwrap().iter().all(|c| c.cell == a));
    assert!(store.begin().is_ok());
}

#[test]
fn world_commands_follow_reverse_active_order() {
    let mut store = Store::default();
    let mut cold = cell();
    cold.air = air(WATER_VAPOR, 1.0);
    cold.air.temperature = 190.0;
    let a = register(&mut store, cold.clone(), [1, 1, 1], false).unwrap();
    let b = register(&mut store, cold, [3, 1, 1], false).unwrap();
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(
        store
            .take_commands()
            .unwrap()
            .iter()
            .map(|c| c.cell)
            .collect::<Vec<_>>(),
        [b, b, a, a]
    );
}

#[test]
fn copying_gas_preserves_the_receivers_volume_class_and_reaction_results() {
    let mut store = Store::default();
    let mut original = cell();
    original.air.results[2] = Some(7.0);
    let id = register(&mut store, original, [1, 1, 1], false).unwrap();
    let mut incoming = Mixture::new(500.0, 100.0, false);
    incoming.set(OXYGEN, 5.0);
    incoming.results[2] = Some(99.0);
    store.write(Write::Replace(id, incoming)).unwrap();
    let state = store.snapshot();
    assert_eq!(state.cells[&id].air.volume, CELL_VOLUME);
    assert!(state.cells[&id].air.turf_heat_capacity);
    assert_eq!(state.cells[&id].air.results[2], Some(7.0));
    assert_eq!(state.cells[&id].air.moles[OXYGEN], 5.0);
    assert_eq!(state.schedule.tiles[&id].archived.moles[OXYGEN], 0.0);
}

#[test]
fn external_gas_cannot_change_space_but_assume_air_still_wakes_it() {
    use bandastation_atmos::transport::Boundary;
    let mut store = Store::default();
    let space = Cell::new(Mixture::new(TCMB, CELL_VOLUME, false), Context::default());
    let mut tile = Tile::new([1, 1, 1], &space.air, false);
    tile.boundary = Boundary::Space;
    tile.run_later = true;
    let id = store.register(space, tile).unwrap();
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    assert!(store.snapshot().schedule.active.is_empty());
    store.write(Write::Release(id, air(OXYGEN, 100.0))).unwrap();
    store
        .write(Write::Replace(id, air(NITROGEN, 500.0)))
        .unwrap();
    let state = store.snapshot();
    assert!(state.cells[&id].air.keys().is_empty());
    assert_eq!(state.cells[&id].air.temperature, TCMB);
    assert_eq!(state.schedule.active, [id]);
}

#[test]
fn replacing_the_mixture_changes_its_class_volume_and_archive_without_replacing_the_tile() {
    use bandastation_atmos::transport::Boundary;
    let mut store = Store::default();
    let mut tile = Tile::new([1, 1, 1], &cell().air, false);
    tile.boundary = Boundary::Space;
    tile.run_later = true;
    tile.ticker = 7;
    let id = store.register(cell(), tile).unwrap();
    let mut incoming = Mixture::new(500.0, 100.0, false);
    incoming.set(OXYGEN, 5.0);
    incoming.results[2] = Some(99.0);
    let mut archived = incoming.archive();
    archived.temperature = 123.0;
    store
        .write(Write::ReplaceFull(id, incoming, archived, false))
        .unwrap();
    let state = store.snapshot();
    assert_eq!(state.cells[&id].air.volume, 100.0);
    assert!(!state.cells[&id].air.turf_heat_capacity);
    assert_eq!(state.cells[&id].air.results[2], Some(99.0));
    assert_eq!(state.schedule.tiles[&id].archived.temperature, 123.0);
    assert_eq!(state.schedule.tiles[&id].archived.moles[OXYGEN], 5.0);
    assert!(matches!(
        state.schedule.tiles[&id].boundary,
        Boundary::Mutable
    ));
    assert!(state.schedule.tiles[&id].run_later);
    assert_eq!(state.schedule.tiles[&id].ticker, 7);
    assert_eq!(state.schedule.active, [id]);
    assert_eq!(store.binding_version(id).unwrap(), 1);
}

#[test]
fn blocked_gas_is_preserved_archived_and_activated_independently_of_solid_state() {
    let mut store = Store::default();
    let original = cell();
    let mut tile = Tile::new([1, 1, 1], &original.air, false);
    tile.terrain.closed = true;
    let id = store.register(original, tile).unwrap();
    store.set_processing(vec![id], vec![]).unwrap();
    let completed = store.begin().unwrap().calculate();
    store.finish(completed).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[NITROGEN], 10.0);
    assert_eq!(
        store.snapshot().schedule.tiles[&id].archived.moles[NITROGEN],
        10.0
    );
    let mut terrain = store.snapshot().schedule.tiles[&id].terrain.clone();
    terrain.closed = false;
    store.write(Write::Terrain(id, terrain)).unwrap();
    store.write(Write::Control(id, 0)).unwrap();
    assert!(store.snapshot().schedule.active.contains(&id));
    assert_eq!(store.snapshot().cells[&id].air.moles[NITROGEN], 10.0);
    assert_eq!(store.binding_version(id).unwrap(), 1);
}
