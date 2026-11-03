//! Differential-test protocol; not the production BYOND interface.
use bandastation_atmos::{catalog::GAS_COUNT, transport::Sample, Mixture};
use std::io::{self, BufRead};

fn mixture(words: &mut std::str::SplitWhitespace<'_>) -> Result<Mixture, String> {
    let mut air = Mixture::new(next(words)?, next(words)?, next(words)? != 0.0);
    let count = next(words)? as usize;
    if count > GAS_COUNT {
        return Err("Too many gases".into());
    }
    for _ in 0..count {
        let gas = next(words)? as usize;
        let amount = next(words)?;
        if gas >= GAS_COUNT || air.keys().contains(&gas) {
            return Err("Invalid gas".into());
        }
        air.set(gas, amount);
    }
    Ok(air)
}

fn next(words: &mut std::str::SplitWhitespace<'_>) -> Result<f32, String> {
    let value: f32 = words
        .next()
        .ok_or("Missing input")?
        .parse()
        .map_err(|_| "Invalid number")?;
    if !value.is_finite() {
        return Err("Nonfinite input".into());
    }
    Ok(value)
}

fn state(sample: &Sample) -> String {
    format!("{{\"temperature\":{},\"moles\":{:?},\"keys\":{:?},\"archived_temperature\":{},\"archived_moles\":{:?},\"archived_keys\":{:?}}}",
        sample.air.temperature, sample.air.moles, sample.air.keys(),
        sample.archived.temperature, sample.archived.moles, sample.archived.keys())
}

fn run(line: &str) -> Result<String, String> {
    let mut words = line.split_whitespace();
    let mode = next(&mut words)? as u8;
    let ticks = next(&mut words)? as usize;
    let repeats = next(&mut words)? as usize;
    if mode > 1 || !(1..=100).contains(&ticks) || !(1..=6).contains(&repeats) {
        return Err("Invalid operation".into());
    }
    let ca = next(&mut words)?;
    let cb = next(&mut words)?;
    let mut a = Sample::new(mixture(&mut words)?);
    let mut b = Sample::new(mixture(&mut words)?);
    if words.next().is_some() {
        return Err("Extra input".into());
    }
    let mut values = Vec::new();
    let mut states = Vec::new();
    for _ in 0..ticks {
        a.archive();
        b.archive();
        for _ in 0..repeats {
            let value = if mode == 0 {
                a.share(&mut b, ca, cb)
            } else {
                Some(a.temperature_share(&mut b, ca))
            };
            values.push(value.map_or("null".to_string(), |v| v.to_string()));
            states.push(format!(
                "{{\"a\":{},\"b\":{},\"last_share\":{}}}",
                state(&a),
                state(&b),
                a.last_share
            ));
        }
    }
    Ok(format!(
        "{{\"values\":[{}],\"states\":[{}]}}",
        values.join(","),
        states.join(",")
    ))
}

fn main() -> Result<(), String> {
    for line in io::stdin().lock().lines() {
        println!("{}", run(&line.map_err(|e| e.to_string())?)?);
    }
    Ok(())
}
