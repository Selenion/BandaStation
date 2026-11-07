//! Synchronous pipe boundary: borrow DM lists once, without DM numeric packet loops.
//! No BYOND reference escapes the call or reaches the atmosphere worker.
use super::{
    load, packet,
    pipes::{port_handle, port_readings},
    Api, Value,
};
use crate::{
    catalog::{
        GAS_COUNT, MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND, MOLAR_ACCURACY, R_IDEAL_GAS_EQUATION,
    },
    cell::{merge, remove_ratio},
    phase::{Store, Write},
    pipenet::{Handle, Pool},
    transport::Boundary,
    Mixture,
};
use std::{ffi::c_char, sync::OnceLock};

#[path = "byond_scrubber.rs"]
mod scrubber;
pub(super) use scrubber::{
    finish_scrubber_tiles, take_scrubber_tile, transfer_scrubber, transfer_scrubber_tile,
};

type Read = unsafe extern "C" fn(*const Value, *mut Value, *mut u32) -> bool;
type ListRemove = unsafe extern "C" fn(*const Value, *const Value, u32) -> u32;

struct DirectApi {
    read_var: unsafe extern "C" fn(*const Value, u32, *mut Value) -> bool,
    write_var: unsafe extern "C" fn(*const Value, u32, *const Value) -> bool,
    read_assoc: Read,
    write_index: unsafe extern "C" fn(*const Value, *const Value, *const Value) -> bool,
    moles: u32,
    temperature: u32,
    volume: u32,
}

fn direct_api() -> Result<&'static DirectApi, String> {
    static API: OnceLock<Result<DirectApi, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let str_id: unsafe extern "C" fn(*const c_char) -> u32 = load(c"Byond_GetStrId")?;
        let fields = [
            str_id(c"moles".as_ptr()),
            str_id(c"temperature".as_ptr()),
            str_id(c"volume".as_ptr()),
        ];
        if fields.contains(&65535) {
            return Err("Missing pipe field names".into());
        }
        Ok(DirectApi {
            read_var: load(c"Byond_ReadVarByStrId")?,
            write_var: load(c"Byond_WriteVarByStrId")?,
            read_assoc: load(c"Byond_ReadListAssoc")?,
            write_index: load(c"Byond_WriteListIndex")?,
            moles: fields[0],
            temperature: fields[1],
            volume: fields[2],
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub(super) fn available() -> Result<(), String> {
    direct_api().map(|_| ())
}

fn vent_api() -> Result<(u32, ListRemove), String> {
    static API: OnceLock<Result<(u32, ListRemove), String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        available()?;
        let str_id: unsafe extern "C" fn(*const c_char) -> u32 = load(c"Byond_GetStrId")?;
        let archive = str_id(c"moles_archive".as_ptr());
        if archive == 65535 {
            return Err("Missing vent archive field".into());
        }
        Ok((archive, load(c"Byond_ListRemove")?))
    })
    .clone()
}

pub(super) fn vent_available() -> Result<(), String> {
    vent_api().map(|_| ())
}

/// BYOND 516.1674+ returns persistent references, including on the main thread.
struct Values<'a> {
    api: &'a Api,
    values: Vec<Value>,
}

impl Drop for Values<'_> {
    fn drop(&mut self) {
        for value in &self.values {
            unsafe { (self.api.dec)(value) };
        }
    }
}

unsafe fn read_values<'a>(
    api: &'a Api,
    read: Read,
    list: &Value,
    limit: u32,
) -> Result<Values<'a>, String> {
    let mut length = 0;
    let valid = read(list, std::ptr::null_mut(), &mut length);
    if (!valid && length == 0) || length > limit {
        return Err("Invalid direct pipe list".into());
    }
    let mut result = Values {
        api,
        values: vec![Value::default(); length as usize],
    };
    if !read(list, result.values.as_mut_ptr(), &mut length)
        || length as usize != result.values.len()
    {
        return Err("Cannot read direct pipe list".into());
    }
    Ok(result)
}

unsafe fn read_var<'a>(
    api: &'a Api,
    direct: &DirectApi,
    object: &Value,
    name: u32,
) -> Result<Values<'a>, String> {
    let mut result = Values {
        api,
        values: vec![Value::default()],
    };
    if !(direct.read_var)(object, name, &mut result.values[0]) {
        return Err("Cannot read pipe field".into());
    }
    Ok(result)
}

fn same(a: &Value, b: &Value) -> bool {
    a.kind == b.kind && a.data == b.data
}

