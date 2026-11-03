use crate::phase::Frame;
use crate::{
    catalog::*,
    network::Tile,
    thermal::Terrain,
    transport::{Boundary, Sample},
    Archive, Context, Holder, Mixture,
};

pub(super) fn air_packet(snapshot: &Frame, id: u32) -> Result<Vec<f32>, String> {
    air_packet_for(snapshot, id, false, true)
}

pub(super) fn publication_air(snapshot: &Frame, id: u32, cache: bool) -> Result<Vec<f32>, String> {
    air_packet_for(snapshot, id, true, cache)
}

fn air_packet_for(
    snapshot: &Frame,
    id: u32,
    compact: bool,
    cache: bool,
) -> Result<Vec<f32>, String> {
    let cell = snapshot.cells.get(&id).ok_or("Stale cell")?;
    let mut output = vec![
        snapshot.epoch as f32,
        cell.air.temperature,
        cell.air.volume,
        cell.air.keys().len() as f32,
    ];
    for &gas in cell.air.keys() {
        output.extend([gas as f32, cell.air.moles[gas]]);
    }
    if compact {
        output.push(if cache {
            cell.air.results.iter().filter(|r| r.is_some()).count() as f32
        } else {
            0.0
        });
        if cache {
            for (reaction, result) in cell.air.results.iter().enumerate() {
                if let Some(value) = result {
                    output.extend([reaction as f32, *value]);
                }
            }
        }
    } else {
        for result in cell.air.results.iter().copied() {
            output.extend([
                if result.is_some() { 1.0 } else { 0.0 },
                result.unwrap_or(0.0),
            ]);
        }
    }
    if let Some(spot) = &cell.hotspot {
        output.extend([1.0, spot.temperature, spot.volume]);
    } else {
        output.extend([0.0, 0.0, 0.0]);
    }
    output.push(cell.flags as f32);
    Ok(output)
}

pub(super) fn turf_packet(snapshot: &Frame, id: u32) -> Result<Vec<f32>, String> {
    publication_turf(snapshot, id, true)
}

pub(super) fn publication_turf(snapshot: &Frame, id: u32, cache: bool) -> Result<Vec<f32>, String> {
    let tile = snapshot.schedule.tiles.get(&id).ok_or("Stale turf")?;
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
    if cache {
        output.extend([tile.archived.temperature, tile.archived.keys().len() as f32]);
        for &gas in tile.archived.keys() {
            output.extend([gas as f32, tile.archived.moles[gas]]);
        }
    }
    Ok(output)
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
    let count = integer(*data.get(3).ok_or("Missing mixture header")?)? as usize;
    if count > GAS_COUNT {
        return Err("Invalid gas count".into());
    }
    let end = 4 + count * 2;
    let mut air = mixture(data.get(..end).ok_or("Missing mixture")?)?;
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
    let archived = mixture(
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
    if values.len() < 4 {
        return Err("Incomplete mixture".into());
    }
    let count = integer(values[3])? as usize;
    if count > GAS_COUNT
        || values.len() != 4 + count * 2
        || values[1] <= 0.0
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
