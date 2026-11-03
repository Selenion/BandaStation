//! Line protocol used only by tools/atmos/compare.py; not a BYOND FFI.
use bandastation_atmos::{catalog::*, Context, Effect, Holder, Mixture};
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
            return Err("Nonfinite number".into());
        }
        Ok(value)
    };
    let index = next()? as i32;
    if index < -1 || index >= REACTIONS.len() as i32 {
        return Err("Invalid reaction".into());
    }
    let ticks = next()? as usize;
    if ticks == 0 || ticks > 100 {
        return Err("Invalid tick count".into());
    }
    let temperature = next()?;
    let volume = next()?;
    if temperature < 0.0 || volume <= 0.0 {
        return Err("Invalid mixture".into());
    }
    let mut mixture = Mixture::new(temperature, volume, next()? != 0.0);
    let holder = match next()? as i32 {
        0 => Holder::None,
        1 => Holder::Atom,
        2 => Holder::Turf,
        3 => Holder::OpenTurf,
        4 => Holder::PipelineAtom,
        5 => Holder::PipelineOpenTurf,
        _ => return Err("Invalid holder".into()),
    };
    let bits = next()? as u32;
    let context = Context {
        holder,
        freeze_succeeds: bits & 1 != 0,
        permafrost: bits & 2 != 0,
        groundless: bits & 4 != 0,
        no_slip: bits & 8 != 0,
        foam: bits & 16 != 0,
        resin: bits & 32 != 0,
        radiation_roll: bits & 64 != 0,
        hot_ice_roll: bits & 128 != 0,
        defer_random: bits & 512 != 0,
        working_power: next()?,
        supermatter_power: next()?,
    };
    if next()? != 0.0 {
        mixture.results.fill(Some(123.0));
    }
    let count = next()? as usize;
    if count > GAS_COUNT {
        return Err("Too many gases".into());
    }
    for _ in 0..count {
        let gas = next()? as usize;
        let value = next()?;
        if gas >= GAS_COUNT || value < 0.0 || mixture.keys().contains(&gas) {
            return Err("Invalid gas".into());
        }
        mixture.set(gas, value);
    }
    if words.next().is_some() {
        return Err("Extra input".into());
    }
    let initial_heat = mixture.heat_capacity();
    let mut outcome = bandastation_atmos::Outcome::default();
    for _ in 0..ticks {
        let step = if index == -1 {
            mixture.react(context)
        } else {
            mixture.react_one(index as usize, context)
        };
        outcome.flags = step.flags;
        outcome.effects.extend(step.effects);
    }
    let results: Vec<String> = mixture
        .results
        .iter()
        .map(|r| r.map_or("null".into(), |n| n.to_string()))
        .collect();
    let effects: Vec<String> = outcome
        .effects
        .iter()
        .map(|e| {
            let (kind, a, b) = match *e {
                Effect::Freeze => (1, 0.0, 0.0),
                Effect::Wet => (2, 0.0, 0.0),
                Effect::Hotspot {
                    temperature,
                    volume,
                } => (3, temperature, volume),
                Effect::Radiation { range, threshold } => (4, range, threshold),
                Effect::HotIce => (5, 0.0, 0.0),
                Effect::RadiationChance { range, threshold } => (11, range, threshold),
                Effect::HotIceChance => (12, 0.0, 0.0),
                Effect::Resin { volume } => (6, volume, 0.0),
                Effect::NuclearParticle => (7, 0.0, 0.0),
                Effect::Hallucination { radius, duration } => (8, radius, duration),
            };
            format!("[{kind},{a},{b}]")
        })
        .collect();
    Ok(format!("{{\"initial_heat\":{},\"flags\":{},\"temperature\":{},\"moles\":{:?},\"keys\":{:?},\"results\":[{}],\"effects\":[{}]}}",
        initial_heat, outcome.flags, mixture.temperature, mixture.moles, mixture.keys(), results.join(","), effects.join(",")))
}

fn main() -> Result<(), String> {
    for line in io::stdin().lock().lines() {
        println!("{}", run(&line.map_err(|e| e.to_string())?)?);
    }
    Ok(())
}
