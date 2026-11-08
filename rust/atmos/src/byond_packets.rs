use crate::phase::{Frame, Store};
use crate::{
    catalog::*,
    network::Tile,
    thermal::Terrain,
    transport::{Boundary, Sample},
    Archive, Context, Holder, Mixture,
};

/// Published observations do not materialize mixture arrays or consume pending changes.
pub(super) fn properties(store: &Store, ids: &[u32]) -> Vec<f32> {
    let frame = store.snapshot();
    let mut output = vec![1.0, frame.epoch as f32, ids.len() as f32];
    for &id in ids {
        let Some(version) = store.published_binding_version(id) else {
            output.extend([0.0; 6]);
            continue;
        };
        let tile = &frame.schedule.tiles[&id];
        let air = store
            .pending_operation_air(id)
            .unwrap_or(&frame.cells[&id].air);
        output.extend([if tile.has_air { 1.0 } else { 2.0 }, version as f32]);
        output.extend(air_properties(air, &tile.boundary));
    }
    output
}

pub(super) fn air_properties(air: &Mixture, boundary: &Boundary) -> [f32; 4] {
    [
        if air.volume == 0.0 {
            0.0
        } else {
            air.pressure()
        },
        air.temperature,
        air.volume,
        boundary.heat_capacity(air),
    ]
}

/// One numeric batch for cells/ports. All-gas rows preserve key order and present zeroes.
pub(super) fn air_values(
    store: &Store,
    targets: &[crate::operations::Target],
    gases: &[usize],
    all_gases: bool,
) -> Vec<f32> {
    use crate::operations::Target;
    let mut output = vec![1.0, targets.len() as f32];
    for &target in targets {
        let version = match target {
            Target::Cell { version, .. } => version,
            Target::Port(handle) => handle.generation,
        };
        append_value_row(
            &mut output,
            store
                .query_air(target)
                .map(|(air, boundary)| (air, boundary, 1.0)),
            version,
            gases,
            all_gases,
        );
    }
    output
}

pub(super) fn reference_values(
    store: &Store,
    handles: &[crate::pipenet::Handle],
    gases: &[usize],
    all_gases: bool,
) -> Vec<f32> {
    let mut output = vec![1.0, handles.len() as f32];
    for &handle in handles {
        append_value_row(
            &mut output,
            store
                .query_air_reference(handle)
                .map(|(air, boundary, retired)| (air, boundary, if retired { 2.0 } else { 1.0 })),
            handle.generation,
            gases,
            all_gases,
        );
    }
    output
}

