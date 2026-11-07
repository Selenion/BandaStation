//! Main-thread writes stop before each DM presentation callback, preserving publication order.
use super::*;

struct Row {
    packet: Vec<f32>,
    air_end: usize,
    metadata: usize,
    baseline: Option<Arc<Baseline>>,
    visuals: Vec<(usize, u32)>,
    service: bool,
}
pub(super) struct Batch {
    rows: Vec<Row>,
    cursor: usize,
}

fn hotspot_packet(row: &Row) -> Vec<f32> {
    let mut output = row.packet[..8].to_vec();
    output[6] = 1.0;
    output[7] = 4.0;
    if row.air_end > 8 {
        output.extend_from_slice(&row.packet[row.air_end - 4..row.air_end]);
    } else {
        output.extend([0.0; 4]);
    }
    output.push(12.0);
    output.extend_from_slice(&row.packet[row.metadata..row.metadata + 12]);
    output
}

unsafe fn truth(api: &Api, object: &Value, field: u32) -> Result<bool, String> {
    Ok((direct()?.truth)(&read_var(api, object, field)?[0]))
}
unsafe fn number(api: &Api, object: &Value, field: u32) -> Result<f32, String> {
    api.num(&read_var(api, object, field)?[0])
}
unsafe fn put(api: &Api, object: &Value, field: u32, value: f32) -> Result<(), String> {
    write_var(object, field, &api.number(value))
}
unsafe fn increment(api: &Api, object: &Value, field: u32) -> Result<(), String> {
    put(api, object, field, number(api, object, field)? + 1.0)
}
unsafe fn replace(api: &Api, air: &Value, field: u32, pairs: &[Value]) -> Result<(), String> {
    let list = new_list(api)?;
    write_pairs(api, &list[0], pairs)?;
    write_var(air, field, &list[0])
}
fn pairs(api: &Api, data: &[f32], keys: &[Value]) -> Result<Vec<Value>, String> {
    let mut values = Vec::with_capacity(data.len());
    for pair in data.chunks_exact(2) {
        values.push(
            *keys
                .get(pair[0] as usize)
                .ok_or("Unknown publication key")?,
        );
        values.push(api.number(pair[1]));
    }
    Ok(values)
}
unsafe fn copy_readonly(api: &Api, destination: &Value, source: &Value) -> Result<(), String> {
    let f = &direct()?.fields;
    let contents = capture(api, source)?;
    let moles = read_var(api, destination, f.moles)?;
    write_pairs(api, &moles[0], &contents.lists[0])?;
    for (field, value) in [f.temperature, f.temperature_archived, f.volume]
        .into_iter()
        .zip(contents.scalars.iter())
    {
        write_var(destination, field, value)?;
    }
    replace(api, destination, f.moles_archive, &contents.lists[1])?;
    replace(api, destination, f.reaction_results, &contents.lists[2])?;
    write_var(
        destination,
        f.last_share,
        &read_var(api, source, f.last_share)?[0],
    )?;
    put(api, destination, f.banda_readonly, 1.0)
}

