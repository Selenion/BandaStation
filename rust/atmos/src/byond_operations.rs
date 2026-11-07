//! Versioned operation packets contain intent and newly supplied gas, never a full turf mirror.
use super::*;
use crate::{
    operations::{Operation, PairOperation, Target},
    pipenet::Handle,
    Mixture,
};

fn target(values: &[f32]) -> Result<Target, String> {
    if values.len() != 3 {
        return Err("Invalid operation target".into());
    }
    let id = integer(values[1])?;
    let version = integer(values[2])?;
    match values[0] {
        0.0 => Ok(Target::Cell { id, version }),
        1.0 => Ok(Target::Port(Handle {
            slot: id,
            generation: version,
        })),
        _ => Err("Invalid operation target kind".into()),
    }
}

fn operation(code: u32, args: &[f32]) -> Result<Operation, String> {
    let arity = |length| {
        if args.len() == length {
            Ok(())
        } else {
            Err("Invalid gas operation arity".to_string())
        }
    };
    let value = match code {
        1 | 2 => {
            arity(1)?;
            let key = integer(args[0])? as usize;
            if code == 1 {
                Operation::Ensure(key)
            } else {
                Operation::Reset(key)
            }
        }
        3 | 4 => {
            arity(2)?;
            let key = integer(args[0])? as usize;
            if code == 3 {
                Operation::SetGas(key, args[1])
            } else {
                Operation::AdjustGas(key, args[1])
            }
        }
        5 => {
            arity(3)?;
            Operation::Convert {
                from: integer(args[0])? as usize,
                to: integer(args[1])? as usize,
                amount: args[2],
            }
        }
        6..=8 | 13 | 14 => {
            arity(1)?;
            match code {
                6 => Operation::SetTemperature(args[0]),
                7 => Operation::Heat(args[0]),
                8 => Operation::SetVolume(args[0]),
                13 => Operation::Take(args[0]),
                _ => Operation::TakeRatio(args[0]),
            }
        }
        9 | 10 => {
            arity(0)?;
            if code == 9 {
                Operation::Archive
            } else {
                Operation::Collect
            }
        }
        11 => Operation::Merge(Box::new(packet::mixture_with_zero_volume(args, true)?)),
        12 => {
            if args.len() < 2 || ![0.0, 1.0].contains(&args[0]) {
                return Err("Invalid copy operation".into());
            }
            Operation::Copy {
                clear_archive: args[0] == 1.0,
                ratio: args[1],
                source: Box::new(packet::mixture_with_zero_volume(&args[2..], true)?),
            }
        }
        15 | 16 => {
            arity(2)?;
            let gas = integer(args[0])? as usize;
            if code == 15 {
                Operation::TakeSpecific {
                    gas,
                    amount: args[1],
                }
            } else {
                Operation::TakeSpecificRatio {
                    gas,
                    ratio: args[1],
                }
            }
        }
        17 | 18 => {
            arity(3)?;
            let mut context = context(integer(args[0])?);
            context.working_power = args[1];
            context.supermatter_power = args[2];
            if code == 17 {
                Operation::React(context)
            } else {
                Operation::Electrolyze(context)
            }
        }
        19 => {
            if args.len() % 2 != 0 || args.len() > GAS_COUNT * 2 {
                return Err("Invalid multiple-gas adjustment".into());
            }
            Operation::AdjustMany(
                args.chunks_exact(2)
                    .map(|pair| Ok((integer(pair[0])? as usize, pair[1])))
                    .collect::<Result<_, String>>()?,
            )
        }
        20..=22 => {
            arity(0)?;
            match code {
                20 => Operation::ClearCurrent,
                21 => Operation::ClearArchive,
                _ => Operation::ClearResults,
            }
        }
        23 => {
            arity(2)?;
            Operation::SetArchivedGas(integer(args[0])? as usize, args[1])
        }
        24 => {
            arity(1)?;
            Operation::SetArchivedTemperature(args[0])
        }
        25 => {
            arity(3)?;
            if ![0.0, 1.0].contains(&args[1]) {
                return Err("Invalid result presence".into());
            }
            Operation::SetResult {
                index: integer(args[0])? as usize,
                value: if args[1] == 1.0 { Some(args[2]) } else { None },
            }
        }
        26 => {
            arity(1)?;
            Operation::SetLastShare(args[0])
        }
        _ => return Err("Unknown gas operation".into()),
    };
    value.validate().map_err(|e| format!("{e:?}"))?;
    Ok(value)
}

fn operations(values: &[f32]) -> Result<Vec<Operation>, String> {
    if values.len() < 2 || values[0] != 1.0 || values.len() > 16384 {
        return Err("Invalid gas operation packet".into());
    }
    let count = integer(values[1])? as usize;
    if count > 256 {
        return Err("Too many gas operations".into());
    }
    let mut cursor = 2;
    let mut commands = Vec::with_capacity(count);
    for _ in 0..count {
        let header = values
            .get(cursor..cursor + 2)
            .ok_or("Incomplete gas operation")?;
        let code = integer(header[0])?;
        let length = integer(header[1])? as usize;
        cursor += 2;
        let args = values
            .get(cursor..cursor + length)
            .ok_or("Incomplete gas operation arguments")?;
        commands.push(operation(code, args)?);
        cursor += length;
    }
    if cursor != values.len() {
        return Err("Trailing gas operation data".into());
    }
    Ok(commands)
}