fn append_value_row(
    output: &mut Vec<f32>,
    sample: Option<(&Mixture, &Boundary, f32)>,
    version: u32,
    gases: &[usize],
    all_gases: bool,
) {
    let Some((air, boundary, status)) = sample else {
        output.extend([0.0, version as f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        return;
    };
    let [pressure, temperature, volume, capacity] = air_properties(air, boundary);
    let count = if all_gases {
        air.keys().len()
    } else {
        gases.len()
    };
    output.extend([
        status,
        version as f32,
        temperature,
        pressure,
        volume,
        capacity,
        air.total_moles(),
        count as f32,
    ]);
    if all_gases {
        for &gas in air.keys() {
            output.extend([gas as f32, air.moles[gas]]);
        }
    } else {
        output.extend(gases.iter().map(|&gas| air.moles[gas]));
    }
}

pub(super) fn air_packet(snapshot: &Frame, id: u32) -> Result<Vec<f32>, String> {
    air_packet_for(snapshot, id, false, true)
}

pub(super) fn publication_air(snapshot: &Frame, id: u32, cache: bool) -> Result<Vec<f32>, String> {
    air_packet_for(snapshot, id, true, cache)
}

/// Cold publications carry gas/state descriptors, never the mixture's moles or archives.
pub(super) fn append_publication_visuals(
    output: &mut Vec<f32>,
    snapshot: &Frame,
    id: u32,
    rules: &crate::visibility::Visibility,
    omit_empty: bool,
    include_temperature: bool,
    pending_air: Option<&Mixture>,
) -> Result<(), String> {
    let cell = snapshot.cells.get(&id).ok_or("Stale cell")?;
    let air = pending_air.unwrap_or(&cell.air);
    let start = output.len();
    let temperature = if include_temperature {
        air.temperature
    } else {
        0.0
    };
    output.extend([snapshot.epoch as f32, temperature, 0.0]);
    append_visibility(output, air, rules);
    if omit_empty && !include_temperature && output.len() == start + 4 && cell.hotspot.is_none() {
        output.truncate(start);
        return Ok(());
    }
    output.push(0.0);
    append_hotspot(output, cell);
    Ok(())
}

pub(super) fn append_visibility(
    output: &mut Vec<f32>,
    air: &Mixture,
    rules: &crate::visibility::Visibility,
) {
    let count = output.len();
    output.push(0.0);
    for &gas in air.keys() {
        if let Some(state) = rules.state(gas, air.moles[gas]) {
            output.extend([gas as f32, state as f32]);
        }
    }
    output[count] = ((output.len() - count - 1) / 2) as f32;
}

fn air_packet_for(
    snapshot: &Frame,
    id: u32,
    compact: bool,
    cache: bool,
) -> Result<Vec<f32>, String> {
    let cell = snapshot.cells.get(&id).ok_or("Stale cell")?;
    Ok(air_packet_with(snapshot, cell, &cell.air, compact, cache))
}

pub(super) fn air_packet_with(
    snapshot: &Frame,
    cell: &crate::cell::Cell,
    air: &Mixture,
    compact: bool,
    cache: bool,
) -> Vec<f32> {
    let mut output = vec![
        snapshot.epoch as f32,
        air.temperature,
        air.volume,
        air.keys().len() as f32,
    ];
    for &gas in air.keys() {
        output.extend([gas as f32, air.moles[gas]]);
    }
    if compact {
        output.push(if cache {
            air.results.iter().filter(|r| r.is_some()).count() as f32
        } else {
            0.0
        });
        if cache {
            for (reaction, result) in air.results.iter().enumerate() {
                if let Some(value) = result {
                    output.extend([reaction as f32, *value]);
                }
            }
        }
    } else {
        for result in air.results.iter().copied() {
            output.extend([
                if result.is_some() { 1.0 } else { 0.0 },
                result.unwrap_or(0.0),
            ]);
        }
    }
    append_hotspot(&mut output, cell);
    output
}

fn append_hotspot(output: &mut Vec<f32>, cell: &crate::cell::Cell) {
    if let Some(spot) = &cell.hotspot {
        output.extend([1.0, spot.temperature, spot.volume]);
    } else {
        output.extend([0.0, 0.0, 0.0]);
    }
    output.push(cell.flags as f32);
}

pub(super) fn turf_packet(snapshot: &Frame, id: u32) -> Result<Vec<f32>, String> {
    publication_turf(snapshot, id, true)
}

pub(super) fn publication_turf(snapshot: &Frame, id: u32, cache: bool) -> Result<Vec<f32>, String> {
    let tile = snapshot.schedule.tiles.get(&id).ok_or("Stale turf")?;
    Ok(turf_packet_with(
        snapshot,
        id,
        tile,
        cache.then_some(&tile.archived),
    ))
}

pub(super) fn turf_packet_with(
    snapshot: &Frame,
    id: u32,
    tile: &Tile,
    archive: Option<&Archive>,
) -> Vec<f32> {
    let spot = snapshot.cells[&id].hotspot.as_ref();
    let mut output = vec![
        tile.terrain.temperature,
        tile.terrain.archived,
        tile.archived_cycle as f32,
        tile.current_cycle as f32,
        tile.ticker as f32,
        if tile.excited { 1.0 } else { 0.0 },
        snapshot.schedule.last_share(id),
        tile.pressure,
        tile.direction as f32,
        spot.map_or(0.0, |s| if s.just_spawned { 1.0 } else { 0.0 }),
        spot.map_or(0.0, |s| if s.bypassing { 1.0 } else { 0.0 }),
        spot.map_or(0.0, |s| if s.cold_fire { 1.0 } else { 0.0 }),
    ];
    if let Some(archive) = archive {
        output.extend([archive.temperature, archive.keys().len() as f32]);
        for &gas in archive.keys() {
            output.extend([gas as f32, archive.moles[gas]]);
        }
    }
    output
}

pub(super) fn tile(
    coordinates: [i32; 3],
    air: &Mixture,
    bits: u32,
    data: &[f32],
) -> Result<(Tile, bool), String> {
    if data.len() < 11
        || ![0.0, 1.0, 2.0].contains(&data[0])
        || data[1..4].iter().any(|v| ![0.0, 1.0].contains(v))
        || ![0.0, 1.0].contains(&data[10])
    {
        return Err("Invalid environment metadata".into());
    }
    let mut tile = Tile::new(coordinates, air, bits & 256 != 0);
    tile.has_air = data[0] != 2.0;
    tile.run_later = data[1] != 0.0;
    tile.excited = data[2] != 0.0;
    tile.terrain = terrain(&[
        if data[10] == 1.0 { 2.0 } else { 0.0 },
        data[4],
        data[5],
        data[6],
        data[7],
        data[8],
    ])?;
    tile.thermal_enabled = tile.terrain.conductivity != 0.0;
    if ![0.0, 1.0].contains(&data[9]) || (!tile.has_air && (tile.excited || !air.keys().is_empty()))
    {
        return Err("Invalid solid/thermal state".into());
    }
    if data[0] == 1.0 {
        if !air.keys().is_empty() || air.temperature != TCMB {
            return Err("Space must have its canonical immutable air".into());
        }
        tile.boundary = Boundary::Space;
    }
    if data[3] == 1.0 {
        let initial = mixture(&data[11..])?;
        let mut planet = Sample::new(initial.clone());
        planet.boundary = Boundary::Planetary(Box::new(initial));
        tile.planetary = Some(Box::new(planet));
    } else if data.len() != 11 {
        return Err("Unexpected atmosphere metadata".into());
    }
    Ok((tile, data[9] != 0.0))
}

pub(super) fn full_mixture(data: &[f32]) -> Result<(Mixture, Archive), String> {
    full_with(data, mixture)
}

pub(super) fn pipe_full_mixture(data: &[f32]) -> Result<(Mixture, Archive), String> {
    full_with(data, |values| mixture_with_zero_volume(values, true))
}

fn full_with(
    data: &[f32],
    decode: fn(&[f32]) -> Result<Mixture, String>,
) -> Result<(Mixture, Archive), String> {
    let count = integer(*data.get(3).ok_or("Missing mixture header")?)? as usize;
    if count > GAS_COUNT {
        return Err("Invalid gas count".into());
    }
    let end = 4 + count * 2;
    let mut air = decode(data.get(..end).ok_or("Missing mixture")?)?;
    let results = data
        .get(end..end + REACTION_COUNT * 2)
        .ok_or("Missing reaction results")?;
    for (index, pair) in results.chunks_exact(2).enumerate() {
        if ![0.0, 1.0].contains(&pair[0]) {
            return Err("Invalid reaction result flag".into());
        }
        if pair[0] == 1.0 {
            air.results[index] = Some(pair[1]);
        }
    }
    let archived = decode(
        data.get(end + REACTION_COUNT * 2..)
            .ok_or("Missing archive")?,
    )?
    .archive();
    Ok((air, archived))
}

pub(super) fn integer(n: f32) -> Result<u32, String> {
    if !(0.0..=16_777_215.0).contains(&n) || n.fract() != 0.0 {
        return Err("Invalid integer".into());
    }
    Ok(n as u32)
}

pub(super) fn mixture(values: &[f32]) -> Result<Mixture, String> {
    mixture_with_zero_volume(values, false)
}

pub(super) fn mixture_with_zero_volume(
    values: &[f32],
    allow_zero: bool,
) -> Result<Mixture, String> {
    if values.len() < 4 {
        return Err("Incomplete mixture".into());
    }
    let count = integer(values[3])? as usize;
    if count > GAS_COUNT
        || values.len() != 4 + count * 2
        || values[1] < 0.0
        || (!allow_zero && values[1] == 0.0)
        || ![0.0, 1.0].contains(&values[2])
    {
        return Err("Invalid mixture".into());
    }
    let mut air = Mixture::new(values[0], values[1], values[2] != 0.0);
    for pair in values[4..].chunks_exact(2) {
        let gas = integer(pair[0])? as usize;
        if gas >= GAS_COUNT || pair[1] < 0.0 || air.keys().contains(&gas) {
            return Err("Invalid gas".into());
        }
        air.set(gas, pair[1]);
    }
    Ok(air)
}

pub(super) fn terrain(values: &[f32]) -> Result<Terrain, String> {
    if values.len() != 6
        || ![0.0, 1.0, 2.0].contains(&values[0])
        || values[3] < 0.0
        || values[4] < 0.0
        || integer(values[5])? > 255
    {
        return Err("Invalid physical turf metadata".into());
    }
    Ok(Terrain {
        closed: values[0] == 2.0,
        temperature: values[1],
        archived: values[2],
        capacity: values[3],
        conductivity: values[4],
        blocked: values[5] as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::{mixture, tile};
    use crate::{catalog::*, transport::Boundary};

    #[test]
    fn visual_publication_has_no_moles_or_results_and_keeps_hotspot_tail() {
        let mut frame = crate::phase::Frame::default();
        let mut air = crate::Mixture::new(777.0, CELL_VOLUME, true);
        air.set(NITROGEN, 1000.0);
        air.set(PLASMA, 0.75);
        air.set(OXYGEN, 100.0);
        air.results[0] = Some(17.0);
        let mut cell = crate::cell::Cell::new(air, crate::Context::default());
        cell.hotspot = Some(crate::cell::Hotspot {
            temperature: 900.0,
            volume: 42.0,
            just_spawned: true,
            bypassing: false,
            cold_fire: false,
        });
        frame.cells.insert(1, cell);
        let mut rules = vec![-1.0; GAS_COUNT];
        rules[PLASMA] = 0.25;
        rules.extend([0.25, 80.0]);
        let rules = crate::visibility::Visibility::from_values(&rules).unwrap();
        let mut data = Vec::new();
        super::append_publication_visuals(&mut data, &frame, 1, &rules, true, false, None).unwrap();
        assert_eq!(&data[..7], &[0.0, 0.0, 0.0, 1.0, PLASMA as f32, 3.0, 0.0]);
        assert_eq!(&data[7..], &[1.0, 900.0, 42.0, 0.0]);
        assert!(!data.contains(&777.0) && !data.contains(&1000.0) && !data.contains(&17.0));

        let cell = frame.cells.get_mut(&1).unwrap();
        cell.air.set(PLASMA, 0.0);
        data.clear();
        super::append_publication_visuals(&mut data, &frame, 1, &rules, true, false, None).unwrap();
        assert_eq!(&data[5..], &[1.0, 900.0, 42.0, 0.0]);

        frame.cells.get_mut(&1).unwrap().hotspot = None;
        data.truncate(1);
        super::append_publication_visuals(&mut data, &frame, 1, &rules, true, false, None).unwrap();
        assert_eq!(data, [0.0]);
        super::append_publication_visuals(&mut data, &frame, 1, &rules, false, false, None)
            .unwrap();
        assert_eq!(data, [0.0; 10]);
        data.clear();
        super::append_publication_visuals(&mut data, &frame, 1, &rules, true, true, None).unwrap();
        assert_eq!(data, [0.0, 777.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn value_queries_preserve_order_zeroes_pending_edits_and_versions() {
        use crate::{
            cell::Cell,
            network::Tile,
            operations::{Operation, Target},
            phase::Store,
            transport::Sample,
            Context, Mixture,
        };
        let mut store = Store::default();
        let mut air = Mixture::new(333.0, CELL_VOLUME, false);
        for gas in (0..GAS_COUNT).rev() {
            air.set(gas, gas as f32);
        }
        air.set(OXYGEN, -0.0);
        let id = store
            .register(
                Cell::new(air.clone(), Context::default()),
                Tile::new([1, 1, 1], &air, false),
            )
            .unwrap();
        let target = Target::Cell { id, version: 1 };
        let port = store.pipes.register(Sample::new(air.clone())).unwrap();
        let before = store.snapshot();
        let data = super::air_values(&store, &[target, Target::Port(port)], &[], true);
        assert_eq!(&data[..2], &[1.0, 2.0]);
        for row in data[2..].chunks_exact(8 + 2 * GAS_COUNT) {
            assert_eq!(
                &row[..8],
                &[
                    1.0,
                    1.0,
                    333.0,
                    air.pressure(),
                    CELL_VOLUME,
                    air.heat_capacity(),
                    air.total_moles(),
                    GAS_COUNT as f32
                ]
            );
            for (pair, &gas) in row[8..].chunks_exact(2).zip(air.keys()) {
                assert_eq!(pair[0], gas as f32);
                assert_eq!(pair[1].to_bits(), air.moles[gas].to_bits());
            }
        }
        assert!(std::sync::Arc::ptr_eq(&before, &store.snapshot()));
        assert!(store.take_changes().unwrap().is_empty());
        store.begin_operations().unwrap();
        store
            .operate(target, &[Operation::SetTemperature(450.0)], false)
            .unwrap();
        let data = super::air_values(&store, &[target], &[PLASMA, OXYGEN, PLASMA], false);
        assert_eq!(data[4], 450.0);
        assert_eq!(&data[10..], &[air.moles[PLASMA], -0.0, air.moles[PLASMA]]);
        assert_eq!(store.snapshot().cells[&id].air.temperature, 333.0);
        store.finish_operations(false, false).unwrap();
        assert_eq!(store.snapshot().cells[&id].air.temperature, 450.0);
        store.pipes.release(port).unwrap();
        store.invalidate_binding(id).unwrap();
        let data = super::air_values(&store, &[target, Target::Port(port)], &[], true);
        assert_eq!(&data[2..10], &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&data[10..], &data[2..10]);
    }

    #[test]
    fn value_queries_read_published_immutable_air_during_calculation() {
        use crate::{
            cell::Cell, network::Tile, operations::Target, phase::Store, Context, Mixture,
        };
        let mut store = Store::default();
        let air = Mixture::new(TCMB, CELL_VOLUME, false);
        let mut tile = Tile::new([1, 1, 1], &air, true);
        tile.boundary = Boundary::Space;
        let id = store
            .register(Cell::new(air.clone(), Context::default()), tile)
            .unwrap();
        let target = Target::Cell { id, version: 1 };
        let before = super::air_values(&store, &[target], &[OXYGEN], false);
        assert_eq!(before[7], HEAT_CAPACITY_VACUUM);
        let job = store.begin().unwrap();
        assert_eq!(
            before,
            super::air_values(&store, &[target], &[OXYGEN], false)
        );
        store.stage(job.calculate()).unwrap();
        assert_eq!(
            before,
            super::air_values(&store, &[target], &[OXYGEN], false)
        );
        store.publish_ready(1).unwrap();
        store.close();
        assert_eq!(super::air_values(&store, &[target], &[], false)[2], 0.0);
    }

    #[test]
    fn properties_are_read_only_and_keep_pressure_capacity_and_request_order() {
        use crate::{cell::Cell, network::Tile, phase::Store, Context, Mixture};
        let mut store = Store::default();
        let mut air = Mixture::new(333.0, 2500.0, false);
        for gas in (0..GAS_COUNT).rev() {
            air.set(gas, (gas + 1) as f32 * 0.25);
        }
        air.results[0] = Some(0.0);
        let id = store
            .register(
                Cell::new(air.clone(), Context::default()),
                Tile::new([1, 1, 1], &air, false),
            )
            .unwrap();
        let vacuum = Mixture::new(800.0, 0.0, false);
        let zero = store
            .register(
                Cell::new(vacuum.clone(), Context::default()),
                Tile::new([2, 1, 1], &vacuum, false),
            )
            .unwrap();
        let space_air = Mixture::new(TCMB, CELL_VOLUME, false);
        let mut tile = Tile::new([3, 1, 1], &space_air, true);
        tile.boundary = Boundary::Space;
        let space = store
            .register(Cell::new(space_air, Context::default()), tile)
            .unwrap();
        let mut tile = Tile::new([4, 1, 1], &vacuum, true);
        tile.has_air = false;
        let solid = store
            .register(Cell::new(vacuum, Context::default()), tile)
            .unwrap();
        let mut terrain = store.snapshot().schedule.tiles[&id].terrain.clone();
        terrain.temperature += 1.0;
        store
            .write(crate::phase::Write::Terrain(id, terrain))
            .unwrap();
        let before = store.snapshot();
        let data = super::properties(&store, &[id, zero, space, solid, id, 999]);
        assert_eq!(&data[..3], &[1.0, 0.0, 6.0]);
        assert_eq!(
            &data[3..9],
            &[1.0, 1.0, air.pressure(), 333.0, 2500.0, air.heat_capacity()]
        );
        assert_eq!(&data[9..15], &[1.0, 1.0, 0.0, 800.0, 0.0, 0.0]);
        assert_eq!(
            &data[15..21],
            &[1.0, 1.0, 0.0, TCMB, CELL_VOLUME, HEAT_CAPACITY_VACUUM]
        );
        assert_eq!(data[21], 2.0);
        assert_eq!(&data[27..33], &data[3..9]);
        assert_eq!(&data[33..], &[0.0; 6]);
        assert!(std::sync::Arc::ptr_eq(&before, &store.snapshot()));
        assert_eq!(store.snapshot().cells[&id].air, air);
        assert_eq!(store.take_changes().unwrap(), vec![id]);
        store.invalidate_binding(id).unwrap();
        assert_eq!(&super::properties(&store, &[id])[3..], &[0.0; 6]);
    }

    #[test]
    fn properties_use_published_state_until_a_worker_is_published() {
        use crate::{cell::Cell, network::Tile, phase::Store, Context, Mixture};
        let mut store = Store::default();
        let air = Mixture::new(300.0, CELL_VOLUME, false);
        let id = store
            .register(
                Cell::new(air.clone(), Context::default()),
                Tile::new([1, 1, 1], &air, false),
            )
            .unwrap();
        let before = super::properties(&store, &[id]);
        let job = store.begin().unwrap();
        assert_eq!(before, super::properties(&store, &[id]));
        store.stage(job.calculate()).unwrap();
        assert_eq!(before, super::properties(&store, &[id]));
        store.publish_ready(before[1] as u32 + 1).unwrap();
        assert_ne!(before[1], super::properties(&store, &[id])[1]);
    }

    #[test]
    fn only_pipe_packets_allow_zero_volume() {
        let mut packet = vec![0.0, 0.0, 0.0, 0.0];
        packet.extend([0.0; REACTION_COUNT * 2]);
        packet.extend([123.0, 0.0, 0.0, 0.0]);
        assert!(super::full_mixture(&packet).is_err());
        let (air, archive) = super::pipe_full_mixture(&packet).unwrap();
        assert_eq!(air.volume, 0.0);
        assert_eq!(air.temperature, 0.0);
        assert_eq!(archive.temperature, 123.0);
        packet[1] = -1.0;
        assert!(super::pipe_full_mixture(&packet).is_err());
    }

    #[test]
    fn compact_publication_preserves_full_state_and_can_omit_unborrowed_archives() {
        let mut frame = crate::phase::Frame::default();
        let mut air = crate::Mixture::new(333.0, 100.0, true);
        air.set(NITROGEN, 17.0);
        air.set(OXYGEN, -0.0);
        air.results[0] = Some(0.0);
        air.results[REACTION_COUNT - 1] = Some(7.0);
        let mut tile = crate::network::Tile::new([1, 2, 3], &air, false);
        tile.archived = air.archive();
        frame.schedule.tiles.insert(1, tile);
        frame
            .cells
            .insert(1, crate::cell::Cell::new(air, crate::Context::default()));
        let legacy = super::air_packet(&frame, 1).unwrap();
        let compact = super::publication_air(&frame, 1, true).unwrap();
        let prefix = 4 + legacy[3] as usize * 2;
        let mut expanded = compact[..prefix].to_vec();
        let mut results = vec![0.0; REACTION_COUNT * 2];
        let count = compact[prefix] as usize;
        for pair in compact[prefix + 1..prefix + 1 + count * 2].chunks_exact(2) {
            results[pair[0] as usize * 2] = 1.0;
            results[pair[0] as usize * 2 + 1] = pair[1];
        }
        expanded.extend(results);
        expanded.extend(&compact[prefix + 1 + count * 2..]);
        assert_eq!(
            expanded.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            legacy.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(
            super::publication_turf(&frame, 1, true).unwrap(),
            super::turf_packet(&frame, 1).unwrap()
        );
        let presentation = super::publication_air(&frame, 1, false).unwrap();
        assert_eq!(&presentation[..prefix], &legacy[..prefix]);
        assert_eq!(presentation[prefix], 0.0);
        assert_eq!(presentation.len(), prefix + 5);
        assert_eq!(super::publication_turf(&frame, 1, false).unwrap().len(), 12);
        assert!(compact.len() < legacy.len());
    }

    #[test]
    fn accepts_finite_negative_temperature_from_the_original_space_exchange() {
        // Banda's two_space_boundaries oracle briefly produces -256 K through cancellation.
        // The transport result must round-trip through a safe DM callback unchanged.
        let air = mixture(&[-256.0, 2500.0, 1.0, 0.0]).unwrap();
        assert_eq!(air.temperature, -256.0);
    }

    #[test]
    fn temporary_blocking_preserves_air_and_space_class() {
        let air = mixture(&[293.15, CELL_VOLUME, 1.0, 1.0, NITROGEN as f32, 17.0]).unwrap();
        let mut data = [
            0.0, 0.0, 1.0, 0.0, 293.15, 293.15, 700000.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let (floor, _) = tile([1, 1, 1], &air, 0, &data).unwrap();
        assert!(floor.has_air && floor.terrain.closed && floor.excited);
        let space = mixture(&[TCMB, CELL_VOLUME, 0.0, 0.0]).unwrap();
        data[0] = 1.0;
        let (space_tile, _) = tile([2, 1, 1], &space, 0, &data).unwrap();
        assert!(space_tile.has_air && space_tile.terrain.closed);
        assert!(matches!(space_tile.boundary, Boundary::Space));
        assert_eq!(
            space_tile.boundary.heat_capacity(&space),
            HEAT_CAPACITY_VACUUM
        );
        data[0] = 2.0;
        assert!(tile([1, 1, 1], &air, 0, &data).is_err());
        data[2] = 0.0;
        let (wall, _) = tile([1, 1, 1], &space, 0, &data).unwrap();
        assert!(!wall.has_air && wall.terrain.closed);
    }
}

pub(super) fn context(bits: u32) -> Context {
    Context {
        holder: Holder::OpenTurf,
        groundless: bits & 4 != 0,
        no_slip: bits & 8 != 0,
        foam: bits & 16 != 0,
        resin: bits & 32 != 0,
        radiation_roll: bits & 64 != 0,
        hot_ice_roll: bits & 128 != 0,
        defer_random: bits & 512 != 0,
        ..Context::default()
    }
}