unsafe fn cache(
    api: &Api,
    owner: u32,
    phase: &Value,
    cell: &Value,
    row: &Row,
    gases: &[Value],
    reactions: &[Value],
) -> Result<(), String> {
    let f = &direct()?.fields;
    let air = read_var(api, cell, f.cached_air)?;
    let metadata = &row.packet[row.metadata..];
    let input = &row.packet[8..row.air_end];
    let baseline = row.baseline.as_ref().filter(|_| row.packet[6] == 2.0);
    if let Some(baseline) = baseline {
        let moles = read_var(api, &air[0], f.moles)?;
        write_pairs(api, &moles[0], &baseline.lists[0])?;
        replace(api, &air[0], f.reaction_results, &baseline.lists[2])?;
        put(api, &air[0], f.temperature, input[1])?;
        write_var(&air[0], f.volume, &baseline.scalars[2])?;
        if metadata.len() > 12 {
            put(api, &air[0], f.temperature_archived, metadata[12])?;
            replace(
                api,
                &air[0],
                f.moles_archive,
                &pairs(api, &metadata[14..], gases)?,
            )?;
        } else {
            write_var(&air[0], f.temperature_archived, &baseline.scalars[1])?;
            replace(api, &air[0], f.moles_archive, &baseline.lists[1])?;
        }
    } else if metadata.len() > 12 {
        let count = input[3] as usize;
        let moles = read_var(api, &air[0], f.moles)?;
        write_pairs(
            api,
            &moles[0],
            &pairs(api, &input[4..4 + count * 2], gases)?,
        )?;
        let results = 5 + count * 2;
        replace(
            api,
            &air[0],
            f.reaction_results,
            &pairs(
                api,
                &input[results..results + input[results - 1] as usize * 2],
                reactions,
            )?,
        )?;
        put(api, &air[0], f.temperature, input[1])?;
        put(api, &air[0], f.volume, input[2])?;
        put(api, &air[0], f.temperature_archived, metadata[12])?;
        replace(
            api,
            &air[0],
            f.moles_archive,
            &pairs(api, &metadata[14..], gases)?,
        )?;
    } else {
        for field in [f.moles_archive, f.reaction_results] {
            let list = read_var(api, &air[0], field)?;
            replace(api, &air[0], field, &read_values(api, &list[0], true)?)?;
        }
    }
    put(api, &air[0], f.last_share, metadata[6])?;
    if baseline.is_some() || metadata.len() > 12 {
        if baseline.is_none() && (direct()?.is_type)(&air[0], c"/datum/gas_mixture/turf".as_ptr()) {
            write_var(&air[0], f.banda_cell, cell)?;
            write_var(
                &air[0],
                f.banda_version,
                &read_var(api, cell, f.binding_version)?[0],
            )?;
        }
        remember(api, owner, row.packet[0] as u32, &air[0])?;
        put(api, cell, f.committed_air, 1.0)?;
        put(api, cell, f.committed_from_native, 1.0)?;
        put(api, cell, f.cache_current, 1.0)?;
    }
    write_var(cell, f.cache_cycle, &read_var(api, phase, f.air_cycle)?[0])?;
    let readonly = read_var(api, cell, f.readonly_air)?;
    if (direct()?.truth)(&readonly[0]) {
        copy_readonly(api, &readonly[0], &air[0])?;
    }
    Ok(())
}

