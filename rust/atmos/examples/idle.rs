//! Isolate full-map snapshot/queue overhead without chemistry, DM objects or active cells.
use bandastation_atmos::{
    catalog::{CELL_VOLUME, TCMB},
    cell::Cell,
    network::Tile,
    phase::Store,
    transport::Boundary,
    Context, Mixture,
};
use std::time::Instant;

fn main() {
    let count: u32 = std::env::args()
        .nth(1)
        .map_or(780300, |value| value.parse().expect("cell count"));
    assert!(count <= 780300);
    let mut store = Store::default();
    let start = Instant::now();
    for index in 0..count {
        let air = Mixture::new(TCMB, CELL_VOLUME, true);
        let coordinates = [
            (index % 255 + 1) as i32,
            (index / 255 % 255 + 1) as i32,
            (index / (255 * 255) + 1) as i32,
        ];
        let mut tile = Tile::new(coordinates, &air, false);
        tile.excited = false;
        tile.boundary = Boundary::Space;
        store
            .register(Cell::new(air, Context::default()), tile)
            .unwrap();
    }
    println!(
        "{{\"cells\":{count},\"register_ms\":{}}}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    store.discard_changes().unwrap();
    for tick in 1..=4 {
        let start = Instant::now();
        let job = store.begin().unwrap();
        let begin = start.elapsed();
        let start = Instant::now();
        let result = job.calculate();
        let calculate = start.elapsed();
        let start = Instant::now();
        store.finish(result).unwrap();
        let finish = start.elapsed();
        assert!(store.take_changes().unwrap().is_empty());
        println!(
            "{{\"tick\":{tick},\"begin_ms\":{},\"calculate_ms\":{},\"finish_ms\":{}}}",
            begin.as_secs_f64() * 1000.0,
            calculate.as_secs_f64() * 1000.0,
            finish.as_secs_f64() * 1000.0
        );
    }
}
