//! Small BYOND 516 C API adapter. Only the completion notification touches BYOND on a worker.
//! Worker state is numeric; main-thread projection separately owns DM cache baselines.
use crate::{
    catalog::*,
    cell::{Cell, WorldAction},
    phase::{Store, Write},
    Effect,
};
#[path = "byond_operations.rs"]
mod operations;
#[path = "byond_packets.rs"]
mod packet;
#[path = "byond_pipe_direct.rs"]
mod pipe_direct;
#[path = "byond_pipes.rs"]
mod pipes;
#[path = "byond_projection.rs"]
mod projection;
#[path = "byond_worker.rs"]
mod worker;
#[path = "byond_world.rs"]
mod world;
use packet::{context, full_mixture, integer, mixture, terrain, tile};
use worker::Engine;

use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex, OnceLock},
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Value {
    kind: u8,
    padding: [u8; 3],
    data: u32,
}

struct Api {
    read: unsafe extern "C" fn(*const Value, *mut Value, *mut u32) -> bool,
    write: unsafe extern "C" fn(*const Value, *const Value, u32) -> bool,
    inc: unsafe extern "C" fn(*const Value),
    dec: unsafe extern "C" fn(*const Value),
    complete: unsafe extern "C" fn(*const Value, *const Value) -> bool,
    error: unsafe extern "C" fn(*const c_char),
    num_kind: u8,
}

#[cfg(windows)]
unsafe fn symbol(name: *const c_char) -> *mut c_void {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    GetProcAddress(GetModuleHandleA(c"byondcore.dll".as_ptr()), name)
}

#[cfg(unix)]
unsafe fn symbol(name: *const c_char) -> *mut c_void {
    #[link(name = "dl")]
    extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    dlsym(std::ptr::null_mut(), name)
}

unsafe fn load<T: Copy>(name: &'static std::ffi::CStr) -> Result<T, String> {
    let address = symbol(name.as_ptr());
    if address.is_null() {
        return Err(format!("Missing BYOND API {}", name.to_string_lossy()));
    }
    Ok(std::mem::transmute_copy(&address))
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let set_num: unsafe extern "C" fn(*mut Value, f32) = load(c"ByondValue_SetNum")?;
        let mut number = Value::default();
        set_num(&mut number, 0.0);
        Ok(Api {
            read: load(c"Byond_ReadList")?,
            write: load(c"Byond_WriteList")?,
            inc: load(c"ByondValue_IncRef")?,
            dec: load(c"ByondValue_DecRef")?,
            complete: load(c"Byond_Return")?,
            error: load(c"Byond_SetLastError")?,
            num_kind: number.kind,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

impl Api {
    fn number(&self, value: f32) -> Value {
        Value {
            kind: self.num_kind,
            padding: [0; 3],
            data: value.to_bits(),
        }
    }
    fn num(&self, value: &Value) -> Result<f32, String> {
        let n = f32::from_bits(value.data);
        if value.kind != self.num_kind || !n.is_finite() {
            return Err("Expected finite number".into());
        }
        Ok(n)
    }
    fn integer(&self, value: &Value) -> Result<u32, String> {
        integer(self.num(value)?)
    }

    unsafe fn numbers(&self, list: &Value) -> Result<Vec<f32>, String> {
        let mut length = 0;
        let valid = (self.read)(list, std::ptr::null_mut(), &mut length);
        if (!valid && length == 0) || length > 2_000_000 {
            return Err("Invalid numeric packet".into());
        }
        let mut values = vec![Value::default(); length as usize];
        if !(self.read)(list, values.as_mut_ptr(), &mut length) {
            return Err("Cannot read packet".into());
        }
        let output = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                self.num(value).map_err(|error| {
                    format!("{error} at packet item {} (type {})", index + 1, value.kind)
                })
            })
            .collect();
        for value in &values {
            (self.dec)(value);
        }
        output
    }
    unsafe fn output(&self, list: &Value, numbers: &[f32]) -> Result<(), String> {
        let values: Vec<_> = numbers.iter().map(|n| self.number(*n)).collect();
        if !(self.write)(list, values.as_ptr(), values.len() as u32) {
            return Err("Cannot write result list".into());
        }
        Ok(())
    }
}

#[derive(Default)]
struct Registry {
    next: u32,
    engines: BTreeMap<u32, Arc<Engine>>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}
fn engine(id: u32) -> Result<Arc<Engine>, String> {
    registry()
        .lock()
        .map_err(|_| "Registry poisoned")?
        .engines
        .get(&id)
        .cloned()
        .ok_or("Unknown phase owner".into())
}

#[cfg(feature = "worker-profile")]
fn simd_enabled() -> bool {
    #[cfg(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64")))]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(all(feature = "simd", any(target_arch = "x86", target_arch = "x86_64"))))]
    {
        false
    }
}

fn last_error() -> &'static Mutex<CString> {
    static ERROR: OnceLock<Mutex<CString>> = OnceLock::new();
    ERROR.get_or_init(|| Mutex::new(CString::new("").unwrap()))
}

