use super::{
    archive_trim, borrow, direct_api, merge, packet, read_values, same, take, Api, Store, Value,
    Write,
};
use crate::byond::pipes::{port_handle, port_readings};
use crate::{catalog::GAS_COUNT, quantize, Mixture, CELL_VOLUME, MOLAR_ACCURACY};

// Keep the cutoff tied to MINIMUM_MOLES_TO_SCRUB in vent_scrubber.dm.
const MINIMUM_MOLES_TO_SCRUB: f32 = MOLAR_ACCURACY * 100.0;

/// Preserve the order of the DM intersection: it affects both the sum and the sample's gas keys.
fn scrub(air: &mut Mixture, selected: Option<&[usize]>, rate: f32) -> (f32, Option<Mixture>) {
    let ratio = rate / air.volume;
    let Some(selected) = selected else {
        let sample = take(air, air.total_moles() * ratio);
        let moved = sample.as_ref().is_some_and(|gas| gas.total_moles() != 0.0);
        return (if moved { 3.0 } else { 5.0 }, sample);
    };
    if selected.is_empty() {
        return (2.0, None);
    }
    let total: f32 = selected.iter().map(|&gas| air.moles[gas]).sum();
    let sample = (total != 0.0).then(|| {
        let mut filtered = Mixture::new(air.temperature, CELL_VOLUME, false);
        for &gas in selected {
            let amount = air.moles[gas];
            let transferred = quantize(amount * ratio.min(1.0) * (amount / total))
                .max(MINIMUM_MOLES_TO_SCRUB.min(amount));
            filtered.set(gas, transferred);
            air.moles[gas] -= transferred;
        }
        filtered
    });
    air.garbage_collect();
    let status = match &sample {
        None => 4.0,
        Some(gas) if gas.total_moles() != 0.0 => 3.0,
        Some(_) => 5.0,
    };
    (status, sample)
}

unsafe fn select_gases(
    api: &Api,
    filter_paths: &Value,
    gases: &[Value],
    air: &Mixture,
    selected: &mut [usize; GAS_COUNT],
) -> Result<usize, String> {
    let filters = read_values(api, api.read, filter_paths, GAS_COUNT as u32)?;
    let mut len = 0;
    for filter in &filters.values {
        let gas = gases
            .iter()
            .position(|path| same(filter, path))
            .ok_or("Unknown scrubber filter")?;
        if air.keys().contains(&gas) && !selected[..len].contains(&gas) {
            selected[len] = gas;
            len += 1;
        }
    }
    Ok(len)
}

/// Exchange between the two live mixtures already owned by the exposure callback.
/// 1 requests DM dispatch, 2 wakes the tile, 3 also updates parents after moving gas,
/// 4 only cleans the archive/current gas keys, 5 updates parents after an empty purge.
pub(in crate::byond) unsafe fn transfer_scrubber(api: &Api, args: &[Value]) -> Result<f32, String> {
    let [tile, port, gas_paths, filter_paths, rate, turf_capacity] = args else {
        return Err("Invalid scrubber arguments".into());
    };
    let rate = api.num(rate)?;
    let turf_capacity = match api.integer(turf_capacity)? {
        0 => false,
        1 => true,
        _ => return Err("Invalid scrubber mixture kind".into()),
    };
    let direct = direct_api()?;
    let gases = read_values(api, api.read, gas_paths, GAS_COUNT as u32)?;
    if gases.values.len() != GAS_COUNT {
        return Err("Invalid scrubber gas catalog".into());
    }
    let Ok(source) = borrow(api, direct, tile, &gases.values) else {
        return Ok(1.0);
    };
    if source.air.volume <= 0.0 || source.pairs.values.chunks_exact(2).any(|p| p[1].kind == 0) {
        return Ok(1.0);
    }
    let mut air = source.air.clone();
    air.turf_heat_capacity = turf_capacity;
    let purging = filter_paths.kind == 0;
    let mut selected = [0; GAS_COUNT];
    let filters = if purging {
        None
    } else {
        let Ok(len) = select_gases(api, filter_paths, &gases.values, &air, &mut selected) else {
            return Ok(1.0);
        };
        Some(&selected[..len])
    };
    let (status, sample) = scrub(&mut air, filters, rate);
    if status == 2.0 || (purging && sample.is_none()) {
        return Ok(status);
    }

    let Some(trim) = archive_trim(api, direct, tile)? else {
        return Ok(1.0);
    };
    let mut destination = None;
    if let Some(sample) = sample {
        let Ok(before) = borrow(api, direct, port, &gases.values) else {
            return Ok(1.0);
        };
        if same(&before.list.values[0], &source.list.values[0])
            || same(&before.list.values[0], &trim.list.values[0])
            || before.pairs.values.chunks_exact(2).any(|p| p[1].kind == 0)
        {
            return Ok(1.0);
        }
        // Unlike a vent, a scrubber merges even a rounded-to-zero purge sample.
        let mut merged = before.air.clone();
        merge(&mut merged, &sample);
        destination = Some((before, merged));
    }

    if let Err(error) = source.write_air(api, direct, tile, &gases.values, &air) {
        if !source.restore(api, direct, tile) {
            return Err(format!("{error}. Cannot restore scrubber tile air"));
        }
        return Err(error);
    }
    if let Some((before, merged)) = destination {
        if let Err(error) = before.write_air(api, direct, port, &gases.values, &merged) {
            let tile_restored = source.restore(api, direct, tile);
            let port_restored = before.restore(api, direct, port);
            if !tile_restored || !port_restored {
                return Err(format!("{error}. Cannot restore scrubber mixtures"));
            }
            return Err(error);
        }
    }
    trim.apply();
    Ok(status)
}

