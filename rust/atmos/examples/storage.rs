//! Measure map node occupancy independently of gas arrays, DM and allocator metadata.
use bandastation_atmos::{cell::Cell, network::Tile, Context, Mixture};
// Exercise private page synchronization without expanding the library's public API.
#[allow(dead_code)]
#[path = "../src/storage.rs"]
mod storage;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    mem::size_of,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};
use storage::Storage;

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// Forward the original pointer/layout unchanged; count only successful requested bytes.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        result
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            let live = if size >= layout.size() {
                LIVE.fetch_add(size - layout.size(), Relaxed) + size - layout.size()
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed) - (layout.size() - size)
            };
            PEAK.fetch_max(live, Relaxed);
        }
        result
    }
}

fn main() {
    let count: u32 = std::env::args()
        .nth(1)
        .map_or(780300, |value| value.parse().expect("entry count"));
    println!(
        "{{\"entries\":{count},\"pointer_bits\":{},\"cell_bytes\":{},\"tile_bytes\":{}}}",
        usize::BITS,
        size_of::<Cell>(),
        size_of::<Tile>()
    );
    let baseline = LIVE.load(Relaxed);
    let air = Mixture::new(2.7, 2500.0, false);
    let mut cells = BTreeMap::new();
    let mut tiles = BTreeMap::new();
    for id in 1..=count {
        cells.insert(id, Cell::new(air.clone(), Context::default()));
        tiles.insert(id, Tile::new([id as i32, 0, 0], &air, false));
    }
    let before = LIVE.load(Relaxed) - baseline;
    PEAK.store(LIVE.load(Relaxed), Relaxed);
    cells = std::mem::take(&mut cells).into_iter().collect();
    tiles = std::mem::take(&mut tiles).into_iter().collect();
    let after = LIVE.load(Relaxed) - baseline;
    let peak = PEAK.load(Relaxed) - baseline;
    assert_eq!(cells.len(), count as usize);
    assert_eq!(tiles.len(), count as usize);
    assert!(cells.keys().copied().eq(1..=count));
    assert!(tiles.keys().copied().eq(1..=count));
    println!("{{\"one_frame_before_bytes\":{before},\"one_frame_after_bytes\":{after},\"rebuild_peak_bytes\":{peak}}}");
    std::hint::black_box((&cells, &tiles));
    drop((cells, tiles));
    let mut cells = Storage::new();
    let mut tiles = Storage::new();
    for id in 1..=count {
        cells.insert(id, Cell::new(air.clone(), Context::default()));
        tiles.insert(id, Tile::new([id as i32, 0, 0], &air, false));
    }
    let one = LIVE.load(Relaxed) - baseline;
    let started = std::time::Instant::now();
    let snapshot = (cells.clone(), tiles.clone());
    let clone_us = started.elapsed().as_micros();
    let shared = LIVE.load(Relaxed) - baseline;
    let started = std::time::Instant::now();
    for id in 1..=count / 8 {
        cells.get_mut(&id).unwrap().flags = 1;
        tiles.get_mut(&id).unwrap().ticker = 1;
    }
    let write_us = started.elapsed().as_micros();
    let active = LIVE.load(Relaxed) - baseline;
    println!("{{\"paged_one_frame_bytes\":{one},\"paged_two_shared_bytes\":{shared},\"paged_two_12_percent_local_writes_bytes\":{active},\"clone_us\":{clone_us},\"write_us\":{write_us},\"option_cell_bytes\":{},\"option_tile_bytes\":{}}}", size_of::<Option<Cell>>(), size_of::<Option<Tile>>());
    let started = std::time::Instant::now();
    for id in 1..=count {
        cells.get_mut(&id).unwrap().flags = 1;
        tiles.get_mut(&id).unwrap().ticker = 1;
    }
    let all = LIVE.load(Relaxed) - baseline;
    println!(
        "{{\"paged_two_all_written_bytes\":{all},\"all_write_us\":{}}}",
        started.elapsed().as_micros()
    );
    std::hint::black_box((&cells, &tiles, &snapshot));
    drop((cells, tiles, snapshot));
    if count > 0 {
        for writes in [32, 512] {
            scattered(count, writes);
        }
    }
}

fn scattered(count: u32, writes: u32) {
    let baseline = LIVE.load(Relaxed);
    let mut cells = Storage::new();
    let mut tiles = Storage::new();
    let empty = Mixture::new(2.7, 2500.0, false);
    let mut room = Mixture::new(293.15, 2500.0, false);
    room.set(bandastation_atmos::catalog::OXYGEN, 20.0);
    room.set(bandastation_atmos::catalog::NITROGEN, 80.0);
    let room_count = count.min(100000);
    for id in 1..=count {
        let air = if id <= room_count { &room } else { &empty };
        cells.insert(id, Cell::new(air.clone(), Context::default()));
        let mut tile = Tile::new([id as i32, 0, 0], air, false);
        tile.archived = air.archive();
        tiles.insert(id, tile);
    }
    let mut previous = (cells.clone(), tiles.clone());
    let mut ids: Vec<_> = (0..writes).map(|n| 1 + (n * 997) % room_count).collect();
    ids.sort_unstable();
    PEAK.store(LIVE.load(Relaxed), Relaxed);
    let started = std::time::Instant::now();
    for cycle in 1..=120 {
        cells.synchronize(&previous.0, ids.iter().copied());
        tiles.synchronize(&previous.1, ids.iter().copied());
        for id in &ids {
            cells.get_mut(id).unwrap().air.temperature += 0.01;
            tiles.get_mut(id).unwrap().ticker = cycle;
        }
        std::mem::swap(&mut previous.0, &mut cells);
        std::mem::swap(&mut previous.1, &mut tiles);
    }
    let elapsed = started.elapsed().as_micros();
    let live = LIVE.load(Relaxed) - baseline;
    let peak = PEAK.load(Relaxed) - baseline;
    println!("{{\"writes_per_tick\":{writes},\"ticks\":120,\"elapsed_us\":{elapsed},\"live_bytes\":{live},\"peak_bytes\":{peak}}}");
    std::hint::black_box((&cells, &tiles, &previous));
}
