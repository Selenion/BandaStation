use super::*;
use crate::{catalog::*, cell::Hotspot, Holder};

#[test]
fn dm_archive_metadata_keeps_current_terrain_and_gas_until_the_safe_commit() {
    let mut store = Store::default();
    let mut air = Mixture::new(333.0, CELL_VOLUME, true);
    air.set(NITROGEN, 17.0);
    let mut tile = Tile::new([1, 1, 1], &air, false);
    tile.terrain.temperature = 350.0;
    let archived = tile.archived.clone();
    let id = store
        .register(Cell::new(air, Context::default()), tile)
        .unwrap();
    let held = store.snapshot();
    store.write(Write::ArchiveMetadata(id, 7, 340.0)).unwrap();
    let current = store.snapshot();
    assert_eq!(current.schedule.tiles[&id].archived_cycle, 7);
    assert_eq!(current.schedule.tiles[&id].terrain.archived, 340.0);
    assert_eq!(current.schedule.tiles[&id].terrain.temperature, 350.0);
    assert_eq!(current.schedule.tiles[&id].archived, archived);
    assert_eq!(current.cells[&id].air.moles[NITROGEN], 17.0);
    assert_eq!(held.schedule.tiles[&id].archived_cycle, 0);
}

#[test]
fn discard_changes_preserves_state_and_held_snapshots() {
    let mut store = Store::default();
    for index in 0..128 {
        let mut air = Mixture::new(T20C, CELL_VOLUME, true);
        air.set(OXYGEN, 100.0);
        air.set(NITROGEN, index as f32 * 7.0);
        let mut tile = Tile::new([index % 8 + 1, index / 8 + 1, 1], &air, false);
        tile.excited = index < 2;
        store
            .register(
                Cell::new(
                    air,
                    Context {
                        holder: Holder::OpenTurf,
                        ..Context::default()
                    },
                ),
                tile,
            )
            .unwrap();
    }
    store.write(Write::Neighbors(1, vec![2])).unwrap();
    store.write(Write::Neighbors(2, vec![1])).unwrap();
    let held = store.snapshot();
    let expected = format!("{held:?}");
    store.discard_changes().unwrap();
    assert!(Arc::ptr_eq(&held, &store.published));
    assert_eq!(format!("{:?}", store.published), expected);
    drop(held);
    store.discard_changes().unwrap();
    assert_eq!(format!("{:?}", store.published), expected);
    for _ in 0..4 {
        assert_full_scan(&mut store);
    }
}

fn assert_full_scan(store: &mut Store) {
    store.discard_changes().unwrap();
    let before = store.snapshot();
    let job = store.begin().unwrap();
    let oracle = Job {
        epoch: job.epoch,
        frame: job.frame.clone(),
        spare: None,
        sync_ids: BTreeSet::new(),
        resync_all: true,
    }
    .calculate();
    let completed = job.calculate();
    assert_eq!(
        format!("{:?}", completed.frame),
        format!("{:?}", oracle.frame),
        "reused buffer differs from a fresh full copy"
    );
    assert_eq!(
        format!("{:?}", completed.commands),
        format!("{:?}", oracle.commands)
    );
    let expected = completed
        .frame
        .changed_cells(&before, completed.frame.cells.keys());
    drop(before);
    store.finish(completed).unwrap();
    assert!(store.spare.is_some());
    let after = store.snapshot();
    let actual: BTreeSet<_> = store.take_changes().unwrap().into_iter().collect();
    assert_eq!(
        actual, expected,
        "incomplete work set at cycle {}",
        after.epoch
    );
    store.take_commands().unwrap();
}

#[test]
fn work_set_matches_full_scan_through_pressure_fire_groups_and_conduction() {
    let mut store = Store::default();
    for index in 0..64 {
        let closed = index % 13 == 0;
        let mut air = Mixture::new(200.0 + index as f32 * 65.0, CELL_VOLUME, true);
        if !closed {
            air.set(NITROGEN, (index * 17 % 73) as f32);
            air.set(OXYGEN, 100.0);
            air.set(PLASMA, (index % 7) as f32);
            air.set(WATER_VAPOR, 1.0);
        }
        let mut tile = Tile::new([index % 8 + 1, index / 8 + 1, 1], &air, true);
        tile.terrain.closed = closed;
        tile.has_air = !closed;
        if index < 8 && !closed {
            air = Mixture::new(TCMB, CELL_VOLUME, true);
            tile.boundary = crate::transport::Boundary::Space;
        }
        tile.terrain.temperature = 400.0 + index as f32 * 120.0;
        store
            .register(
                Cell::new(
                    air,
                    Context {
                        holder: Holder::OpenTurf,
                        ..Context::default()
                    },
                ),
                tile,
            )
            .unwrap();
    }
    for id in 1..=64 {
        let snapshot = store.snapshot();
        let tile = &snapshot.schedule.tiles[&id];
        let [x, y, _] = tile.coordinates;
        let links = [
            (y < 8).then_some(id + 8),
            (y > 1).then_some(id.wrapping_sub(8)),
            (x < 8).then_some(id + 1),
            (x > 1).then_some(id - 1),
        ];
        let neighbors = links
            .iter()
            .flatten()
            .copied()
            .filter(|n| tile.has_air && snapshot.schedule.tiles[n].has_air)
            .collect();
        store.write(Write::Neighbors(id, neighbors)).unwrap();
        store.write(Write::ThermalNeighbors(id, links)).unwrap();
    }
    for _ in 0..80 {
        assert_full_scan(&mut store);
    }
}

