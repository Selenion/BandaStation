use bandastation_atmos::{
    cell::{Cell, Hotspot},
    network::Tile,
    phase::{Error, Store, Write},
    Context, Mixture,
};

fn register(store: &mut Store, xyz: [i32; 3]) -> u32 {
    let cell = Cell::new(Mixture::new(293.15, 2500.0, true), Context::default());
    let tile = Tile::new(xyz, &cell.air, false);
    store.register(cell, tile).unwrap()
}

#[test]
fn coordinate_lookup_is_unique_and_removal_releases_the_position() {
    let mut store = Store::default();
    let first = register(&mut store, [1, 2, 3]);
    assert_eq!(store.cell_at([1, 2, 3]), Some(first));
    assert_eq!(store.cell_at([3, 2, 1]), None);
    let snapshot = store.snapshot();
    assert_eq!(
        store.register(
            snapshot.cells[&first].clone(),
            snapshot.schedule.tiles[&first].clone()
        ),
        Err(Error::InvalidTopology)
    );
    store.write(Write::Remove(first)).unwrap();
    assert_eq!(store.cell_at([1, 2, 3]), None);
    let second = register(&mut store, [1, 2, 3]);
    assert_ne!(first, second);
    store.close();
    assert_eq!(store.cell_at([1, 2, 3]), None);
}

#[test]
fn world_topology_does_not_wake_sleeping_cells_and_changes_are_consumed_once() {
    let mut store = Store::default();
    let a = register(&mut store, [1, 1, 1]);
    let b = register(&mut store, [2, 1, 1]);
    store.set_processing(vec![], vec![]).unwrap();
    store
        .write(Write::Topology(a, vec![b], [None, None, Some(b), None]))
        .unwrap();
    store
        .write(Write::Topology(b, vec![a], [None, None, None, Some(a)]))
        .unwrap();
    assert!(store.snapshot().schedule.active.is_empty());
    assert_eq!(store.take_changes().unwrap(), [a, b]);
    assert!(store.take_changes().unwrap().is_empty());
    store.write(Write::Control(a, 0)).unwrap();
    store.discard_changes().unwrap();
    let job = store.begin().unwrap();
    assert_eq!(store.take_changes(), Err(Error::Busy));
    assert_eq!(store.discard_changes(), Err(Error::Busy));
    store.finish(job.calculate()).unwrap();
    let changed = store.take_changes().unwrap();
    assert!(changed.contains(&a));
    assert!(changed.contains(&b)); // Neighbor archives change even without its own active pass.
}

#[test]
fn external_hotspot_keeps_its_state_and_has_only_one_processing_entry() {
    let mut store = Store::default();
    let id = register(&mut store, [1, 1, 1]);
    let spot = Hotspot {
        temperature: 700.0,
        volume: 100.0,
        just_spawned: true,
        bypassing: false,
        cold_fire: false,
    };
    store
        .write(Write::HotspotState(id, Some(spot.clone())))
        .unwrap();
    store
        .write(Write::HotspotState(id, Some(spot.clone())))
        .unwrap();
    assert_eq!(store.snapshot().cells[&id].hotspot, Some(spot));
    assert_eq!(store.snapshot().schedule.hotspots, [id]);
    store.write(Write::HotspotState(id, None)).unwrap();
    assert!(store.snapshot().schedule.hotspots.is_empty());
    assert!(store.snapshot().cells[&id].hotspot.is_none());
}

#[test]
fn world_cycles_keep_the_real_ssair_number_after_an_initialization_gap() {
    let mut store = Store::default();
    let id = register(&mut store, [1, 1, 1]);
    let job = store.begin_at(5).unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().epoch, 5);
    assert_eq!(store.snapshot().schedule.tiles[&id].current_cycle, 5);
    store.take_commands().unwrap();
    assert!(matches!(store.begin_at(5), Err(Error::StaleJob)));
    store.write(Write::Control(id, 0)).unwrap();
    let job = store.begin_at(11).unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().epoch, 11);
    assert_eq!(store.snapshot().schedule.tiles[&id].current_cycle, 11);
    assert_eq!(store.snapshot().schedule.tiles[&id].archived_cycle, 11);
}
