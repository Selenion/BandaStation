//! Bounded numeric batches for the world adapter. No BYOND references escape a call.
use super::*;
use crate::cell::Hotspot;
use crate::{Archive, Mixture};

#[cfg(test)]
mod tests {
    use super::Cursor;

    #[test]
    fn compact_commit_preserves_zero_results_order_and_archive() {
        let data = [
            8.0, 333.0, 100.0, 1.0, 2.0, 3.0, 17.0, 0.0, -0.0, 2.0, 0.0, 0.0, 25.0, 7.0, 6.0,
            290.0, 100.0, 1.0, 1.0, 0.0, 5.0,
        ];
        let mut cursor = Cursor {
            data: &data,
            position: 0,
        };
        let (air, archive) = cursor.full_air().unwrap();
        cursor.done().unwrap();
        assert_eq!(air.keys(), &[3, 0]);
        assert_eq!(air.moles[0].to_bits(), (-0.0_f32).to_bits());
        assert_eq!(air.results[0], Some(0.0));
        assert_eq!(air.results[1], None);
        assert_eq!(air.results[25], Some(7.0));
        assert_eq!(archive.temperature, 290.0);
        assert_eq!(archive.keys(), &[0]);
        assert_eq!(archive.moles[0], 5.0);
        let mut duplicate = data;
        duplicate[12] = 0.0;
        assert!(Cursor {
            data: &duplicate,
            position: 0
        }
        .full_air()
        .is_err());
        assert!(Cursor {
            data: &data[..data.len() - 1],
            position: 0
        }
        .full_air()
        .is_err());
    }
}