#[test]
fn buffer_sync_keeps_group_edits_rebinding_removal_and_new_coordinates() {
    let mut store = Store::default();
    for x in 1..=2 {
        let mut air = Mixture::new(293.15, CELL_VOLUME, true);
        air.set(NITROGEN, x as f32 * 100.0);
        let tile = Tile::new([x, 1, 1], &air, true);
        store
            .register(Cell::new(air, Context::default()), tile)
            .unwrap();
    }
    store.write(Write::Neighbors(1, vec![2])).unwrap();
    store.write(Write::Neighbors(2, vec![1])).unwrap();
    assert_full_scan(&mut store);
    store.write(Write::Control(1, 1)).unwrap();
    assert_full_scan(&mut store);
    let version = store.invalidate_binding(1).unwrap();
    let air = Mixture::new(900.0, CELL_VOLUME, true);
    let mut tile = Tile::new([1, 1, 1], &air, true);
    tile.has_air = false;
    tile.terrain.closed = true;
    store
        .rebind(
            1,
            version,
            Rebinding {
                cell: Cell::new(air, Context::default()),
                tile,
                inherit_group: false,
                inherit_gas: false,
            },
        )
        .unwrap();
    assert_full_scan(&mut store);
    let job = store.begin().unwrap();
    store.write(Write::Remove(2)).unwrap();
    store.finish(job.calculate()).unwrap();
    store.take_commands().unwrap();
    assert_eq!(store.cell_at([2, 1, 1]), None);
    assert_full_scan(&mut store);
    let air = Mixture::new(300.0, CELL_VOLUME, true);
    let tile = Tile::new([2, 1, 1], &air, true);
    let replacement = store
        .register(Cell::new(air, Context::default()), tile)
        .unwrap();
    assert_ne!(replacement, 2);
    assert_full_scan(&mut store);
    store.set_processing(vec![replacement], vec![1]).unwrap();
    assert_full_scan(&mut store);
}

#[test]
fn held_snapshots_are_never_recycled_or_overwritten() {
    let mut store = Store::default();
    let mut air = Mixture::new(293.15, CELL_VOLUME, true);
    air.set(NITROGEN, 10.0);
    let tile = Tile::new([1, 1, 1], &air, false);
    store
        .register(Cell::new(air, Context::default()), tile)
        .unwrap();
    assert_full_scan(&mut store);
    let protected = store.snapshot();
    let mut released = Mixture::new(293.15, CELL_VOLUME, false);
    released.set(NITROGEN, 5.0);
    store.write(Write::Release(1, released)).unwrap();
    let current = store.snapshot();
    let completed = store.begin().unwrap().calculate();
    store.finish(completed).unwrap();
    assert!(store.spare.is_none());
    assert_eq!(protected.cells[&1].air.moles[NITROGEN], 10.0);
    assert_eq!(current.cells[&1].air.moles[NITROGEN], 15.0);
    drop(protected);
    drop(current);
    store.take_commands().unwrap();
    assert_full_scan(&mut store);
    assert_full_scan(&mut store);
}