/// Edit Rust-owned air within the callback. The live port is visible to the next tile immediately.
pub(in crate::byond) unsafe fn transfer_scrubber_tile(
    api: &Api,
    store: &mut Store,
    args: &[Value],
    owned: bool,
) -> Result<f32, String> {
    let [binding, port, gas_paths, filter_paths, rate, visuals] = args else {
        return Err("Invalid native scrubber tile arguments".into());
    };
    let binding = api.numbers(binding)?;
    let [id, version] = binding[..] else {
        return Err("Invalid native scrubber binding".into());
    };
    let id = crate::byond::integer(id)?;
    let version = crate::byond::integer(version)?;
    let rate = api.num(rate)?;
    let Some((source, archive)) = store.scrubber_air(id, version) else {
        return Ok(1.0);
    };
    if source.volume <= 0.0 {
        return Ok(1.0);
    }
    let gases = read_values(api, api.read, gas_paths, GAS_COUNT as u32)?;
    if gases.values.len() != GAS_COUNT {
        return Err("Invalid scrubber gas catalog".into());
    }
    let mut selected = [0; GAS_COUNT];
    let purging = filter_paths.kind == 0;
    let filters = if purging {
        None
    } else {
        let Ok(len) = select_gases(api, filter_paths, &gases.values, source, &mut selected) else {
            return Ok(1.0);
        };
        if len == 0 {
            return Ok(2.0);
        }
        Some(&selected[..len])
    };
    let mut air = source.clone();
    let (status, sample) = scrub(&mut air, filters, rate);
    let mut archive = archive.clone();
    let edited = !purging || sample.is_some();
    if edited {
        archive.garbage_collect();
    }

    let mut port_update = if owned {
        let handle = port_handle(api, port)?;
        let mut state = store
            .pipes
            .get(handle)
            .map_err(|error| format!("{error:?}"))?
            .clone();
        if let Some(gas) = &sample {
            merge(&mut state.air, gas);
        }
        Some((handle, state))
    } else {
        None
    };
    let mut output = Vec::new();
    if let Some((_, state)) = &port_update {
        output.extend(port_readings(state));
    }
    if purging {
        let Some(rules) = &store.visibility else {
            return Ok(1.0);
        };
        output.extend([0.0; 4]); // The same visibility packet shape as the vent path.
        packet::append_visibility(&mut output, &air, rules);
    }
    if owned || purging {
        api.output(visuals, &output)?;
    }
    if let Some(sample) = sample {
        if let Some((handle, state)) = port_update.take() {
            store
                .pipes
                .replace(handle, state)
                .map_err(|error| format!("{error:?}"))?;
        } else {
            let direct = direct_api()?;
            let Ok(before) = borrow(api, direct, port, &gases.values) else {
                return Ok(1.0);
            };
            if before.pairs.values.chunks_exact(2).any(|p| p[1].kind == 0) {
                return Ok(1.0);
            }
            // A non-null rounded-to-zero purge still merges and may change port temperature.
            let mut merged = before.air.clone();
            merge(&mut merged, &sample);
            if let Err(error) = before.write_air(api, direct, port, &gases.values, &merged) {
                if !before.restore(api, direct, port) {
                    return Err(format!("{error}. Cannot restore the scrubber port"));
                }
                return Err(error);
            }
        }
    }
    if edited {
        let entry = (id, version, air, archive);
        if let Some(previous) = store.scrubber_edits.iter_mut().find(|edit| edit.0 == id) {
            *previous = entry;
        } else {
            store.scrubber_edits.push(entry);
        }
    }
    Ok(status)
}

pub(in crate::byond) fn finish_scrubber_tiles(
    store: &mut Store,
    commit: bool,
) -> Result<(), String> {
    let mut edits = std::mem::take(&mut store.scrubber_edits);
    let result = if commit {
        edits.drain(..).try_for_each(|(id, version, air, archive)| {
            if store.machine_air(id, version).is_some() {
                store
                    .write(Write::Commit(id, air, archive, false))
                    .map_err(|e| format!("{e:?}"))?;
            }
            Ok(())
        })
    } else {
        edits.clear();
        Ok(())
    };
    store.scrubber_edits = edits;
    result
}

/// A later ordinary consumer in the same callback takes over the edit as a working DM mixture.
pub(in crate::byond) unsafe fn take_scrubber_tile(
    api: &Api,
    store: &mut Store,
    args: &[Value],
) -> Result<f32, String> {
    let [id, version, input, metadata] = args else {
        return Err("Invalid scrubber materialization arguments".into());
    };
    let id = api.integer(id)?;
    let version = api.integer(version)?;
    let Some(index) = store
        .scrubber_edits
        .iter()
        .position(|edit| edit.0 == id && edit.1 == version)
    else {
        return Ok(1.0);
    };
    if store.machine_air(id, version).is_none() {
        return Ok(1.0);
    }
    let (_, _, air, archive) = &store.scrubber_edits[index];
    let frame = store.snapshot();
    api.output(
        input,
        &packet::air_packet_with(&frame, &frame.cells[&id], air, true, true),
    )?;
    api.output(
        metadata,
        &packet::turf_packet_with(&frame, id, &frame.schedule.tiles[&id], Some(archive)),
    )?;
    store.scrubber_edits.remove(index);
    Ok(2.0)
}
