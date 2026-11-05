//! Sequential queue stress test; hashes are computed outside the timed calculation.
use bandastation_atmos::{
    catalog::*,
    cell::{Cell, Hotspot},
    network::Schedule,
    storage::Storage,
    Context, Holder, Mixture,
};
use std::{
    fmt::{self, Write},
    time::Instant,
};

struct Digest(u64);
impl Write for Digest {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for byte in value.bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        Ok(())
    }
}

fn main() {
    let trace = std::env::args().any(|arg| arg == "--trace");
    let modes: &[&str] = if trace {
        &["stress"]
    } else {
        &["equal", "mixed", "gradient", "fire"]
    };
    let widths: &[u32] = if trace { &[8] } else { &[16, 64, 128, 255] };
    let mut random = 0x051a_7220u32;
    for &mode in modes {
        for &width in widths {
            let mut schedule = Schedule::default();
            let mut cells = Storage::new();
            for y in 0..width {
                for x in 0..width {
                    let id = y * width + x + 1;
                    let mut air = Mixture::new(
                        if mode == "fire" { 700.0 } else { 293.15 },
                        CELL_VOLUME,
                        true,
                    );
                    air.set(OXYGEN, 100.0);
                    air.set(
                        NITROGEN,
                        if mode == "gradient" {
                            if (x + y) % 2 == 0 {
                                600.0
                            } else {
                                1.0
                            }
                        } else {
                            300.0
                        },
                    );
                    if mode == "fire" {
                        air.set(PLASMA, 20.0);
                    }
                    schedule.add(id, [x as i32 + 1, y as i32 + 1, 1], &air, false);
                    let mut cell = Cell::new(
                        air,
                        Context {
                            holder: Holder::OpenTurf,
                            ..Context::default()
                        },
                    );
                    if mode == "mixed" && id % 2 == 1 {
                        cell.hotspot = Some(Hotspot {
                            temperature: 700.0,
                            volume: 100.0,
                            just_spawned: false,
                            bypassing: false,
                            cold_fire: false,
                        });
                    }
                    cells.insert(id, cell);
                }
            }
            for y in 0..width {
                for x in 0..width {
                    let id = y * width + x + 1;
                    let neighbors = &mut schedule.tiles.get_mut(&id).unwrap().neighbors;
                    if y + 1 < width {
                        neighbors.push(id + width);
                    }
                    if y > 0 {
                        neighbors.push(id - width);
                    }
                    if x + 1 < width {
                        neighbors.push(id + 1);
                    }
                    if x > 0 {
                        neighbors.push(id - 1);
                    }
                }
            }
            for step in 1..=if trace { 10000 } else { 3 } {
                if trace {
                    random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                    let id = random % (width * width) + 1;
                    let air = &mut cells.get_mut(&id).unwrap().air;
                    air.set(NITROGEN, (random % 600 + 1) as f32);
                    air.set(PLASMA, if random & 16 == 0 { 15.0 } else { 0.0 });
                    air.temperature = if random & 32 == 0 { 700.0 } else { 293.15 };
                    schedule.tiles.get_mut(&id).unwrap().run_later = random & 64 != 0;
                    schedule.activate(id);
                }
                let start = Instant::now();
                let commands = if mode == "mixed" {
                    schedule.run(&mut cells, step)
                } else {
                    schedule.tick(&mut cells, step)
                };
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                let mut digest = Digest(0xcbf29ce484222325);
                for (id, cell) in cells.iter() {
                    write!(digest, "{id}:{cell:?}:{:?}", schedule.tiles[&id]).unwrap();
                }
                write!(
                    digest,
                    "{:?}{:?}{:?}{:?}{commands:?}",
                    schedule.active, schedule.groups, schedule.hotspots, schedule.conducting
                )
                .unwrap();
                println!("{{\"mode\":\"{mode}\",\"cells\":{},\"step\":{step},\"ms\":{ms},\"hash\":\"{:016x}\",\"active\":{},\"commands\":{}}}", width*width, digest.0, schedule.active.len(), commands.len());
            }
        }
    }
}