struct Borrowed<'a> {
    list: Values<'a>,
    pairs: Values<'a>,
    temperature: f32,
    air: Mixture,
}

unsafe fn borrow<'a>(
    api: &'a Api,
    direct: &DirectApi,
    object: &Value,
    gases: &[Value],
) -> Result<Borrowed<'a>, String> {
    let temperature = api.num(&read_var(api, direct, object, direct.temperature)?.values[0])?;
    let volume = api.num(&read_var(api, direct, object, direct.volume)?.values[0])?;
    if volume < 0.0 {
        return Err("Invalid pipe volume".into());
    }
    let list = read_var(api, direct, object, direct.moles)?;
    let pairs = read_values(
        api,
        direct.read_assoc,
        &list.values[0],
        (GAS_COUNT * 2) as u32,
    )?;
    if pairs.values.len() % 2 != 0 {
        return Err("Invalid pipe gas pairs".into());
    }
    let mut air = Mixture::new(temperature, volume, false);
    for pair in pairs.values.chunks_exact(2) {
        let gas = gases
            .iter()
            .position(|g| same(g, &pair[0]))
            .ok_or("Unknown pipe gas")?;
        // full_packet treats a present null association as zero without changing its order.
        let amount = if pair[1].kind == 0 {
            0.0
        } else {
            api.num(&pair[1])?
        };
        if amount < 0.0 || air.keys().contains(&gas) {
            return Err("Invalid pipe gas".into());
        }
        air.set(gas, amount);
    }
    Ok(Borrowed {
        list,
        pairs,
        temperature,
        air,
    })
}

impl Borrowed<'_> {
    unsafe fn write_air(
        &self,
        api: &Api,
        direct: &DirectApi,
        object: &Value,
        gases: &[Value],
        air: &Mixture,
    ) -> Result<(), String> {
        let same_keys = air.keys() == self.air.keys();
        if !same_keys {
            let keys: Vec<_> = air.keys().iter().map(|&g| gases[g]).collect();
            if !(api.write)(&self.list.values[0], keys.as_ptr(), keys.len() as u32) {
                return Err("Cannot write pipe keys".into());
            }
        }
        for (i, &gas) in air.keys().iter().enumerate() {
            let value = api.number(air.moles[gas]);
            if (!same_keys || !same(&value, &self.pairs.values[i * 2 + 1]))
                && !(direct.write_index)(&self.list.values[0], &gases[gas], &value)
            {
                return Err("Cannot write pipe moles".into());
            }
        }
        if air.temperature.to_bits() != self.temperature.to_bits()
            && !(direct.write_var)(object, direct.temperature, &api.number(air.temperature))
        {
            return Err("Cannot write pipe temperature".into());
        }
        Ok(())
    }

    unsafe fn restore(&self, api: &Api, direct: &DirectApi, object: &Value) -> bool {
        let keys: Vec<_> = self
            .pairs
            .values
            .chunks_exact(2)
            .map(|pair| pair[0])
            .collect();
        let list = &self.list.values[0];
        (api.write)(list, keys.as_ptr(), keys.len() as u32)
            && self
                .pairs
                .values
                .chunks_exact(2)
                .all(|pair| (direct.write_index)(list, &pair[0], &pair[1]))
            && (direct.write_var)(object, direct.temperature, &api.number(self.temperature))
    }
}

/// Remove a rounded sample, limited by the available moles.
fn take(air: &mut Mixture, amount: f32) -> Option<Mixture> {
    let total = air.total_moles();
    let amount = amount.min(total);
    (amount > 0.0).then(|| remove_ratio(air, amount / total))
}

struct ArchiveTrim<'a> {
    list: Values<'a>,
    _pairs: Values<'a>,
    keys: Vec<Value>,
    remove: ListRemove,
}

impl ArchiveTrim<'_> {
    unsafe fn apply(self) {
        if !self.keys.is_empty() {
            (self.remove)(
                &self.list.values[0],
                self.keys.as_ptr(),
                self.keys.len() as u32,
            );
        }
    }
}

unsafe fn archive_trim<'a>(
    api: &'a Api,
    direct: &DirectApi,
    object: &Value,
) -> Result<Option<ArchiveTrim<'a>>, String> {
    let (archive_name, remove) = vent_api()?;
    let list = read_var(api, direct, object, archive_name)?;
    let pairs = read_values(
        api,
        direct.read_assoc,
        &list.values[0],
        (GAS_COUNT * 2) as u32,
    )?;
    let mut keys = Vec::new();
    for pair in pairs.values.chunks_exact(2) {
        let Ok(value) = api.num(&pair[1]) else {
            return Ok(None);
        };
        if value <= MOLAR_ACCURACY {
            keys.push(pair[0]);
        }
    }
    Ok(Some(ArchiveTrim {
        list,
        _pairs: pairs,
        keys,
        remove,
    }))
}

