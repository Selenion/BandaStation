use bandastation_atmos::storage::Storage;
use bandastation_atmos::{catalog::*, cell::Cell, network::Schedule, Context, Holder, Mixture};
use std::collections::BTreeMap;

fn graph(positions: &[[i32; 3]], nitrogen: &[f32]) -> (Schedule, Storage<Cell>) {
    let mut schedule = Schedule::default();
    let mut cells = Storage::new();
    for (index, (&position, &amount)) in positions.iter().zip(nitrogen).enumerate() {
        let id = index as u32 + 1;
        let mut air = Mixture::new(293.15, CELL_VOLUME, true);
        air.set(NITROGEN, amount);
        schedule.add(id, position, &air, false);
        cells.insert(
            id,
            Cell::new(
                air,
                Context {
                    holder: Holder::OpenTurf,
                    ..Context::default()
                },
            ),
        );
    }
    (schedule, cells)
}

fn connect(schedule: &mut Schedule, a: u32, b: u32) {
    schedule.tiles.get_mut(&a).unwrap().neighbors.push(b);
    schedule.tiles.get_mut(&b).unwrap().neighbors.push(a);
}

#[test]
fn run_later_without_a_group_keeps_exchange_and_tickers() {
    // Both deltas are below compare's relative threshold; the second exchange
    // exceeds even the breakdown timer threshold. Neither needs a new group.
    for (amount, delta) in [(1000.0, 0.5), (1_000_000.0, 6.0)] {
        let (mut schedule, mut cells) = graph(&[[1, 1, 1], [2, 1, 1]], &[amount, amount - delta]);
        cells.get_mut(&1).unwrap().air.set(OXYGEN, amount);
        cells.get_mut(&2).unwrap().air.set(OXYGEN, amount + delta);
        connect(&mut schedule, 1, 2);
        schedule.tiles.get_mut(&2).unwrap().run_later = true;
        schedule.tiles.get_mut(&1).unwrap().ticker = 5;
        schedule.tiles.get_mut(&2).unwrap().ticker = 5;
        let commands = schedule.tick(&mut cells, 1);
        assert!(schedule.groups.is_empty());
        // With coefficients (1, 1), DM transfers the full difference, rather
        // than averaging the cells as the ordinary neighboring exchange does.
        assert_eq!(cells[&1].air.moles[OXYGEN], amount + delta);
        assert_eq!(cells[&1].air.moles[NITROGEN], amount - delta);
        assert_eq!(cells[&2].air.moles[OXYGEN], amount);
        assert_eq!(cells[&2].air.moles[NITROGEN], amount);
        for id in [1, 2] {
            assert_eq!(schedule.tiles[&id].ticker, 0);
            assert_eq!(schedule.tiles[&id].current_cycle, 1);
            assert!(schedule.tiles[&id].group.is_none());
        }
        assert!(commands.len() >= 2, "both cells must finish their exposure");
    }
}

#[test]
fn debug_max_share_is_optional_and_resets_on_each_processing_pass() {
    let (mut schedule, mut cells) = graph(&[[1, 1, 1], [2, 1, 1]], &[100.0, 10.0]);
    connect(&mut schedule, 1, 2);
    assert!(schedule.max_shares.is_none());
    schedule.max_shares = Some(BTreeMap::new());
    let _ = schedule.run(&mut cells, 1);
    let maximum = schedule.max_shares.as_ref().unwrap()[&2];
    assert!(maximum > 0.0);
    let saved = schedule.clone();
    for cell in cells.values_mut() {
        cell.air.set(NITROGEN, 55.0);
    }
    let _ = schedule.run(&mut cells, 2);
    assert_eq!(schedule.max_shares.as_ref().unwrap()[&2], 0.0);
    assert_eq!(saved.max_shares.as_ref().unwrap()[&2], maximum);
    schedule.remove(1);
    assert!(!schedule.max_shares.as_ref().unwrap().contains_key(&1));
}

#[test]
fn an_activated_neighbor_waits_until_the_next_pass() {
    let (mut schedule, mut cells) = graph(&[[1, 1, 1], [2, 1, 1], [3, 1, 1]], &[400.0, 0.0, 0.0]);
    connect(&mut schedule, 1, 2);
    connect(&mut schedule, 2, 3);
    schedule.active = vec![1];
    schedule.tiles.get_mut(&2).unwrap().excited = false;
    schedule.tiles.get_mut(&3).unwrap().excited = false;
    let _ = schedule.run(&mut cells, 1);
    // Original DM oracle: wave_from_one_active_cell, first and second tick.
    assert_eq!(schedule.active, [1, 2]);
    assert_eq!(cells[&3].air.moles[NITROGEN], 0.0);
    assert_eq!(schedule.tiles[&2].current_cycle, 0);
    let _ = schedule.run(&mut cells, 2);
    assert_eq!(schedule.active, [1, 2, 3]);
    assert!((cells[&3].air.moles[NITROGEN] - 66.6666718).abs() < 0.0001);
    assert_eq!(schedule.tiles[&3].current_cycle, 0);
}