#[test]
fn space_share_is_normalized_after_an_external_mixture_replacement() {
    let mut store = Store::default();
    for x in 1..=3 {
        let mut air = Mixture::new(if x == 1 { 293.15 } else { TCMB }, CELL_VOLUME, true);
        if x == 1 {
            air.set(NITROGEN, 100.0);
        }
        let mut tile = Tile::new([x, 1, 1], &air, false);
        if x != 1 {
            tile.boundary = crate::transport::Boundary::Space;
        }
        store
            .register(Cell::new(air, Context::default()), tile)
            .unwrap();
    }
    store.write(Write::Neighbors(1, vec![2])).unwrap();
    store.write(Write::Neighbors(2, vec![1, 3])).unwrap();
    store.write(Write::Neighbors(3, vec![2])).unwrap();
    assert_full_scan(&mut store);
    for id in 1..=3 {
        store.write(Write::Control(id, 3)).unwrap();
    }
    assert_full_scan(&mut store);
    let shared = store.snapshot().schedule.last_share(3);
    assert!(shared > 0.0);
    store
        .write(Write::ReplaceFull(
            2,
            Mixture::new(TCMB, CELL_VOLUME, true),
            Archive::empty(true),
            true,
        ))
        .unwrap();
    assert_eq!(store.snapshot().schedule.last_share(2), 0.0);
    assert_full_scan(&mut store);
    assert_eq!(store.snapshot().schedule.last_share(2), shared);
    assert_full_scan(&mut store);
    let held = store.snapshot();
    store
        .write(Write::ReplaceFull(
            2,
            Mixture::new(TCMB, CELL_VOLUME, true),
            Archive::empty(true),
            true,
        ))
        .unwrap();
    let replaced = store.snapshot();
    let completed = store.begin().unwrap().calculate();
    store.finish(completed).unwrap();
    assert_eq!(held.schedule.last_share(2), shared);
    assert_eq!(replaced.schedule.last_share(2), 0.0);
    assert_eq!(store.snapshot().schedule.last_share(2), shared);
    drop((held, replaced));
    assert_full_scan(&mut store);
}

#[test]
fn unchanged_input_writes_do_not_project_air_but_still_update_worker_context() {
    let mut store = Store::default();
    let mut air = Mixture::new(293.15, CELL_VOLUME, true);
    air.set(NITROGEN, 10.0);
    let tile = Tile::new([1, 1, 1], &air, false);
    store
        .register(Cell::new(air, Context::default()), tile)
        .unwrap();
    assert_full_scan(&mut store);
    let context = Context {
        groundless: true,
        ..Context::default()
    };
    store
        .write(Write::Context(1, context, false, false))
        .unwrap();
    let terrain = store.snapshot().schedule.tiles[&1].terrain.clone();
    store.write(Write::Terrain(1, terrain)).unwrap();
    let snapshot = store.snapshot();
    let current = snapshot.cells[&1].air.clone();
    let archive = snapshot.schedule.tiles[&1].archived.clone();
    drop(snapshot);
    store
        .write(Write::Commit(1, current.clone(), archive.clone(), false))
        .unwrap();
    assert!(store.take_changes().unwrap().is_empty());
    assert_full_scan(&mut store);
    assert!(store.snapshot().cells[&1].context.groundless);
    let mut changed = current;
    changed.volume = 123.0;
    store
        .write(Write::Commit(1, changed, archive, false))
        .unwrap();
    // A rejected receiver volume still needs publication to repair a caller's cached value.
    assert_eq!(store.take_changes().unwrap(), [1]);
    assert_eq!(store.snapshot().cells[&1].air.volume, CELL_VOLUME);
}

#[test]
fn inactive_hotspot_and_solid_conduction_are_in_the_work_set() {
    let mut store = Store::default();
    for x in 1..=3 {
        let mut air = Mixture::new(5000.0, CELL_VOLUME, true);
        air.set(OXYGEN, 100.0);
        air.set(PLASMA, 10.0);
        let mut tile = Tile::new([x, 1, 1], &air, true);
        tile.excited = false;
        tile.terrain.closed = x == 1;
        tile.has_air = x != 1;
        tile.terrain.temperature = 6000.0;
        store
            .register(
                Cell::new(
                    air,
                    Context {
                        holder: Holder::OpenTurf,
                        ..Context::default()
                    },
                ),
                tile,
            )
            .unwrap();
    }
    store.write(Write::Neighbors(2, vec![3])).unwrap();
    store.write(Write::Neighbors(3, vec![2])).unwrap();
    store
        .write(Write::ThermalNeighbors(1, [None, None, Some(2), None]))
        .unwrap();
    store
        .write(Write::ThermalNeighbors(2, [None, None, Some(3), Some(1)]))
        .unwrap();
    store
        .write(Write::ThermalNeighbors(3, [None, None, None, Some(2)]))
        .unwrap();
    store
        .write(Write::HotspotState(
            2,
            Some(Hotspot {
                temperature: 5000.0,
                volume: CELL_VOLUME,
                just_spawned: false,
                bypassing: true,
                cold_fire: false,
            }),
        ))
        .unwrap();
    store.write(Write::Conducting(1, true)).unwrap();
    for _ in 0..20 {
        assert_full_scan(&mut store);
    }
}