fn move_vent(port: &mut Mixture, tile: &mut Mixture, amount: f32, releasing: bool) -> (bool, bool) {
    let (source, destination) = if releasing {
        (port, tile)
    } else {
        (tile, port)
    };
    let sample = take(source, amount);
    let moved = sample.as_ref().is_some_and(|gas| gas.total_moles() != 0.0);
    if let Some(gas) = sample.as_ref().filter(|_| moved) {
        merge(destination, gas);
    }
    (moved, sample.is_some())
}

/// Move gas while the store is idle and the caller holds the current turf binding.
/// A positive cell ID releases from the port. A negative ID siphons into it.
pub(super) unsafe fn transfer_vent(
    api: &Api,
    store: &mut Store,
    args: &[Value],
) -> Result<f32, String> {
    const FALLBACK: f32 = 1.0;
    const EMPTY: f32 = 2.0;
    const MOVED: f32 = 3.0;
    let signed_id = api.num(&args[0])?;
    let id = super::integer(signed_id.abs())?;
    let releasing = signed_id > 0.0;
    let version = api.integer(&args[1])?;
    let amount = api.num(&args[4])?;
    let Some((air, archive)) = store.machine_air(id, version) else {
        return Ok(FALLBACK);
    };
    let Some(rules) = &store.visibility else {
        return Ok(FALLBACK);
    };
    let direct = direct_api()?;
    let object = &args[2];
    let gases = read_values(api, api.read, &args[3], GAS_COUNT as u32)?;
    if gases.values.len() != GAS_COUNT {
        return Err("Invalid vent gas catalog".into());
    }
    let Ok(before) = borrow(api, direct, object, &gases.values) else {
        return Ok(FALLBACK);
    };
    // Unsupported port contents retain the DM path, including explicit null associations.
    if before.air.volume <= 0.0 || before.pairs.values.chunks_exact(2).any(|p| p[1].kind == 0) {
        return Ok(FALLBACK);
    }
    let mut port = before.air.clone();
    let mut air = air.clone();
    let mut archive = archive.clone();
    let (moved, sampled) = move_vent(&mut port, &mut air, amount, releasing);
    let trim_archive = releasing && sampled;
    let trim = if trim_archive {
        let Some(trim) = archive_trim(api, direct, object)? else {
            return Ok(FALLBACK);
        };
        Some(trim)
    } else {
        if !releasing && sampled {
            archive.garbage_collect();
        }
        None
    };
    // A failed output write must leave both sides untouched. Suction refreshes the tile even if empty.
    let mut output = Vec::new();
    if !releasing || moved {
        output.extend(packet::air_properties(&air, &Boundary::Mutable));
        packet::append_visibility(&mut output, &air, rules);
    }
    api.output(&args[5], &output)?;
    if moved || trim_archive {
        if let Err(error) = before.write_air(api, direct, object, &gases.values, &port) {
            if !before.restore(api, direct, object) {
                return Err(format!("{error}. Cannot restore the vent port"));
            }
            return Err(error);
        }
    }
    if let Some(trim) = trim {
        trim.apply();
    }
    if !releasing || moved {
        // With the store lock held, the validated idle binding cannot change during the C API writes.
        if moved || sampled {
            store
                .write(Write::Commit(id, air, archive, false))
                .map_err(|e| format!("{e:?}"))?;
        }
        store
            .write(Write::Control(id, 0))
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok(if moved { MOVED } else { EMPTY })
}

pub(super) unsafe fn transfer_owned_vent(
    api: &Api,
    store: &mut Store,
    args: &[Value],
) -> Result<f32, String> {
    let signed_id = api.num(&args[0])?;
    let id = super::integer(signed_id.abs())?;
    let releasing = signed_id > 0.0;
    let version = api.integer(&args[1])?;
    let handle = port_handle(api, &args[2])?;
    let amount = api.num(&args[3])?;
    let (Some((tile, archive)), Some(rules)) = (store.machine_air(id, version), &store.visibility)
    else {
        return Ok(1.0);
    };
    let mut port = store
        .pipes
        .get(handle)
        .map_err(|error| format!("{error:?}"))?
        .clone();
    if port.air.volume <= 0.0 {
        return Ok(1.0);
    }
    let (mut tile, mut archive) = (tile.clone(), archive.clone());
    let (moved, sampled) = move_vent(&mut port.air, &mut tile, amount, releasing);
    if sampled {
        if releasing {
            port.archived.garbage_collect();
        } else {
            archive.garbage_collect();
        }
    }
    let mut output = port_readings(&port).to_vec();
    if !releasing || moved {
        output.extend(packet::air_properties(&tile, &Boundary::Mutable));
        packet::append_visibility(&mut output, &tile, rules);
    }
    api.output(&args[4], &output)?;
    if moved || (releasing && sampled) {
        store
            .pipes
            .replace(handle, port)
            .map_err(|error| format!("{error:?}"))?;
    }
    if !releasing || moved {
        if sampled {
            store
                .write(Write::Commit(id, tile, archive, false))
                .map_err(|error| format!("{error:?}"))?;
        }
        store
            .write(Write::Control(id, 0))
            .map_err(|error| format!("{error:?}"))?;
    }
    Ok(if moved { 3.0 } else { 2.0 })
}

/// The passive vent uses gas_mixture.equalize, including its cleanup on no transfer.
fn equalize_passive(port: &mut Mixture, tile: &mut Mixture) -> Option<bool> {
    let mut moved = false;
    if (port.temperature - tile.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND {
        let port_capacity = port.heat_capacity();
        let tile_capacity = tile.heat_capacity();
        let temperature = (port.temperature * port_capacity + tile.temperature * tile_capacity)
            / (port_capacity + tile_capacity);
        if !temperature.is_finite() {
            return None;
        }
        port.temperature = temperature;
        tile.temperature = temperature;
        moved = true;
    }
    let volume = port.volume + tile.volume;
    if port.temperature <= 0.0 || !volume.is_finite() {
        return None;
    }
    let mut gases = port.keys().to_vec();
    gases.extend(tile.keys().iter().filter(|gas| !port.keys().contains(gas)));
    for gas in gases {
        let left = port.moles[gas];
        let right = tile.moles[gas];
        if (left / port.volume - right / tile.volume).abs()
            > 0.1 / (R_IDEAL_GAS_EQUATION * port.temperature)
        {
            let total = left + right;
            if !total.is_finite() {
                return None;
            }
            port.set(gas, total * (port.volume / volume));
            tile.set(gas, total * (tile.volume / volume));
            moved = true;
        }
    }
    // assert_gas adds missing zero archive entries. The original collects them immediately.
    port.garbage_collect();
    tile.garbage_collect();
    Some(moved)
}

pub(super) unsafe fn transfer_passive_vent(
    api: &Api,
    store: &mut Store,
    args: &[Value],
) -> Result<f32, String> {
    const FALLBACK: f32 = 1.0;
    let id = api.integer(&args[0])?;
    let version = api.integer(&args[1])?;
    let Some((air, archive)) = store.machine_air(id, version) else {
        return Ok(FALLBACK);
    };
    let direct = direct_api()?;
    let object = &args[2];
    let gases = read_values(api, api.read, &args[3], GAS_COUNT as u32)?;
    let Ok(before) = borrow(api, direct, object, &gases.values) else {
        return Ok(FALLBACK);
    };
    if before.air.volume <= 0.0
        || air.volume <= 0.0
        || before
            .pairs
            .values
            .chunks_exact(2)
            .any(|pair| pair[1].kind == 0)
    {
        return Ok(FALLBACK);
    }
    let Some(trim) = archive_trim(api, direct, object)? else {
        return Ok(FALLBACK);
    };
    let mut port = before.air.clone();
    let mut tile = air.clone();
    let mut tile_archive = archive.clone();
    let Some(moved) = equalize_passive(&mut port, &mut tile) else {
        return Ok(FALLBACK);
    };
    tile_archive.garbage_collect();
    let tile_changed = &tile != air || !tile_archive.same_bits(archive);
    if let Err(error) = before.write_air(api, direct, object, &gases.values, &port) {
        if !before.restore(api, direct, object) {
            return Err(format!("{error}. Cannot restore the passive vent port"));
        }
        return Err(error);
    }
    trim.apply();
    if tile_changed {
        store
            .write(Write::Commit(id, tile, tile_archive, false))
            .map_err(|error| format!("{error:?}"))?;
    }
    Ok(if moved { 3.0 } else { 2.0 })
}

pub(super) unsafe fn transfer_owned_passive(
    api: &Api,
    store: &mut Store,
    args: &[Value],
) -> Result<f32, String> {
    let id = api.integer(&args[0])?;
    let version = api.integer(&args[1])?;
    let handle = port_handle(api, &args[2])?;
    let Some((tile, archive)) = store.machine_air(id, version) else {
        return Ok(1.0);
    };
    let mut port = store
        .pipes
        .get(handle)
        .map_err(|error| format!("{error:?}"))?
        .clone();
    if port.air.volume <= 0.0 || tile.volume <= 0.0 {
        return Ok(1.0);
    }
    let (mut air, mut archived) = (tile.clone(), archive.clone());
    let Some(moved) = equalize_passive(&mut port.air, &mut air) else {
        return Ok(1.0);
    };
    port.archived.garbage_collect();
    archived.garbage_collect();
    let tile_changed = &air != tile || !archived.same_bits(archive);
    api.output(&args[3], &port_readings(&port))?;
    store
        .pipes
        .replace(handle, port)
        .map_err(|error| format!("{error:?}"))?;
    if tile_changed {
        store
            .write(Write::Commit(id, air, archived, false))
            .map_err(|error| format!("{error:?}"))?;
    }
    Ok(if moved { 3.0 } else { 2.0 })
}

pub(super) unsafe fn reconcile(api: &Api, args: &[Value], pool: &mut Pool) -> Result<(), String> {
    reconcile_with(api, direct_api()?, args, pool)
}

unsafe fn reconcile_with(
    api: &Api,
    direct: &DirectApi,
    args: &[Value],
    pool: &mut Pool,
) -> Result<(), String> {
    let ids = api.numbers(&args[0])?;
    if ids.len() % 2 != 0 || ids.len() > 131072 {
        return Err("Invalid direct pipe handles".into());
    }
    let objects = read_values(api, api.read, &args[1], 65536)?;
    let gases = read_values(api, api.read, &args[2], GAS_COUNT as u32)?;
    if objects.values.len() * 2 != ids.len() || gases.values.len() != GAS_COUNT {
        return Err("Direct pipe group/catalog mismatch".into());
    }
    let catalog: std::collections::BTreeSet<_> =
        gases.values.iter().map(|g| (g.kind, g.data)).collect();
    if catalog.len() != GAS_COUNT
        || gases
            .values
            .iter()
            .any(|g| g.kind == 0 || g.kind == api.num_kind)
    {
        return Err("Invalid direct pipe catalog".into());
    }
    let mut seen_handles = std::collections::BTreeSet::new();
    let mut seen_objects = std::collections::BTreeSet::new();
    let mut handles = Vec::with_capacity(objects.values.len());
    let mut borrowed = Vec::with_capacity(objects.values.len());
    let readings = args.get(3);
    if let Some(output) = readings {
        api.output(output, &[])?;
    }
    let mut nonzero_volume = false;
    // Read/validate the whole group before changing either DM or native gas.
    for (pair, object) in ids.chunks_exact(2).zip(&objects.values) {
        let handle = Handle {
            slot: super::integer(pair[0])?,
            generation: super::integer(pair[1])?,
        };
        let native_owned = readings.is_some() && object.kind == 0;
        if !seen_handles.insert((handle.slot, handle.generation))
            || (!native_owned && !seen_objects.insert((object.kind, object.data)))
        {
            return Err("Duplicate direct pipe binding".into());
        }
        let sample = pool.get(handle).map_err(|e| format!("{e:?}"))?;
        handles.push(handle);
        if native_owned {
            nonzero_volume |= sample.air.volume != 0.0;
            borrowed.push(None);
        } else {
            let value = borrow(api, direct, object, &gases.values)?;
            nonzero_volume |= value.air.volume != 0.0;
            borrowed.push(Some(value));
        }
    }
    for (&id, value) in handles.iter().zip(&borrowed) {
        if let Some(value) = value {
            pool.import_air(id, value.air.clone())
                .map_err(|e| format!("{e:?}"))?;
        }
    }
    pool.reconcile(&handles).map_err(|e| format!("{e:?}"))?;
    // Original reconcile_air returns before copy_from_ratio when all port volumes are zero.
    let mut properties = Vec::new();
    for ((&id, object), before) in handles.iter().zip(&objects.values).zip(&borrowed) {
        let sample = pool.get(id).map_err(|e| format!("{e:?}"))?;
        if let Some(before) = before {
            if nonzero_volume {
                before.write_air(api, direct, object, &gases.values, &sample.air)?;
            }
        } else {
            properties.extend(super::pipes::port_readings(sample));
        }
    }
    if let Some(output) = readings {
        api.output(output, &properties)?;
    }
    Ok(())
}
