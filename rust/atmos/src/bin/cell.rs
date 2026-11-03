//! Local protocol for comparing native cell physics with real Banda world captures.
use bandastation_atmos::{
    catalog::*,
    cell::{Cell, WorldAction},
    Context, Effect, Holder, Mixture,
};
use std::io::{self, BufRead};

fn run(line: &str) -> Result<String, String> {
    let mut words = line.split_whitespace();
    let mut next = || -> Result<f32, String> {
        let value: f32 = words
            .next()
            .ok_or("Missing input")?
            .parse()
            .map_err(|_| "Invalid number")?;
        if !value.is_finite() {
            return Err("Nonfinite input".into());
        }
        Ok(value)
    };
    let mode = next()? as u8;
    let temperature = next()?;
    let count = next()? as usize;
    if mode > 1 || count > GAS_COUNT {
        return Err("Invalid input".into());
    }
    let mut air = Mixture::new(temperature, CELL_VOLUME, true);
    for _ in 0..count {
        let gas = next()? as usize;
        let amount = next()?;
        if gas >= GAS_COUNT || amount < 0.0 || air.keys().contains(&gas) {
            return Err("Invalid gas".into());
        }
        air.set(gas, amount);
    }
    if words.next().is_some() {
        return Err("Extra input".into());
    }
    let mut cell = Cell::new(
        air,
        Context {
            holder: Holder::OpenTurf,
            ..Context::default()
        },
    );
    let flags = if mode == 0 {
        cell.react().flags
    } else {
        cell.hotspot_expose(temperature, CELL_VOLUME, false);
        0
    };
    let hotspot = cell.hotspot.as_ref();
    let mut actions = Vec::new();
    for command in &cell.commands {
        let (kind, a, b) = match command.action {
            WorldAction::Reaction(Effect::Freeze) => (1, 0.0, 0.0),
            WorldAction::Reaction(Effect::Wet) => (2, 0.0, 0.0),
            WorldAction::FireExposure {
                temperature,
                volume,
            } => (9, temperature, volume),
            _ => return Err("Unrepresented action in world fixture".into()),
        };
        actions.push(format!(
            "[{kind},{a},{b},{},{}]",
            command.air.moles[OXYGEN], command.air.temperature
        ));
    }
    let results = cell
        .air
        .results
        .iter()
        .map(|v| v.map_or("null".to_string(), |n| n.to_string()))
        .collect::<Vec<_>>();
    Ok(format!("{{\"flags\":{flags},\"temperature\":{},\"moles\":{:?},\"keys\":{:?},\"results\":[{}],\"hotspot\":{},\"hotspot_temperature\":{},\"hotspot_volume\":{},\"actions\":[{}]}}",
        cell.air.temperature, cell.air.moles, cell.air.keys(), results.join(","), hotspot.is_some(),
        hotspot.map_or("null".to_string(), |s| s.temperature.to_string()), hotspot.map_or("null".to_string(), |s| s.volume.to_string()), actions.join(",")))
}

fn main() -> Result<(), String> {
    for line in io::stdin().lock().lines() {
        println!("{}", run(&line.map_err(|e| e.to_string())?)?);
    }
    Ok(())
}
