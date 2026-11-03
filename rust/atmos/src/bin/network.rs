//! Differential test protocol for a closed graph of ordinary mutable open turfs.
use bandastation_atmos::storage::Storage;
use bandastation_atmos::transport::{Boundary, Sample};
use bandastation_atmos::{catalog::*, cell::Cell, network::Schedule, Context, Holder, Mixture};
use std::io::{self, BufRead};

fn number(words: &mut std::str::SplitWhitespace<'_>) -> Result<f32, String> {
    let n: f32 = words
        .next()
        .ok_or("Missing input")?
        .parse()
        .map_err(|_| "Invalid number")?;
    if !n.is_finite() {
        return Err("Nonfinite number".into());
    }
    Ok(n)
}
fn integer(words: &mut std::str::SplitWhitespace<'_>) -> Result<usize, String> {
    let n = number(words)?;
    if !(0.0..=10000.0).contains(&n) || n.fract() != 0.0 {
        return Err("Invalid integer".into());
    }
    Ok(n as usize)
}
fn list(values: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", values.into_iter().collect::<Vec<_>>().join(","))
}
fn state(schedule: &Schedule, cells: &Storage<Cell>, fire: bool, thermal: bool) -> String {
    let groups = schedule.groups.keys().copied().collect::<Vec<_>>();
    let output = cells.iter().map(|(id, cell)| {
        let t = &schedule.tiles[&id];
        let results = list(cell.air.results.iter().map(|n| n.map_or("null".into(), |n| n.to_string())));
        let extra = if thermal { format!(",\"solid_temperature\":{},\"solid_archived\":{}",t.terrain.temperature,t.terrain.archived) } else { String::new() };
        format!("{{\"temperature\":{},\"moles\":{:?},\"keys\":{:?},\"results\":{},\"archived_temperature\":{},\"archived_moles\":{:?},\"archived_keys\":{:?},\"archived_cycle\":{},\"current_cycle\":{},\"ticker\":{},\"excited\":{},\"group\":{},\"last_share\":{},\"pressure\":{},\"direction\":{}{extra}}}",
            cell.air.temperature,cell.air.moles,cell.air.keys(),results,t.archived.temperature,t.archived.moles,t.archived.keys(),
            t.archived_cycle,t.current_cycle,t.ticker,u8::from(t.excited),t.group.map_or(0,|g| groups.iter().position(|v| *v==g).unwrap()+1),schedule.last_share(id),t.pressure,t.direction)
    });
    let groups = schedule.groups.values().map(|g| {
        format!(
            "{{\"members\":{:?},\"breakdown\":{},\"dismantle\":{}}}",
            g.members.iter().map(|n| n - 1).collect::<Vec<_>>(),
            g.breakdown,
            g.dismantle
        )
    });
    let mut extra = if fire {
        let spots = schedule.hotspots.iter().map(|id| {
            let spot = cells[id].hotspot.as_ref().unwrap();
            format!(
                "[{},{},{},{},{},{}]",
                id - 1,
                spot.temperature,
                spot.volume,
                u8::from(spot.just_spawned),
                u8::from(spot.bypassing),
                u8::from(spot.cold_fire)
            )
        });
        format!(",\"hotspots\":{}", list(spots))
    } else {
        String::new()
    };
    if thermal {
        extra += &format!(
            ",\"conducting\":{:?}",
            schedule
                .conducting
                .iter()
                .map(|n| n - 1)
                .collect::<Vec<_>>()
        );
    }
    format!(
        "{{\"active\":{:?},\"cells\":{},\"groups\":{}{extra}}}",
        schedule.active.iter().map(|n| n - 1).collect::<Vec<_>>(),
        list(output),
        list(groups)
    )
}
fn run(line: &str) -> Result<String, String> {
    let mut words = line.split_whitespace();
    let ticks = integer(&mut words)?;
    let count = integer(&mut words)?;
    let mode = integer(&mut words)?;
    if mode > 3 {
        return Err("Invalid fire flag".into());
    }
    if ticks == 0 || ticks > 100 || count == 0 || count > 100 {
        return Err("Invalid size".into());
    }
    let mut cells = Storage::new();
    let mut schedule = Schedule::default();
    for index in 0..count {
        let coordinates = [
            integer(&mut words)? as i32,
            integer(&mut words)? as i32,
            integer(&mut words)? as i32,
        ];
        let kind = integer(&mut words)?;
        let temperature = number(&mut words)?;
        let mut air = Mixture::new(temperature, CELL_VOLUME, kind == 0);
        let gases = integer(&mut words)?;
        for _ in 0..gases {
            let gas = integer(&mut words)?;
            let amount = number(&mut words)?;
            if gas >= GAS_COUNT || amount < 0.0 || air.keys().contains(&gas) {
                return Err("Invalid gas".into());
            }
            air.set(gas, amount);
        }
        let id = index as u32 + 1;
        schedule.add(id, coordinates, &air, false);
        match kind {
            0 => {}
            1 => {
                schedule.tiles.get_mut(&id).unwrap().boundary = Boundary::Space;
                schedule.tiles.get_mut(&id).unwrap().run_later = true;
            }
            2 => {
                let tile = schedule.tiles.get_mut(&id).unwrap();
                tile.boundary = Boundary::Planetary(Box::new(air.clone()));
                tile.archived = air.archive();
            }
            3 => {
                let tile = schedule.tiles.get_mut(&id).unwrap();
                tile.terrain.closed = true;
                tile.has_air = false;
                tile.excited = false;
            }
            _ => return Err("Invalid boundary".into()),
        }
        let planetary = integer(&mut words)?;
        if planetary == 1 {
            let mut initial = Mixture::new(number(&mut words)?, CELL_VOLUME, false);
            let gases = integer(&mut words)?;
            for _ in 0..gases {
                let gas = integer(&mut words)?;
                let amount = number(&mut words)?;
                if gas >= GAS_COUNT || amount < 0.0 || initial.keys().contains(&gas) {
                    return Err("Invalid atmosphere gas".into());
                }
                initial.set(gas, amount);
            }
            let mut sample = Sample::new(initial.clone());
            sample.boundary = Boundary::Planetary(Box::new(initial));
            schedule.tiles.get_mut(&id).unwrap().planetary = Some(Box::new(sample));
        } else if planetary != 0 {
            return Err("Invalid atmosphere flag".into());
        }
        let thermal = integer(&mut words)?;
        if thermal == 1 {
            let tile = schedule.tiles.get_mut(&id).unwrap();
            tile.terrain.temperature = number(&mut words)?;
            tile.terrain.capacity = number(&mut words)?;
            tile.terrain.conductivity = number(&mut words)?;
            let blocked = integer(&mut words)?;
            if tile.terrain.capacity < 0.0 || tile.terrain.conductivity < 0.0 || blocked > 255 {
                return Err("Invalid terrain".into());
            }
            tile.terrain.blocked = blocked as u8;
            tile.thermal_enabled = tile.terrain.conductivity != 0.0;
        } else if thermal != 0 {
            return Err("Invalid thermal flag".into());
        }
        let neighbors = integer(&mut words)?;
        let mut links = Vec::new();
        for _ in 0..neighbors {
            let n = integer(&mut words)?;
            if n >= count {
                return Err("Invalid neighbor".into());
            }
            links.push(n as u32 + 1);
        }
        schedule.tiles.get_mut(&id).unwrap().neighbors = links;
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
    schedule.active.clear();
    for tile in schedule.tiles.values_mut() {
        tile.excited = false;
    }
    let active = integer(&mut words)?;
    for _ in 0..active {
        let n = integer(&mut words)? as u32 + 1;
        if !cells.contains_key(&n) || schedule.active.contains(&n) {
            return Err("Invalid active cell".into());
        }
        schedule.activate(n);
    }
    let conducting = integer(&mut words)?;
    for _ in 0..conducting {
        let id = integer(&mut words)? as u32 + 1;
        if !cells.contains_key(&id) || schedule.conducting.contains(&id) {
            return Err("Invalid conducting tile".into());
        }
        schedule.conducting.push(id);
    }
    for id in cells.keys() {
        let position = schedule.tiles[&id].coordinates;
        let mut links = [None; 4];
        for (index, delta) in [[0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0]]
            .into_iter()
            .enumerate()
        {
            let target = [position[0] + delta[0], position[1] + delta[1], position[2]];
            links[index] = schedule
                .tiles
                .iter()
                .find(|(_, t)| t.coordinates == target)
                .map(|(id, _)| id);
        }
        schedule.tiles.get_mut(&id).unwrap().thermal_neighbors = links;
    }
    if words.next().is_some() {
        return Err("Extra input".into());
    }
    schedule.validate().map_err(str::to_string)?;
    let mut states = Vec::new();
    for cycle in 1..=ticks {
        let _ = if mode != 0 {
            schedule.tick(&mut cells, cycle as u32)
        } else {
            schedule.run(&mut cells, cycle as u32)
        };
        states.push(state(&schedule, &cells, mode & 1 != 0, mode & 2 != 0));
    }
    Ok(list(states))
}
fn main() -> Result<(), String> {
    for line in io::stdin().lock().lines() {
        println!("{}", run(&line.map_err(|e| e.to_string())?)?);
    }
    Ok(())
}