/// Legacy string ABI lets DM include the actual adapter error in a runtime report.
#[no_mangle]
pub extern "C" fn banda_atmos_error(_count: u32, _values: *const *const c_char) -> *const c_char {
    last_error()
        .lock()
        .map_or(c"Error diagnostic unavailable".as_ptr(), |error| {
            error.as_ptr()
        })
}

unsafe fn dispatch(api: &Api, args: &[Value]) -> Result<f32, String> {
    if matches!(api.integer(&args[0])?, 74..=81) {
        return projection::dispatch(api, args);
    }
    let opcode = api.integer(args.first().ok_or("Missing operation")?)?;
    if opcode == 0 {
        if args.len() != 1 {
            return Err("Unexpected arguments".into());
        }
        let mut registry = registry().lock().map_err(|_| "Registry poisoned")?;
        registry.next = registry
            .next
            .checked_add(1)
            .filter(|v| *v <= 16_777_215)
            .ok_or("Handle exhausted")?;
        let id = registry.next;
        registry.engines.insert(id, Arc::new(Engine::default()));
        return Ok(id as f32);
    }
    let id = api.integer(args.get(1).ok_or("Missing owner")?)?;
    let owner = engine(id)?;
    let mut store = owner.lock().map_err(|_| "Phase owner poisoned")?;
    if matches!(opcode, 82..=86) {
        return operations::dispatch(api, args, &mut store);
    }
    if matches!(opcode, 41..=49 | 68..=69) {
        return pipes::dispatch(api, args, &mut store);
    }
    let field = |index| {
        args.get(index)
            .ok_or_else(|| "Missing argument".to_string())
    };
    match (opcode, args.len()) {
        (1, 2) => {
            store.close();
            owner.notify();
            drop(store);
            registry()
                .lock()
                .map_err(|_| "Registry poisoned")?
                .engines
                .remove(&id);
            projection::close(id)?;
            return Ok(1.0);
        }
        (2, 7) => {
            let coordinates = api.numbers(field(5)?)?;
            if coordinates.len() != 3
                || coordinates
                    .iter()
                    .any(|n| *n < 1.0 || *n > 32767.0 || n.fract() != 0.0)
            {
                return Err("Invalid coordinates".into());
            }
            let bits = api.integer(field(3)?)?;
            let mut cell = Cell::new(mixture(&api.numbers(field(2)?)?)?, context(bits));
            cell.freeze_leaves_permafrost = api.num(field(4)?)? != 0.0;
            let data = api.numbers(field(6)?)?;
            let (tile, conducting) = tile(
                [
                    coordinates[0] as i32,
                    coordinates[1] as i32,
                    coordinates[2] as i32,
                ],
                &cell.air,
                bits,
                &data,
            )?;
            let id = store.register(cell, tile).map_err(|e| format!("{e:?}"))?;
            store
                .write(Write::Conducting(id, conducting))
                .map_err(|e| format!("{e:?}"))?;
            return Ok(id as f32);
        }
        (3, 6) => {
            let cell = api.integer(field(2)?)?;
            let data = api.numbers(field(5)?)?;
            let write = match api.integer(field(3)?)? {
                0 => Write::Replace(cell, mixture(&data)?),
                1 => Write::Release(cell, mixture(&data)?),
                mode @ (2 | 3) => {
                    let (air, archived) = full_mixture(&data)?;
                    Write::Commit(cell, air, archived, mode == 2)
                }
                _ => return Err("Invalid write mode".into()),
            };
            if api.num(field(4)?)? != 1.0 {
                return Err("Protocol version mismatch".into());
            }
            store.write(write).map_err(|e| format!("{e:?}"))?;
        }
        (4, 4) => {
            let id = api.integer(field(2)?)?;
            let frame = store.snapshot();
            let output = if let Some(sample) = store.pending_operation_sample(id) {
                packet::air_packet_with(&frame, &frame.cells[&id], &sample.air, false, true)
            } else {
                packet::air_packet(&frame, id)?
            };
            api.output(field(3)?, &output)?;
        }
        (5, 3) => {
            let commands = store.pending_commands().map_err(|e| format!("{e:?}"))?;
            let mut output = Vec::new();
            for command in commands {
                let (kind, a, b) = match command.effect.action {
                    WorldAction::Reaction(Effect::Freeze) => (1, 0.0, 0.0),
                    WorldAction::Reaction(Effect::Wet) => (2, 0.0, 0.0),
                    WorldAction::Reaction(Effect::Hotspot {
                        temperature,
                        volume,
                    }) => (3, temperature, volume),
                    WorldAction::Reaction(Effect::Radiation { range, threshold }) => {
                        (4, range, threshold)
                    }
                    WorldAction::Reaction(Effect::HotIce) => (5, 0.0, 0.0),
                    WorldAction::Reaction(Effect::Resin { volume }) => (6, volume, 0.0),
                    WorldAction::Reaction(Effect::NuclearParticle) => (7, 0.0, 0.0),
                    WorldAction::Reaction(Effect::Hallucination { radius, duration }) => {
                        (8, radius, duration)
                    }
                    WorldAction::FireExposure {
                        temperature,
                        volume,
                    } => (9, temperature, volume),
                    WorldAction::BurnTile => (10, 0.0, 0.0),
                    WorldAction::TurfExposure { temperature, gas } => {
                        (13, temperature, if gas { 1.0 } else { 0.0 })
                    }
                    WorldAction::Pressure {
                        difference,
                        direction,
                    } => (14, difference, direction as f32),
                    WorldAction::Reaction(Effect::RadiationChance { range, threshold }) => {
                        (11, range, threshold)
                    }
                    WorldAction::Reaction(Effect::HotIceChance) => (12, 0.0, 0.0),
                };
                output.extend([
                    command.cell as f32,
                    command.binding_version as f32,
                    kind as f32,
                    a,
                    b,
                ]);
            }
            api.output(field(2)?, &output)?;
            store.take_commands().map_err(|e| format!("{e:?}"))?;
        }
        (6, 3) => store
            .write(Write::Remove(api.integer(field(2)?)?))
            .map_err(|e| format!("{e:?}"))?,
        (7, 5) => store
            .write(Write::Context(
                api.integer(field(2)?)?,
                context(api.integer(field(3)?)?),
                api.num(field(4)?)? != 0.0,
                api.integer(field(3)?)? & 256 != 0,
            ))
            .map_err(|e| format!("{e:?}"))?,
        (8, 2) => return Ok(if store.busy() { 2.0 } else { 1.0 }),
        (9, 3) => api.output(
            field(2)?,
            &[
                11.0,
                GAS_COUNT as f32,
                REACTION_COUNT as f32,
                CATALOG_SIGNATURE as f32,
            ],
        )?,
        (10, 2) => {
            store.abort();
            owner.notify();
        }
        (11, 5) => {
            let neighbors = api.numbers(field(3)?)?;
            if neighbors.len() > 6
                || neighbors
                    .iter()
                    .any(|n| *n < 1.0 || *n > 16_777_215.0 || n.fract() != 0.0)
            {
                return Err("Invalid neighbors".into());
            }
            store
                .write(Write::Neighbors(
                    api.integer(field(2)?)?,
                    neighbors.into_iter().map(|n| n as u32).collect(),
                ))
                .map_err(|e| format!("{e:?}"))?;
            let links = api.numbers(field(4)?)?;
            if links.len() != 4
                || links
                    .iter()
                    .any(|n| *n < 0.0 || *n > 16_777_215.0 || n.fract() != 0.0)
            {
                return Err("Invalid thermal neighbors".into());
            }
            store
                .write(Write::ThermalNeighbors(
                    api.integer(field(2)?)?,
                    std::array::from_fn(|i| {
                        if links[i] == 0.0 {
                            None
                        } else {
                            Some(links[i] as u32)
                        }
                    }),
                ))
                .map_err(|e| format!("{e:?}"))?;
        }
        (12, 4) => {
            let id = api.integer(field(2)?)?;
            let frame = store.snapshot();
            let output = if let Some(sample) = store.pending_operation_sample(id) {
                let mut output = packet::turf_packet_with(
                    &frame,
                    id,
                    &frame.schedule.tiles[&id],
                    Some(&sample.archived),
                );
                output[6] = sample.last_share;
                output
            } else {
                packet::turf_packet(&frame, id)?
            };
            api.output(field(3)?, &output)?;
        }
        (13, 4) => {
            let value = terrain(&api.numbers(field(3)?)?)?;
            store
                .write(Write::Terrain(api.integer(field(2)?)?, value))
                .map_err(|e| format!("{e:?}"))?;
        }
        (14, 6) => {
            let temperature = api.num(field(3)?)?;
            let volume = api.num(field(4)?)?;
            let strengthen = api.num(field(5)?)?;
            if temperature < TCMB || volume < 0.0 || ![0.0, 1.0].contains(&strengthen) {
                return Err("Invalid hotspot exposure".into());
            }
            store
                .write(Write::Expose(
                    api.integer(field(2)?)?,
                    temperature,
                    volume,
                    strengthen != 0.0,
                ))
                .map_err(|e| format!("{e:?}"))?;
        }
        (15, 3) => store
            .write(Write::ClearHotspot(api.integer(field(2)?)?))
            .map_err(|e| format!("{e:?}"))?,
        (16, 2 | 3) => {
            let job = if args.len() == 3 {
                store.begin_at(api.integer(field(2)?)?)
            } else {
                store.begin()
            };
            let completed = job.map_err(|e| format!("{e:?}"))?.calculate();
            store.finish(completed).map_err(|e| format!("{e:?}"))?;
        }
        (63, 2) => {}
        (17 | 63, 3) => {
            let snapshot = store.snapshot();
            let schedule = &snapshot.schedule;
            let mut output = Vec::new();
            for ids in [&schedule.active, &schedule.hotspots, &schedule.conducting] {
                output.push(ids.len() as f32);
                output.extend(ids.iter().map(|id| *id as f32));
            }
            if opcode == 17 {
                output.push(schedule.groups.len() as f32);
                for group in schedule.groups.values() {
                    output.extend([
                        group.breakdown as f32,
                        group.dismantle as f32,
                        group.members.len() as f32,
                    ]);
                    output.extend(group.members.iter().map(|id| *id as f32));
                }
            }
            api.output(field(2)?, &output)?;
        }
        (18, 4) => {
            let active = api
                .numbers(field(2)?)?
                .into_iter()
                .map(integer)
                .collect::<Result<Vec<_>, _>>()?;
            let conducting = api
                .numbers(field(3)?)?
                .into_iter()
                .map(integer)
                .collect::<Result<Vec<_>, _>>()?;
            store
                .set_processing(active, conducting)
                .map_err(|e| format!("{e:?}"))?;
        }
        (19, 4) => {
            let mode = api.integer(field(3)?)?;
            if mode > 5 {
                return Err("Invalid scheduling control".into());
            }
            store
                .write(Write::Control(api.integer(field(2)?)?, mode as u8))
                .map_err(|e| format!("{e:?}"))?;
        }
        (20, 3) => {
            return store
                .invalidate_binding(api.integer(field(2)?)?)
                .map(|v| v as f32)
                .map_err(|e| format!("{e:?}"))
        }
        (21, 8) => {
            let id = api.integer(field(2)?)?;
            let version = api.integer(field(3)?)?;
            let (air, archived) = full_mixture(&api.numbers(field(4)?)?)?;
            let bits = api.integer(field(5)?)?;
            let mut cell = Cell::new(air, context(bits));
            cell.freeze_leaves_permafrost = api.num(field(6)?)? != 0.0;
            let data = api.numbers(field(7)?)?;
            if data.len() < 2 || data[..2].iter().any(|v| ![0.0, 1.0].contains(v)) {
                return Err("Invalid inheritance flags".into());
            }
            let coordinates = store
                .snapshot()
                .schedule
                .tiles
                .get(&id)
                .ok_or("Missing tile")?
                .coordinates;
            let (mut tile, _) = tile(coordinates, &cell.air, bits, &data[2..])?;
            tile.archived = archived;
            store
                .rebind(
                    id,
                    version,
                    crate::phase::Rebinding {
                        cell,
                        tile,
                        inherit_group: data[0] == 1.0,
                        inherit_gas: data[1] == 1.0,
                    },
                )
                .map_err(|e| format!("{e:?}"))?;
        }
        (22, 3) => {
            return store
                .binding_version(api.integer(field(2)?)?)
                .map(|v| v as f32)
                .map_err(|e| format!("{e:?}"))
        }
        (38, 4) => {
            let mode = api.integer(field(2)?)?;
            let cycle = api.integer(field(3)?)?;
            match mode {
                0 if cycle == 0 => {} // Additive capability probe; older ABI 11 DLLs reject it.
                1 => return owner.publish(store, cycle).map(|()| 1.0),
                _ => return Err("Invalid worker synchronization request".into()),
            }
        }
        #[cfg(feature = "worker-profile")]
        (39, 3) => {
            api.output(field(2)?, &owner.profile())?;
        }
        (23..=37 | 40 | 50..=53 | 64..=66 | 73 | 87..=90, _) => {
            return world::dispatch(api, &mut store, args)
        }
        (54, 2) => pipe_direct::vent_available()?,
        (55, 8) => return pipe_direct::transfer_vent(api, &mut store, &args[2..]),
        (56, 2) => pipe_direct::vent_available()?,
        (57, 8) if !store.busy() => return pipe_direct::transfer_scrubber(api, &args[2..]),
        (58, 3) if api.integer(field(2)?)? == 1 => pipe_direct::vent_available()?,
        (59, 4) => {
            let id = api.integer(field(2)?)?;
            let version = api.integer(field(3)?)?;
            let Some((air, _)) = store.scrubber_air(id, version) else {
                return Ok(-1.0);
            };
            let keys = air.keys().iter().fold(0u32, |keys, gas| keys | (1 << gas));
            // Present zero keys count; zero as an FFI return indicates an error.
            return Ok((keys + 1) as f32);
        }
        (60 | 71, 8) => {
            return pipe_direct::transfer_scrubber_tile(api, &mut store, &args[2..], opcode == 71)
        }
        (61, 3) => pipe_direct::finish_scrubber_tiles(&mut store, api.integer(field(2)?)? == 1)?,
        (61, 4) if api.integer(field(2)?)? == 0 => {
            let ids: Vec<_> = store
                .scrubber_edits
                .iter()
                .map(|edit| edit.0 as f32)
                .collect();
            api.output(field(3)?, &ids)?;
        }
        (62, 6) => return pipe_direct::take_scrubber_tile(api, &mut store, &args[2..]),
        (67, 6) => return pipe_direct::transfer_passive_vent(api, &mut store, &args[2..]),
        (70, 7) => return pipe_direct::transfer_owned_vent(api, &mut store, &args[2..]),
        (72, 6) => return pipe_direct::transfer_owned_passive(api, &mut store, &args[2..]),
        _ => return Err("Invalid operation or arity".into()),
    }
    Ok(1.0)
}

#[no_mangle]
pub unsafe extern "C" fn banda_atmos(count: u32, values: *const Value) -> Value {
    let Ok(api) = api() else {
        return Value::default();
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if count == 0 || count > 8 || values.is_null() {
            return Err("Invalid arguments".into());
        }
        dispatch(api, std::slice::from_raw_parts(values, count as usize))
    }))
    .unwrap_or_else(|_| Err("Panic in atmosphere adapter".into()));
    match result {
        Ok(n) => api.number(n),
        Err(error) => {
            if let Ok(error) = CString::new(error) {
                (api.error)(error.as_ptr());
                if let Ok(mut saved) = last_error().lock() {
                    *saved = error;
                }
            }
            api.number(0.0)
        }
    }
}

/// The isolated-region protocol publishes before notifying the waiting DM proc.
#[no_mangle]
pub unsafe extern "C" fn banda_atmos_tick(count: u32, values: *const Value, waiting: Value) {
    worker::start(count, values, waiting, false);
}

/// Whole-world work publishes only when DM enters a safe phase, including synchronous consumers.
#[no_mangle]
pub unsafe extern "C" fn banda_atmos_world_tick(count: u32, values: *const Value, waiting: Value) {
    worker::start(count, values, waiting, true);
}