#[test]
fn the_second_boundary_refreshes_an_unprocessed_neighbors_archive() {
    let (mut schedule, mut cells) = graph(&[[2, 1, 1], [1, 1, 1], [3, 1, 1]], &[100.0, 300.0, 0.0]);
    connect(&mut schedule, 1, 2);
    connect(&mut schedule, 1, 3);
    schedule.active = vec![2, 3];
    schedule.tiles.get_mut(&1).unwrap().excited = false;
    let _ = schedule.run(&mut cells, 1);
    // Original DM oracle: neighbor_is_rearchived. One frame-wide archive would retain 100.
    assert!((schedule.tiles[&1].archived.moles[NITROGEN] - 66.6666641).abs() < 0.0001);
    assert!((cells[&1].air.moles[NITROGEN] - 183.333359).abs() < 0.0001);
    assert_eq!(schedule.groups.values().next().unwrap().members, [3, 1, 2]);
}

#[test]
fn vertical_exchange_breakdown_and_sleep_match_banda() {
    let (mut schedule, mut cells) = graph(&[[1, 1, 1], [1, 1, 2]], &[400.0, 0.0]);
    connect(&mut schedule, 1, 2);
    schedule.validate().unwrap();
    for cycle in 1..=5 {
        let _ = schedule.run(&mut cells, cycle);
    }
    assert_eq!(cells[&1].air.moles[NITROGEN], 200.0);
    assert_eq!(cells[&2].air.moles[NITROGEN], 200.0);
    assert_eq!(schedule.tiles[&1].archived.moles[NITROGEN], 0.0);
    assert_eq!(schedule.tiles[&1].ticker, 11);
    for cycle in 6..=11 {
        let _ = schedule.run(&mut cells, cycle);
    }
    assert!(schedule.active.is_empty());
    assert!(schedule.groups.is_empty());
}

#[test]
fn a_closed_boundary_passes_no_gas_and_invalid_links_are_rejected() {
    let (mut schedule, mut cells) = graph(&[[1, 1, 1], [2, 1, 1]], &[400.0, 0.0]);
    let _ = schedule.run(&mut cells, 1);
    assert_eq!(cells[&1].air.moles[NITROGEN], 400.0);
    assert_eq!(cells[&2].air.moles[NITROGEN], 0.0);
    assert!(schedule.active.is_empty());
    schedule.tiles.get_mut(&1).unwrap().neighbors.push(2);
    assert!(schedule.validate().is_err());
    schedule.tiles.get_mut(&2).unwrap().neighbors.push(1);
    schedule.validate().unwrap();
    schedule.tiles.get_mut(&2).unwrap().coordinates = [3, 1, 1];
    assert!(schedule.validate().is_err());
}

#[test]
fn an_active_hotspot_reactivates_its_turf_when_returning_a_gas_sample() {
    let (mut schedule, mut cells) = graph(&[[1, 1, 1]], &[0.0]);
    let mut air = Mixture::new(1644.0, CELL_VOLUME, true);
    air.set(OXYGEN, 0.51);
    air.set(PLASMA, 1.0);
    cells.get_mut(&1).unwrap().air = air;
    // Full Banda world capture: oxygen_just_above_birth, including the final death of the spot.
    for (cycle, ticker) in [(1, 0), (2, 0), (3, 1)] {
        let _ = schedule.tick(&mut cells, cycle);
        assert_eq!(schedule.tiles[&1].ticker, ticker);
    }
    assert!(schedule.hotspots.is_empty());
    assert!(cells[&1].hotspot.is_none());
}

#[test]
fn breakdown_uses_space_capacity_even_though_space_is_not_a_turf_mixture() {
    use bandastation_atmos::transport::Boundary;
    let (mut schedule, mut cells) = graph(&[[1, 1, 1], [2, 1, 1], [3, 1, 1]], &[300.0, 0.0, 50.0]);
    cells.get_mut(&1).unwrap().air.set(OXYGEN, 100.0);
    cells.get_mut(&2).unwrap().air = Mixture::new(TCMB, CELL_VOLUME, false);
    cells.get_mut(&3).unwrap().air.temperature = 2000.0;
    schedule.tiles.get_mut(&2).unwrap().boundary = Boundary::Space;
    schedule.tiles.get_mut(&2).unwrap().run_later = true;
    connect(&mut schedule, 1, 2);
    connect(&mut schedule, 2, 3);
    for cycle in 1..=5 {
        let _ = schedule.run(&mut cells, cycle);
    }
    // Original full-game two_rooms_share_one_space: both empty floors and space contribute 7000.
    assert!((cells[&1].air.temperature - 668.466675).abs() < 0.001);
    assert_eq!(cells[&2].air.temperature, TCMB);
}