struct Cursor<'a> {
    data: &'a [f32],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn number(&mut self) -> Result<f32, String> {
        let value = *self
            .data
            .get(self.position)
            .ok_or("Incomplete world packet")?;
        self.position += 1;
        Ok(value)
    }
    fn count(&mut self) -> Result<usize, String> {
        Ok(integer(self.number()?)? as usize)
    }
    fn packet(&mut self) -> Result<&'a [f32], String> {
        let length = self.count()?;
        let end = self
            .position
            .checked_add(length)
            .ok_or("Packet length overflow")?;
        let values = self
            .data
            .get(self.position..end)
            .ok_or("Incomplete subpacket")?;
        self.position = end;
        Ok(values)
    }
    fn done(&self) -> Result<(), String> {
        if self.position == self.data.len() {
            Ok(())
        } else {
            Err("Trailing world packet data".into())
        }
    }

    fn full_air(&mut self) -> Result<(Mixture, Archive), String> {
        let mut air = mixture(self.packet()?)?;
        let count = self.count()?;
        if count > REACTION_COUNT {
            return Err("Invalid result count".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..count {
            let reaction = self.count()?;
            let value = self.number()?;
            if reaction >= REACTION_COUNT || !seen.insert(reaction) {
                return Err("Invalid reaction result".into());
            }
            air.results[reaction] = Some(value);
        }
        let archived = mixture(self.packet()?)?.archive();
        Ok((air, archived))
    }
}

pub(super) unsafe fn dispatch(api: &Api, store: &mut Store, args: &[Value]) -> Result<f32, String> {
    let field = |i| {
        args.get(i)
            .ok_or_else(|| "Missing world argument".to_string())
    };
    let opcode = api.integer(field(0)?)?;
    match (opcode, args.len()) {
        (23, 4) => {
            let data = api.numbers(field(2)?)?;
            let mut cursor = Cursor {
                data: &data,
                position: 0,
            };
            let count = cursor.count()?;
            if count > 256 {
                return Err("Registration batch too large".into());
            }
            let mut entries = Vec::new();
            let mut positions = std::collections::BTreeSet::new();
            for _ in 0..count {
                let xyz = [
                    cursor.count()? as i32,
                    cursor.count()? as i32,
                    cursor.count()? as i32,
                ];
                if xyz.iter().any(|v| !(1..=32767).contains(v))
                    || store.cell_at(xyz).is_some()
                    || !positions.insert(xyz)
                {
                    return Err("Invalid/duplicate coordinates".into());
                }
                let bits = cursor.count()? as u32;
                let freeze = cursor.number()? != 0.0;
                let air_data = cursor.packet()?;
                let mut nested = Cursor {
                    data: air_data,
                    position: 0,
                };
                let (air, archived) = nested.full_air()?;
                nested.done()?;
                let (mut tile, conducting) = tile(xyz, &air, bits, cursor.packet()?)?;
                tile.archived = archived;
                let mut cell = Cell::new(air, context(bits));
                cell.freeze_leaves_permafrost = freeze;
                entries.push((cell, tile, conducting));
            }
            cursor.done()?;
            let mut ids = Vec::new();
            for (cell, tile, conducting) in entries {
                let id = store.register(cell, tile).map_err(|e| format!("{e:?}"))?;
                if conducting {
                    store
                        .write(Write::Conducting(id, true))
                        .map_err(|e| format!("{e:?}"))?;
                }
                ids.push(id as f32);
            }
            api.output(field(3)?, &ids)?;
        }
        (24, 3) => {
            let data = api.numbers(field(2)?)?;
            let mut cursor = Cursor {
                data: &data,
                position: 0,
            };
            let count = cursor.count()?;
            if count > 256 {
                return Err("Topology batch too large".into());
            }
            let mut writes = Vec::new();
            for _ in 0..count {
                let id = cursor.count()? as u32;
                let neighbors = cursor.count()?;
                if neighbors > 6 {
                    return Err("Too many gas neighbors".into());
                }
                let mut links = Vec::new();
                for _ in 0..neighbors {
                    links.push(cursor.count()? as u32);
                }
                let mut thermal = [None; 4];
                for value in &mut thermal {
                    let id = cursor.count()? as u32;
                    *value = if id == 0 { None } else { Some(id) };
                }
                writes.push(Write::Topology(id, links, thermal));
            }
            cursor.done()?;
            for write in writes {
                store.write(write).map_err(|e| format!("{e:?}"))?;
            }
        }
        (25, 3) => {
            let data = api.numbers(field(2)?)?;
            let mut cursor = Cursor {
                data: &data,
                position: 0,
            };
            let count = cursor.count()?;
            if count > 256 {
                return Err("Input batch too large".into());
            }
            let mut writes = Vec::new();
            for _ in 0..count {
                let id = cursor.count()? as u32;
                let bits = cursor.count()? as u32;
                let freeze = cursor.number()? != 0.0;
                writes.push(Write::Context(id, context(bits), freeze, bits & 256 != 0));
                let air = cursor.packet()?;
                if !air.is_empty() {
                    let (air, archive) = full_mixture(air)?;
                    writes.push(Write::Commit(id, air, archive, false));
                }
                writes.push(Write::Terrain(id, terrain(cursor.packet()?)?));
            }
            cursor.done()?;
            for write in writes {
                store.write(write).map_err(|e| format!("{e:?}"))?;
            }
        }
        (26, 3) => {
            let ids = store.take_changes().map_err(|e| format!("{e:?}"))?;
            api.output(
                field(2)?,
                &ids.into_iter().map(|n| n as f32).collect::<Vec<_>>(),
            )?;
        }
        (27 | 35, 4) => {
            let requests = api.numbers(field(2)?)?;
            let compact = opcode == 35;
            let width = if compact { 2 } else { 1 };
            if requests.len() > 256 * width || requests.len() % width != 0 {
                return Err("Output batch too large".into());
            }
            let snapshot = store.snapshot();
            let mut output = Vec::new();
            for request in requests.chunks_exact(width) {
                let id = integer(request[0])?;
                let cache = if compact {
                    if ![0.0, 1.0].contains(&request[1]) {
                        return Err("Invalid cache flag".into());
                    }
                    request[1] == 1.0
                } else {
                    true
                };
                let tile = snapshot
                    .schedule
                    .tiles
                    .get(&id)
                    .ok_or("Unknown output cell")?;
                let air = if compact {
                    packet::publication_air(&snapshot, id, cache)?
                } else {
                    packet::air_packet(&snapshot, id)?
                };
                let metadata = packet::publication_turf(&snapshot, id, cache)?;
                output.extend([
                    id as f32,
                    store.binding_version(id).map_err(|e| format!("{e:?}"))? as f32,
                    tile.coordinates[0] as f32,
                    tile.coordinates[1] as f32,
                    tile.coordinates[2] as f32,
                    if !tile.has_air {
                        4.0
                    } else if matches!(tile.boundary, crate::transport::Boundary::Space) {
                        3.0
                    } else if snapshot.cells[&id].air.turf_heat_capacity {
                        2.0
                    } else {
                        1.0
                    },
                    air.len() as f32,
                ]);
                output.extend(air);
                output.push(metadata.len() as f32);
                output.extend(metadata);
            }
            api.output(field(3)?, &output)?;
        }
        (36, 5) => {
            let id = api.integer(field(2)?)?;
            let wake = api.integer(field(3)?)?;
            if wake > 1 {
                return Err("Invalid wake flag".into());
            }
            let data = api.numbers(field(4)?)?;
            let mut cursor = Cursor {
                data: &data,
                position: 0,
            };
            let (air, archived) = cursor.full_air()?;
            cursor.done()?;
            store
                .write(Write::Commit(id, air, archived, wake == 1))
                .map_err(|e| format!("{e:?}"))?;
        }
        (28, 4) => {
            let data = api.numbers(field(2)?)?;
            if data.len() != 3 {
                return Err("Invalid coordinate lookup".into());
            }
            let xyz = [
                integer(data[0])? as i32,
                integer(data[1])? as i32,
                integer(data[2])? as i32,
            ];
            api.output(field(3)?, &[store.cell_at(xyz).unwrap_or(0) as f32])?;
        }
        (29, 3) => {
            let data = api.numbers(field(2)?)?;
            if data.len() != 1 && data.len() != 6 {
                return Err("Invalid hotspot state packet".into());
            }
            let id = integer(data[0])?;
            let hotspot = if data.len() == 6 {
                if data[3..].iter().any(|v| ![0.0, 1.0].contains(v)) {
                    return Err("Invalid hotspot flags".into());
                }
                Some(Hotspot {
                    temperature: data[1],
                    volume: data[2],
                    just_spawned: data[3] != 0.0,
                    bypassing: data[4] != 0.0,
                    cold_fire: data[5] != 0.0,
                })
            } else {
                None
            };
            store
                .write(Write::HotspotState(id, hotspot))
                .map_err(|e| format!("{e:?}"))?;
        }
        (30, 2) => store.discard_changes().map_err(|e| format!("{e:?}"))?,
        (31, 4) => {
            let detail = api.integer(field(2)?)?;
            if detail > 1 {
                return Err("Invalid debug detail".into());
            }
            let pending = store.pending_commands().map_err(|e| format!("{e:?}"))?;
            let frame = store.snapshot();
            let schedule = &frame.schedule;
            let mut output = vec![
                schedule.active.len() as f32,
                schedule.hotspots.len() as f32,
                schedule.groups.len() as f32,
                schedule.conducting.len() as f32,
                pending
                    .iter()
                    .filter(|c| matches!(c.effect.action, WorldAction::Pressure { .. }))
                    .count() as f32,
            ];
            if detail == 1 {
                for (&id, group) in &schedule.groups {
                    // A group ID can outlive f32's exact integer range. Four u16 words remain exact in DM.
                    output.extend((0..4).map(|word| ((id >> (word * 16)) & 65535) as f32));
                    let maximum = schedule.max_shares.as_ref().map_or(0.0, |shares| {
                        group
                            .members
                            .iter()
                            .filter_map(|id| shares.get(id))
                            .copied()
                            .fold(0.0, f32::max)
                    });
                    output.extend([
                        group.breakdown as f32,
                        group.dismantle as f32,
                        group.members.len() as f32,
                        maximum,
                    ]);
                    output.extend(group.members.iter().map(|id| *id as f32));
                }
            }
            api.output(field(3)?, &output)?;
        }
        (32, 3) => {
            let id = api.integer(field(2)?)?;
            let frame = store.snapshot();
            let cell = frame.cells.get(&id).ok_or("Unknown gas cell")?;
            let tile = &frame.schedule.tiles[&id];
            return Ok(if !tile.has_air {
                4.0
            } else if matches!(tile.boundary, crate::transport::Boundary::Space) {
                3.0
            } else if cell.air.turf_heat_capacity {
                2.0
            } else {
                1.0
            });
        }
        (33, 5) => {
            let id = api.integer(field(2)?)?;
            let (air, archive) = full_mixture(&api.numbers(field(3)?)?)?;
            let space = api.integer(field(4)?)?;
            if space > 1 {
                return Err("Invalid space mixture flag".into());
            }
            store
                .write(Write::ReplaceFull(id, air, archive, space == 1))
                .map_err(|e| format!("{e:?}"))?;
        }
        (34, 3) => {
            let enabled = api.integer(field(2)?)?;
            if enabled > 1 {
                return Err("Invalid max share tracker flag".into());
            }
            store
                .track_max_share(enabled == 1)
                .map_err(|e| format!("{e:?}"))?;
        }
        (37, 5) => {
            let id = api.integer(field(2)?)?;
            let cycle = api.integer(field(3)?)?;
            let temperature = api.num(field(4)?)?;
            store
                .write(Write::ArchiveMetadata(id, cycle, temperature))
                .map_err(|e| format!("{e:?}"))?;
        }
        _ => return Err("Invalid world operation/arity".into()),
    }
    Ok(1.0)
}
