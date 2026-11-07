//! Main-thread projection and cache baselines. Workers never own these BYOND references.
use super::{load, Api, Value};
use std::{
    collections::BTreeMap,
    ffi::CStr,
    sync::{Arc, Mutex, OnceLock},
};
#[path = "byond_publication.rs"]
mod publication;

macro_rules! fields {
    ($($name:ident),+ $(,)?) => {
        struct Fields { $( $name: u32, )+ }
        impl Fields {
            unsafe fn load() -> Result<Self, String> {
                let get: unsafe extern "C" fn(*const std::ffi::c_char) -> u32 = load(c"Byond_GetStrId")?;
                Ok(Self { $( $name: {
                    let name = CStr::from_bytes_with_nul_unchecked(concat!(stringify!($name), "\0").as_bytes());
                    let id = get(name.as_ptr());
                    if id == 65535 { return Err(format!("Missing projection field {}", stringify!($name))); }
                    id
                }, )+ })
            }
        }
    }
}
fields!(
    temperature,
    temperature_archived,
    volume,
    moles,
    moles_archive,
    reaction_results,
    last_share,
    binding_version,
    pending_rebind,
    native_air_kind,
    cached_air,
    readonly_air,
    cache_current,
    cache_cycle,
    committed_air,
    committed_from_native,
    source_cycle_offset,
    air_cycle,
    excited,
    archived_cycle,
    current_cycle,
    significant_share_ticker,
    pressure_difference,
    pressure_direction,
    banda_atmos_cell,
    banda_air_present,
    flags_1,
    air,
    x,
    y,
    z,
    atmos_overlay_types,
    active_hotspot,
    hotspots_pending,
    max_plane_offset,
    z_level_to_plane_offset,
    service_publications_skipped,
    visual_publications,
    banda_readonly,
    banda_atmos_id,
    banda_cell,
    banda_version,
    direct_publications,
    gc_destroyed
);

struct Direct {
    fields: Fields,
    read_var: unsafe extern "C" fn(*const Value, u32, *mut Value) -> bool,
    write_var: unsafe extern "C" fn(*const Value, u32, *const Value) -> bool,
    read_assoc: unsafe extern "C" fn(*const Value, *mut Value, *mut u32) -> bool,
    read_index: unsafe extern "C" fn(*const Value, *const Value, *mut Value) -> bool,
    write_index: unsafe extern "C" fn(*const Value, *const Value, *const Value) -> bool,
    create_list: unsafe extern "C" fn(*mut Value) -> bool,
    truth: unsafe extern "C" fn(*const Value) -> bool,
    equal: unsafe extern "C" fn(*const Value, *const Value) -> bool,
    is_type: unsafe extern "C" fn(*const Value, *const std::ffi::c_char) -> bool,
}