unsafe fn presentation_changed(
    api: &Api,
    target: &Value,
    row: &Row,
    gases: &[Value],
    overlays: &Value,
    mapping: &Value,
) -> Result<bool, String> {
    let f = &direct()?.fields;
    let current = read_var(api, target, f.atmos_overlay_types)?;
    // DM length(null) is zero; cold turfs do not necessarily own an overlay list.
    if !(direct()?.truth)(&current[0]) {
        return Ok(!row.visuals.is_empty());
    }
    let current = read_values(api, &current[0], false)?;
    if row.visuals.len() != current.len() {
        return Ok(true);
    }
    if current.is_empty() {
        return Ok(false);
    }
    let offset = if truth(api, mapping, f.max_plane_offset)? {
        let offsets = read_var(api, mapping, f.z_level_to_plane_offset)?;
        let z = read_var(api, target, f.z)?;
        api.num(&read_index(api, &offsets[0], &z[0])?[0])?
    } else {
        0.0
    };
    for ((gas, state), current) in row.visuals.iter().zip(current.iter()) {
        let planes = read_index(api, overlays, &gases[*gas])?;
        let states = read_index(api, &planes[0], &api.number(offset + 1.0))?;
        let image = read_index(api, &states[0], &api.number(*state as f32))?;
        if !(direct()?.equal)(current, &image[0]) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 1: complete; 2: bind; 3: changed overlay; 4: legacy refresh; 5: hotspot values only.
unsafe fn step(
    api: &Api,
    owner: u32,
    phase: &Value,
    context: &[Value],
    output: &Value,
    batch: &mut Batch,
) -> Result<f32, String> {
    let d = direct()?;
    let f = &d.fields;
    let gases = read_values(api, &context[1], false)?;
    let reactions = read_values(api, &context[2], false)?;
    while let Some(row) = batch.rows.get(batch.cursor) {
        let id = api.number(row.packet[0]);
        let target = read_index(api, &context[0], &id)?;
        for (field, value) in [f.x, f.y, f.z].into_iter().zip(&row.packet[2..5]) {
            if !(d.equal)(&read_var(api, &target[0], field)?[0], &api.number(*value)) {
                return Err("Native atmosphere coordinate registry mismatch".into());
            }
        }
        let cell = read_var(api, &target[0], f.banda_atmos_cell)?;
        if !(d.truth)(&cell[0]) {
            api.output(output, &[row.packet[0]])?;
            return Ok(2.0);
        }
        if number(api, &cell[0], f.binding_version)? != row.packet[1]
            || truth(api, &cell[0], f.pending_rebind)?
            || truth(api, &cell[0], f.gc_destroyed)?
        {
            batch.cursor += 1;
            continue;
        }
        put(api, &cell[0], f.native_air_kind, row.packet[5])?;
        let open = (d.is_type)(&target[0], c"/turf/open".as_ptr());
        let has_air = open
            && (truth(api, &target[0], f.air)?
                || (truth(api, &target[0], f.banda_atmos_id)?
                    && truth(api, &target[0], f.banda_air_present)?
                    && (number(api, &target[0], f.flags_1)? as u32 & api.integer(&context[5])?
                        != 0)));
        let cached = read_var(api, &cell[0], f.cached_air)?;
        let readonly = read_var(api, &cell[0], f.readonly_air)?;
        let has_cache = (d.truth)(&cached[0]) || (d.truth)(&readonly[0]);
        // Constructors and late mutations keep the existing DM path and its exact call order.
        let invalid_service =
            if has_air && has_cache && row.packet[row.air_end] == 12.0 && row.baseline.is_none() {
                match baseline(owner, row.packet[0] as u32)? {
                    Some(previous) if (d.truth)(&cached[0]) => {
                        !truth(api, &cell[0], f.cache_current)?
                            || !matches(api, &cached[0], &previous)?
                    }
                    _ => true,
                }
            } else {
                false
            };
        if has_air
            && has_cache
            && (!(d.truth)(&cached[0]) || row.packet[5] == 3.0 || invalid_service)
        {
            batch.cursor += 1;
            api.output(output, &row.packet)?;
            return Ok(4.0);
        }
        let metadata = &row.packet[row.metadata..];
        let cycle_offset = number(api, phase, f.source_cycle_offset)?;
        let archived = if metadata[2] != 0.0 {
            metadata[2] + cycle_offset
        } else {
            0.0
        };
        let current = if metadata[3] != 0.0 {
            metadata[3] + cycle_offset
        } else {
            0.0
        };
        let updates = [
            (f.temperature, metadata[0]),
            (f.temperature_archived, metadata[1]),
            (f.archived_cycle, archived),
            (f.current_cycle, current),
            (f.significant_share_ticker, metadata[4]),
            (f.excited, metadata[5]),
            (f.pressure_difference, metadata[7]),
            (f.pressure_direction, metadata[8]),
        ];
        let pending = has_air
            && (truth(api, &target[0], f.active_hotspot)?
                || truth(api, &cell[0], f.hotspots_pending)?);
        let mut duplicate = row.service
            && row.packet[6] != 0.0
            && row.air_end == 8
            && !has_cache
            && has_air
            && !pending;
        if duplicate {
            duplicate =
                !presentation_changed(api, &target[0], row, &gases, &context[3], &context[4])?;
            for (field, value) in updates {
                duplicate &= (d.equal)(&read_var(api, &target[0], field)?[0], &api.number(value));
            }
        }
        if duplicate {
            increment(api, phase, f.service_publications_skipped)?;
        } else {
            for (field, value) in updates.into_iter().take(4) {
                put(api, &target[0], field, value)?;
            }
            if has_air {
                if has_cache {
                    cache(api, owner, phase, &cell[0], row, &gases, &reactions)?;
                }
                for (field, value) in updates.into_iter().skip(4) {
                    put(api, &target[0], field, value)?;
                }
            }
        }
        increment(api, phase, f.direct_publications)?;
        if row.packet[6] != 0.0 {
            increment(api, phase, f.visual_publications)?;
        }
        batch.cursor += 1;
        // Match sync_air: cache replacement precedes comparison of actual image references.
        let changed = has_air
            && !duplicate
            && presentation_changed(api, &target[0], row, &gases, &context[3], &context[4])?;
        let pending = has_air
            && (truth(api, &target[0], f.active_hotspot)?
                || truth(api, &cell[0], f.hotspots_pending)?);
        let hotspot = row.air_end > 8 && row.packet[row.air_end - 4] != 0.0;
        if changed {
            api.output(output, &row.packet)?;
            return Ok(3.0);
        }
        if has_air && (pending || hotspot) {
            // No gas decoder or second overlay comparison is needed just to refresh a hotspot.
            api.output(output, &hotspot_packet(row))?;
            return Ok(5.0);
        }
    }
    api.output(output, &[])?;
    Ok(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_callback_omits_gas_and_archive_but_preserves_lifecycle_values() {
        let mut packet = vec![1.0, 7.0, 10.0, 20.0, 2.0, 2.0, 0.0, 9.0];
        packet.extend([123.0, 500.0, 2500.0, 0.0, 0.0, 1.0, 900.0, 2000.0, 17.0]);
        packet.push(16.0);
        packet.extend((1..=16_u32).map(|value| value as f32));
        let row = Row {
            packet,
            air_end: 17,
            metadata: 18,
            baseline: None,
            visuals: Vec::new(),
            service: false,
        };
        let output = hotspot_packet(&row);
        assert_eq!(output.len(), 25);
        assert_eq!(&output[..6], &row.packet[..6]);
        assert_eq!(&output[6..13], &[1.0, 4.0, 1.0, 900.0, 2000.0, 17.0, 12.0]);
        assert_eq!(&output[13..], &row.packet[18..30]);
        let row = Row {
            packet: [&row.packet[..8], &[12.0], &row.packet[18..30]].concat(),
            air_end: 8,
            metadata: 9,
            baseline: None,
            visuals: Vec::new(),
            service: true,
        };
        assert_eq!(&hotspot_packet(&row)[8..12], &[0.0; 4]);
    }
}

pub(super) unsafe fn dispatch(
    api: &Api,
    owner: u32,
    opcode: u32,
    args: &[Value],
) -> Result<f32, String> {
    match (opcode, args.len()) {
        (75, 3) => {
            let requests = api.numbers(&args[2])?;
            if requests.len() > 256 * 3 || requests.len() % 3 != 0 {
                return Err("Invalid direct publication requests".into());
            }
            let pairs: Vec<_> = requests
                .chunks_exact(3)
                .flat_map(|r| [r[0], r[1]])
                .collect();
            let engine = super::super::engine(owner)?;
            let store = engine.lock().map_err(|_| "Atmosphere engine poisoned")?;
            let packet = super::super::world::publication(&store, &pairs, 73)?;
            let rules = store
                .visibility
                .as_ref()
                .ok_or("Missing gas visibility rules")?;
            let mut rows = Vec::new();
            let mut offset = 0;
            for request in requests.chunks_exact(3) {
                let end = offset + 8 + packet[offset + 7] as usize;
                let next = end + 1 + packet[end] as usize;
                let data = packet[offset..next].to_vec();
                let mut visuals = Vec::new();
                if end > offset + 8 {
                    for pair in data[12..12 + data[11] as usize * 2].chunks_exact(2) {
                        let gas = pair[0] as usize;
                        let state = if data[6] != 0.0 {
                            Some(pair[1] as u32)
                        } else {
                            rules.state(gas, pair[1])
                        };
                        if let Some(state) = state {
                            visuals.push((gas, state));
                        }
                    }
                }
                rows.push(Row {
                    baseline: if request[1] >= 2.0 {
                        baseline(owner, request[0] as u32)?
                    } else {
                        None
                    },
                    packet: data,
                    air_end: end - offset,
                    metadata: end - offset + 1,
                    visuals,
                    service: request[2] != 0.0,
                });
                offset = next;
            }
            drop(store);
            let mut state = state().lock().map_err(|_| "Projection cache poisoned")?;
            if state.next_batch >= 16_777_215 {
                return Err("Direct publication handles exhausted".into());
            }
            state.next_batch += 1;
            let id = state.next_batch;
            state.batches.insert((owner, id), Batch { rows, cursor: 0 });
            Ok(id as f32)
        }
        (76, 6) => {
            let id = api.integer(&args[2])?;
            let mut batch = {
                state()
                    .lock()
                    .map_err(|_| "Projection cache poisoned")?
                    .batches
                    .remove(&(owner, id))
                    .ok_or("Missing publication batch")?
            };
            let context = read_values(api, &args[4], false)?;
            if context.len() != 6 {
                return Err("Invalid publication context".into());
            }
            let status = step(api, owner, &args[3], &context, &args[5], &mut batch)?;
            if status != 1.0 {
                state()
                    .lock()
                    .map_err(|_| "Projection cache poisoned")?
                    .batches
                    .insert((owner, id), batch);
            }
            Ok(status)
        }
        (77, 3) => {
            let batch = {
                state()
                    .lock()
                    .map_err(|_| "Projection cache poisoned")?
                    .batches
                    .remove(&(owner, api.integer(&args[2])?))
            };
            drop(batch);
            Ok(1.0)
        }
        _ => Err("Invalid direct publication operation".into()),
    }
}
