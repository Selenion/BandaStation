//! Pipe records are independent of the turf worker; no BYOND object is retained.
use super::*;
use crate::{pipenet::Handle, transport::Sample};

fn handle(values: &[f32]) -> Result<Handle, String> {
    if values.len() != 2 {
        return Err("Invalid pipe handle".into());
    }
    Ok(Handle {
        slot: integer(values[0])?,
        generation: integer(values[1])?,
    })
}

pub(super) unsafe fn port_handle(api: &Api, value: &Value) -> Result<Handle, String> {
    handle(&api.numbers(value)?)
}

pub(super) fn port_readings(sample: &Sample) -> [f32; 2] {
    [
        sample.air.temperature,
        if sample.air.volume == 0.0 {
            0.0
        } else {
            sample.air.pressure()
        },
    ]
}

pub(super) fn port_packets(sample: &Sample) -> (Vec<f32>, Vec<f32>) {
    let air = &sample.air;
    let mut gas = vec![0.0, air.temperature, air.volume, air.keys().len() as f32];
    for &key in air.keys() {
        gas.extend([key as f32, air.moles[key]]);
    }
    let count = gas.len();
    gas.push(0.0);
    for (reaction, amount) in air.results.iter().enumerate() {
        if let Some(amount) = amount {
            gas.extend([reaction as f32, *amount]);
        }
    }
    gas[count] = ((gas.len() - count - 1) / 2) as f32;
    let mut archive = vec![0.0; 12];
    archive[6] = sample.last_share;
    archive.extend([
        sample.archived.temperature,
        sample.archived.keys().len() as f32,
    ]);
    for &key in sample.archived.keys() {
        archive.extend([key as f32, sample.archived.moles[key]]);
    }
    (gas, archive)
}

fn sample(values: &[f32]) -> Result<Sample, String> {
    let (&last_share, data) = values.split_first().ok_or("Missing pipe sample")?;
    let (air, archived) = packet::pipe_full_mixture(data)?;
    Ok(Sample {
        air,
        archived,
        last_share,
        boundary: crate::transport::Boundary::Mutable,
    })
}

pub(super) unsafe fn dispatch(api: &Api, args: &[Value], store: &mut Store) -> Result<f32, String> {
    let opcode = api.integer(&args[0])?;
    match (opcode, args.len()) {
        (41, 2) => return Ok(1.0),
        (42, 4) => {
            let value = sample(&api.numbers(&args[2])?)?;
            let id = store.pipes.register(value).map_err(|e| format!("{e:?}"))?;
            if let Err(error) = api.output(&args[3], &[id.slot as f32, id.generation as f32]) {
                store.release_pipe(id).map_err(|e| format!("{e:?}"))?;
                return Err(error);
            }
        }
        (43, 4) => {
            let id = handle(&api.numbers(&args[2])?)?;
            let value = sample(&api.numbers(&args[3])?)?;
            store
                .pipes
                .replace(id, value)
                .map_err(|e| format!("{e:?}"))?;
        }
        (45, 4) => {
            let values = api.numbers(&args[2])?;
            if values.len() % 2 != 0 || values.len() > 131072 {
                return Err("Invalid pipe group".into());
            }
            let handles: Result<Vec<_>, _> = values.chunks_exact(2).map(handle).collect();
            let handles = handles?;
            // Reject an invalid output list before changing any native mixture.
            api.output(&args[3], &[])?;
            store
                .pipes
                .reconcile(&handles)
                .map_err(|e| format!("{e:?}"))?;
            let mut output = Vec::new();
            for &id in &handles {
                let air = &store.pipes.get(id).map_err(|e| format!("{e:?}"))?.air;
                output.extend([air.temperature, air.keys().len() as f32]);
                for &gas in air.keys() {
                    output.extend([gas as f32, air.moles[gas]]);
                }
            }
            api.output(&args[3], &output)?;
        }
        (46, 3) => {
            let id = handle(&api.numbers(&args[2])?)?;
            store.release_pipe(id).map_err(|e| format!("{e:?}"))?;
        }
        (47, 3) => api.output(
            &args[2],
            &[store.pipes.len() as f32, store.pipes.slots() as f32],
        )?,
        (48, 5) => super::pipe_direct::reconcile(api, &args[2..], &mut store.pipes)?,
        (49, 2) => {
            super::pipe_direct::available()?;
        }
        (68, 2) => super::pipe_direct::available()?,
        (68, 6) => super::pipe_direct::reconcile(api, &args[2..], &mut store.pipes)?,
        (69, 5) => {
            let id = port_handle(api, &args[2])?;
            let sample = store.pipes.get(id).map_err(|error| format!("{error:?}"))?;
            let (gas, archive) = port_packets(sample);
            api.output(&args[3], &gas)?;
            api.output(&args[4], &archive)?;
        }
        _ => return Err("Invalid pipe operation".into()),
    }
    Ok(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn read(_: *const Value, output: *mut Value, length: *mut u32) -> bool {
        let values = [1.0_f32, 1.0, 2.0, 1.0];
        *length = values.len() as u32;
        if output.is_null() {
            return false;
        }
        for (i, n) in values.into_iter().enumerate() {
            *output.add(i) = Value {
                kind: 1,
                padding: [0; 3],
                data: n.to_bits(),
            };
        }
        true
    }
    unsafe extern "C" fn reject(_: *const Value, _: *const Value, _: u32) -> bool {
        false
    }
    unsafe extern "C" fn reference(_: *const Value) {}
    unsafe extern "C" fn complete(_: *const Value, _: *const Value) -> bool {
        false
    }
    unsafe extern "C" fn error(_: *const c_char) {}

    #[test]
    fn invalid_output_does_not_change_pipe_gas() {
        let api = Api {
            read,
            write: reject,
            inc: reference,
            dec: reference,
            complete,
            error,
            num_kind: 1,
        };
        let mut store = Store::default();
        let a = store
            .pipes
            .register(Sample::new(crate::Mixture::new(300.0, 100.0, false)))
            .unwrap();
        let mut hot = crate::Mixture::new(600.0, 200.0, false);
        hot.set(0, 100.0);
        let b = store.pipes.register(Sample::new(hot)).unwrap();
        let before = (
            store.pipes.get(a).unwrap().clone(),
            store.pipes.get(b).unwrap().clone(),
        );
        let args = [
            api.number(45.0),
            api.number(1.0),
            Value::default(),
            Value::default(),
        ];
        assert!(unsafe { dispatch(&api, &args, &mut store) }.is_err());
        assert_eq!(store.pipes.get(a).unwrap(), &before.0);
        assert_eq!(store.pipes.get(b).unwrap(), &before.1);
    }
}