fn direct() -> Result<&'static Direct, String> {
    static API: OnceLock<Result<Direct, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        Ok(Direct {
            fields: Fields::load()?,
            read_var: load(c"Byond_ReadVarByStrId")?,
            write_var: load(c"Byond_WriteVarByStrId")?,
            read_assoc: load(c"Byond_ReadListAssoc")?,
            read_index: load(c"Byond_ReadListIndex")?,
            write_index: load(c"Byond_WriteListIndex")?,
            create_list: load(c"Byond_CreateList")?,
            truth: load(c"ByondValue_IsTrue")?,
            equal: load(c"ByondValue_Equals")?,
            is_type: load(c"ByondValue_IsType")?,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

struct Values {
    values: Vec<Value>,
    dec: unsafe extern "C" fn(*const Value),
}
impl Drop for Values {
    fn drop(&mut self) {
        for value in &self.values {
            unsafe { (self.dec)(value) };
        }
    }
}
impl std::ops::Deref for Values {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        &self.values
    }
}

unsafe fn read_values(api: &Api, list: &Value, assoc: bool) -> Result<Values, String> {
    let read = if assoc {
        direct()?.read_assoc
    } else {
        api.read
    };
    let mut length = 0;
    if !read(list, std::ptr::null_mut(), &mut length) && length == 0 || length > 2_000_000 {
        return Err("Invalid projection list".into());
    }
    let mut result = Values {
        values: vec![Value::default(); length as usize],
        dec: api.dec,
    };
    if !read(list, result.values.as_mut_ptr(), &mut length) || length as usize != result.len() {
        return Err("Cannot read projection list".into());
    }
    Ok(result)
}
struct One {
    value: [Value; 1],
    dec: unsafe extern "C" fn(*const Value),
}
impl std::ops::Deref for One {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        &self.value
    }
}
impl Drop for One {
    fn drop(&mut self) {
        unsafe { (self.dec)(&self.value[0]) };
    }
}
unsafe fn read_var(api: &Api, object: &Value, field: u32) -> Result<One, String> {
    let mut result = One {
        value: [Value::default()],
        dec: api.dec,
    };
    if !(direct()?.read_var)(object, field, &mut result.value[0]) {
        return Err(format!("Cannot read projection field {field}"));
    }
    Ok(result)
}
unsafe fn read_index(api: &Api, list: &Value, key: &Value) -> Result<One, String> {
    let mut result = One {
        value: [Value::default()],
        dec: api.dec,
    };
    if !(direct()?.read_index)(list, key, &mut result.value[0]) {
        return Err("Cannot read projection index".into());
    }
    Ok(result)
}
unsafe fn write_var(object: &Value, field: u32, value: &Value) -> Result<(), String> {
    if !(direct()?.write_var)(object, field, value) {
        return Err(format!("Cannot write projection field {field}"));
    }
    Ok(())
}
unsafe fn new_list(api: &Api) -> Result<One, String> {
    let mut result = One {
        value: [Value::default()],
        dec: api.dec,
    };
    if !(direct()?.create_list)(&mut result.value[0]) {
        return Err("Cannot create projection list".into());
    }
    Ok(result)
}
unsafe fn write_pairs(api: &Api, list: &Value, pairs: &[Value]) -> Result<(), String> {
    let keys: Vec<_> = pairs.chunks_exact(2).map(|p| p[0]).collect();
    if !(api.write)(list, std::ptr::null(), 0)
        || !(api.write)(list, keys.as_ptr(), keys.len() as u32)
    {
        return Err("Cannot replace projection keys".into());
    }
    for pair in pairs.chunks_exact(2) {
        if !(direct()?.write_index)(list, &pair[0], &pair[1]) {
            return Err("Cannot write projection association".into());
        }
    }
    Ok(())
}