fn append_removed(output: &mut Vec<f32>, air: Option<&Mixture>, kind: u8) {
    output.push(if air.is_some() { 1.0 } else { 0.0 });
    if let Some(air) = air {
        output.extend([
            air.temperature,
            air.volume,
            kind as f32,
            air.keys().len() as f32,
        ]);
        for &key in air.keys() {
            output.extend([key as f32, air.moles[key]]);
        }
    }
}

fn append_effect(output: &mut Vec<f32>, effect: &Effect) {
    let (code, a, b) = match *effect {
        Effect::Freeze => (1, 0.0, 0.0),
        Effect::Wet => (2, 0.0, 0.0),
        Effect::Hotspot {
            temperature,
            volume,
        } => (3, temperature, volume),
        Effect::Radiation { range, threshold } => (4, range, threshold),
        Effect::HotIce => (5, 0.0, 0.0),
        Effect::Resin { volume } => (6, volume, 0.0),
        Effect::NuclearParticle => (7, 0.0, 0.0),
        Effect::Hallucination { radius, duration } => (8, radius, duration),
        Effect::RadiationChance { range, threshold } => (11, range, threshold),
        Effect::HotIceChance => (12, 0.0, 0.0),
    };
    output.extend([code as f32, a, b]);
}

pub(super) unsafe fn dispatch(api: &Api, args: &[Value], store: &mut Store) -> Result<f32, String> {
    let code = api.integer(&args[0])?;
    match (code, args.len()) {
        (82, 2) => {}
        (83, 6) => {
            let destination = target(&api.numbers(&args[2])?)?;
            let commands = operations(&api.numbers(&args[3])?)?;
            let wake = api.num(&args[4])? != 0.0;
            api.output(&args[5], &[])?;
            let results = store
                .operate(destination, &commands, wake)
                .map_err(|e| format!("{e:?}"))?;
            let mut output = vec![1.0, results.len() as f32];
            for result in results {
                output.push(result.outcome.flags as f32);
                append_removed(&mut output, result.removed.as_ref(), result.removed_kind);
                let queued = matches!(destination, Target::Cell { .. });
                output.push(if queued {
                    0.0
                } else {
                    result.outcome.effects.len() as f32
                });
                if !queued {
                    for effect in result.outcome.effects {
                        append_effect(&mut output, &effect);
                    }
                }
            }
            api.output(&args[5], &output)?;
        }
        (84, 3) => match api.integer(&args[2])? {
            0 => store.begin_operations(),
            1 => store.finish_operations(false, false),
            2 => store.finish_operations(true, false),
            _ => return Err("Invalid operation transaction mode".into()),
        }
        .map_err(|e| format!("{e:?}"))?,
        (85, 5) => {
            let target = target(&api.numbers(&args[2])?)?;
            api.output(&args[3], &[])?;
            api.output(&args[4], &[])?;
            if let Some(sample) = store
                .take_operation_sample(target)
                .map_err(|e| format!("{e:?}"))?
            {
                let (air, archive) = pipes::port_packets(&sample);
                api.output(&args[3], &air)?;
                api.output(&args[4], &archive)?;
                return Ok(2.0);
            }
        }
        (86, 7) => {
            let source = target(&api.numbers(&args[2])?)?;
            let destination = target(&api.numbers(&args[3])?)?;
            let data = api.numbers(&args[4])?;
            let Some((&code, data)) = data.split_first() else {
                return Err("Missing pair operation".into());
            };
            let op = match code {
                0.0 => {
                    let commands = operations(data)?;
                    if commands.len() != 1 {
                        return Err("Transfer needs one selector".into());
                    }
                    PairOperation::Transfer(commands.into_iter().next().unwrap())
                }
                1.0 if data.len() == 2 => PairOperation::Share {
                    own: data[0],
                    other: data[1],
                },
                2.0 if data.len() == 2 => PairOperation::TemperatureShare {
                    coefficient: data[0],
                    other_capacity: data[1],
                },
                3.0 if data.is_empty() => PairOperation::Equalize,
                _ => return Err("Invalid pair operation".into()),
            };
            api.output(&args[6], &[])?;
            let result = store
                .operate_pair(source, destination, &op, api.num(&args[5])? != 0.0)
                .map_err(|e| format!("{e:?}"))?;
            let mut output = vec![
                1.0,
                if result.value.is_some() { 1.0 } else { 0.0 },
                result.value.unwrap_or(0.0),
            ];
            append_removed(&mut output, result.removed.as_ref(), result.removed_kind);
            api.output(&args[6], &output)?;
        }
        _ => return Err("Invalid gas operation call".into()),
    }
    Ok(1.0)
}
