use bandastation_atmos::{
    catalog::*,
    cell::{Cell, WorldAction},
    network::Tile,
    phase::{Error, Rebinding, Store, Write},
    Context, Effect, Holder, Mixture,
};

fn cell(gas: usize, amount: f32, temperature: f32) -> Cell {
    let mut air = Mixture::new(temperature, CELL_VOLUME, true);
    air.set(gas, amount);
    Cell::new(
        air,
        Context {
            holder: Holder::OpenTurf,
            ..Context::default()
        },
    )
}
fn register(store: &mut Store, x: i32, c: Cell) -> u32 {
    let tile = Tile::new([x, 1, 1], &c.air, false);
    store.register(c, tile).unwrap()
}
fn replacement(x: i32, inherit: bool) -> Rebinding {
    let c = cell(NITROGEN, 99.0, 293.15);
    let mut tile = Tile::new([x, 1, 1], &c.air, true);
    tile.terrain.capacity = 100000.0;
    tile.terrain.conductivity = 0.1;
    Rebinding {
        cell: c,
        tile,
        inherit_group: inherit,
        inherit_gas: inherit,
    }
}
fn connected() -> (Store, u32, u32) {
    let mut store = Store::default();
    let a = register(&mut store, 1, cell(NITROGEN, 100.0, 293.15));
    let b = register(&mut store, 2, cell(OXYGEN, 100.0, 293.15));
    store.write(Write::Neighbors(a, vec![b])).unwrap();
    store.write(Write::Neighbors(b, vec![a])).unwrap();
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    store.take_commands().unwrap();
    (store, a, b)
}

#[test]
fn inherited_floor_keeps_coordinate_identity_group_order_and_cooldowns() {
    let (mut store, a, b) = connected();
    let before = store.snapshot();
    let group = before.schedule.tiles[&a].group.unwrap();
    let version = store.invalidate_binding(a).unwrap();
    assert!(matches!(store.begin(), Err(Error::DetachedBinding)));
    store.rebind(a, version, replacement(1, true)).unwrap();
    let after = store.snapshot();
    assert_eq!(after.cells.len(), 2);
    assert_eq!(store.binding_version(a), Ok(2));
    assert_eq!(after.schedule.tiles[&a].coordinates, [1, 1, 1]);
    assert_eq!(after.schedule.tiles[&a].group, Some(group));
    assert_eq!(after.schedule.tiles[&b].group, Some(group));
    assert_eq!(
        after.schedule.groups[&group].members,
        before.schedule.groups[&group].members
    );
    assert_eq!(
        after.schedule.groups[&group].breakdown,
        before.schedule.groups[&group].breakdown
    );
    assert_eq!(
        after.schedule.groups[&group].dismantle,
        before.schedule.groups[&group].dismantle
    );
    assert_eq!(after.schedule.active, before.schedule.active);
    assert_eq!(after.cells[&a].air.moles, before.cells[&a].air.moles);
    assert_eq!(
        after.cells[&a].air.temperature,
        before.cells[&a].air.temperature
    );
    assert_eq!(after.schedule.tiles[&a].neighbors, [b]);
    assert_eq!(after.schedule.tiles[&b].neighbors, [a]);
    assert_eq!(after.schedule.tiles[&a].terrain.capacity, 100000.0);
    assert_ne!(
        after.schedule.tiles[&a].terrain.capacity,
        before.schedule.tiles[&a].terrain.capacity
    );
    assert!(after.schedule.tiles[&a]
        .archived
        .moles
        .iter()
        .all(|m| *m == 0.0));
    assert_eq!(
        after.schedule.tiles[&a].archived.keys(),
        after.cells[&a].air.keys()
    );
    assert!(after.cells[&a].air.results.iter().all(Option::is_none));
}

#[test]
fn releases_on_both_sides_of_rebinding_survive_an_in_flight_calculation() {
    let mut store = Store::default();
    let id = register(&mut store, 1, cell(NITROGEN, 10.0, 293.15));
    let job = store.begin().unwrap();
    store
        .write(Write::Release(id, cell(NITROGEN, 3.0, 293.15).air))
        .unwrap();
    let version = store.invalidate_binding(id).unwrap();
    store.rebind(id, version, replacement(1, true)).unwrap();
    store
        .write(Write::Release(id, cell(NITROGEN, 5.0, 293.15).air))
        .unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[NITROGEN], 18.0);
    assert!(store.take_commands().unwrap().is_empty());
}

#[test]
fn old_worker_effects_are_rejected_but_inherited_physics_is_published() {
    let mut store = Store::default();
    let id = register(&mut store, 1, cell(WATER_VAPOR, 1.0, 190.0));
    let job = store.begin().unwrap();
    let version = store.invalidate_binding(id).unwrap();
    store.rebind(id, version, replacement(1, true)).unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[WATER_VAPOR], 0.75);
    assert!(store.take_commands().unwrap().is_empty());
    store.write(Write::Control(id, 0)).unwrap();
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    let commands = store.take_commands().unwrap();
    assert!(commands
        .iter()
        .any(|c| matches!(c.effect.action, WorldAction::Reaction(Effect::Freeze))));
    assert!(commands.iter().all(|c| c.binding_version == version));
}

#[test]
fn effects_already_fetched_by_dm_can_be_checked_against_the_new_binding() {
    let mut store = Store::default();
    let id = register(&mut store, 1, cell(WATER_VAPOR, 1.0, 190.0));
    let job = store.begin().unwrap();
    store.finish(job.calculate()).unwrap();
    let fetched = store.take_commands().unwrap();
    let version = store.invalidate_binding(id).unwrap();
    store.rebind(id, version, replacement(1, true)).unwrap();
    assert!(fetched
        .iter()
        .all(|command| command.binding_version != store.binding_version(id).unwrap()));
}

#[test]
fn noninherited_floor_replaces_gas_and_clears_the_old_group() {
    let (mut store, a, b) = connected();
    let version = store.invalidate_binding(a).unwrap();
    store.rebind(a, version, replacement(1, false)).unwrap();
    let after = store.snapshot();
    assert_eq!(after.cells[&a].air.moles[NITROGEN], 99.0);
    assert_eq!(after.cells[&a].air.moles[OXYGEN], 0.0);
    assert!(after.schedule.groups.is_empty());
    assert!(after.schedule.tiles[&a].group.is_none());
    assert!(after.schedule.tiles[&b].group.is_none());
    assert_eq!(store.binding_version(a), Ok(2));
}

#[test]
fn outdated_rebind_cannot_override_a_second_replacement() {
    let mut store = Store::default();
    let id = register(&mut store, 1, cell(NITROGEN, 10.0, 293.15));
    let older = store.invalidate_binding(id).unwrap();
    let current = store.invalidate_binding(id).unwrap();
    assert_eq!(
        store.rebind(id, older, replacement(1, false)),
        Err(Error::StaleBinding)
    );
    assert!(matches!(store.begin(), Err(Error::DetachedBinding)));
    store.rebind(id, current, replacement(1, true)).unwrap();
    assert_eq!(store.snapshot().cells[&id].air.moles[NITROGEN], 10.0);
}