struct Baseline {
    scalars: Values,
    lists: [Values; 3],
}
unsafe fn capture(api: &Api, air: &Value) -> Result<Baseline, String> {
    let f = &direct()?.fields;
    let mut scalars = Values {
        values: Vec::with_capacity(3),
        dec: api.dec,
    };
    for field in [f.temperature, f.temperature_archived, f.volume] {
        let value = read_var(api, air, field)?;
        (api.inc)(&value[0]);
        scalars.values.push(value[0]);
    }
    let mut lists = Vec::new();
    for field in [f.moles, f.moles_archive, f.reaction_results] {
        let list = read_var(api, air, field)?;
        lists.push(read_values(api, &list[0], true)?);
    }
    Ok(Baseline {
        scalars,
        lists: lists.try_into().ok().unwrap(),
    })
}
unsafe fn matches(api: &Api, air: &Value, baseline: &Baseline) -> Result<bool, String> {
    let d = direct()?;
    let f = &d.fields;
    for (index, field) in [f.temperature, f.temperature_archived, f.volume]
        .into_iter()
        .enumerate()
    {
        let value = read_var(api, air, field)?;
        if index < 2 && !(d.truth)(&value[0]) || !(d.equal)(&value[0], &baseline.scalars[index]) {
            return Ok(false);
        }
    }
    for (index, field) in [f.moles, f.moles_archive, f.reaction_results]
        .into_iter()
        .enumerate()
    {
        let list = read_var(api, air, field)?;
        let values = read_values(api, &list[0], true)?;
        if values.len() != baseline.lists[index].len() {
            return Ok(false);
        }
        for (current, previous) in values
            .chunks_exact(2)
            .zip(baseline.lists[index].chunks_exact(2))
        {
            if !(d.equal)(&current[0], &previous[0])
                || !(d.truth)(&current[1])
                || !(d.equal)(&current[1], &previous[1])
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Default)]
struct State {
    baselines: BTreeMap<(u32, u32), Arc<Baseline>>,
    batches: BTreeMap<(u32, u32), publication::Batch>,
    next_batch: u32,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}
fn baseline(owner: u32, id: u32) -> Result<Option<Arc<Baseline>>, String> {
    Ok(state()
        .lock()
        .map_err(|_| "Projection cache poisoned")?
        .baselines
        .get(&(owner, id))
        .cloned())
}
unsafe fn remember(api: &Api, owner: u32, id: u32, air: &Value) -> Result<(), String> {
    let new = Arc::new(capture(api, air)?);
    let old = {
        state()
            .lock()
            .map_err(|_| "Projection cache poisoned")?
            .baselines
            .insert((owner, id), new)
    };
    drop(old);
    Ok(())
}
pub(super) fn close(owner: u32) -> Result<(), String> {
    let (removed, batches) = {
        let mut state = state().lock().map_err(|_| "Projection cache poisoned")?;
        let ids: Vec<_> = state
            .baselines
            .keys()
            .filter(|(o, _)| *o == owner)
            .copied()
            .collect();
        let batches: Vec<_> = state
            .batches
            .keys()
            .filter(|(o, _)| *o == owner)
            .copied()
            .collect();
        let batches = batches
            .into_iter()
            .filter_map(|id| state.batches.remove(&id))
            .collect::<Vec<_>>();
        (
            ids.into_iter()
                .filter_map(|id| state.baselines.remove(&id))
                .collect::<Vec<_>>(),
            batches,
        )
    };
    drop(removed);
    drop(batches);
    Ok(())
}

pub(super) unsafe fn dispatch(api: &Api, args: &[Value]) -> Result<f32, String> {
    let opcode = api.integer(&args[0])?;
    let owner = api.integer(&args[1])?;
    super::engine(owner)?;
    let _ = direct()?;
    match (opcode, args.len()) {
        (74, 2) => {}
        (75..=77, _) => return publication::dispatch(api, owner, opcode, args),
        (78, 4) => remember(api, owner, api.integer(&args[2])?, &args[3])?,
        (79, 4) => {
            let previous = baseline(owner, api.integer(&args[2])?)?;
            return Ok(if let Some(previous) = previous {
                if matches(api, &args[3], &previous)? {
                    2.0
                } else {
                    1.0
                }
            } else {
                1.0
            });
        }
        (80, 3) => {
            let old = {
                state()
                    .lock()
                    .map_err(|_| "Projection cache poisoned")?
                    .baselines
                    .remove(&(owner, api.integer(&args[2])?))
            };
            drop(old);
        }
        (81, 4) => {
            let previous =
                baseline(owner, api.integer(&args[2])?)?.ok_or("Missing cache baseline")?;
            let mut output = previous.scalars.values.clone();
            let mut lists = Vec::new();
            for pairs in &previous.lists {
                let list = new_list(api)?;
                write_pairs(api, &list[0], pairs)?;
                output.push(list[0]);
                lists.push(list);
            }
            if !(api.write)(&args[3], output.as_ptr(), output.len() as u32) {
                return Err("Cannot export cache baseline".into());
            }
        }
        _ => return Err("Invalid projection operation".into()),
    }
    Ok(1.0)
}
