use super::*;

pub(super) fn apply(frame: &mut Frame, write: Write, sync_ids: &mut BTreeSet<CellId>) -> bool {
    sync_ids.insert(write.cell_id());
    let context_only = matches!(write, Write::Context(_, _, _, _));
    if let Write::ArchiveMetadata(id, cycle, temperature) = write {
        let tile = frame.schedule.tiles.get_mut(&id).unwrap();
        let changed = tile.archived_cycle != cycle || tile.terrain.archived != temperature;
        tile.archived_cycle = cycle;
        tile.terrain.archived = temperature;
        return changed;
    }
    if let Write::ReplaceFull(id, air, archived, space) = write {
        if let (Some(cell), Some(tile)) =
            (frame.cells.get_mut(&id), frame.schedule.tiles.get_mut(&id))
        {
            // Direct DM turf.air assignment replaces the mixture, not the turf or its hotspot/group.
            cell.air = air;
            tile.has_air = true;
            tile.archived = archived;
            tile.boundary = if space {
                crate::transport::Boundary::Space
            } else {
                crate::transport::Boundary::Mutable
            };
            tile.last_share = 0.0;
            frame.schedule.refresh_space_override(id);
        }
        return true;
    }
    if let Write::Topology(id, neighbors, thermal) = write {
        if let Some(tile) = frame.schedule.tiles.get_mut(&id) {
            // World scheduling already follows the original adjacency rebuild's activation goal.
            // Reassigning an equal vector would separate otherwise shared space state.
            if tile.neighbors != neighbors {
                tile.neighbors = neighbors;
            }
            tile.thermal_neighbors = thermal;
        }
        return true;
    }
    if let Write::HotspotState(id, hotspot) = write {
        if let Some(cell) = frame.cells.get_mut(&id) {
            cell.hotspot = hotspot;
            if cell.hotspot.is_some() {
                if !frame.schedule.hotspots.contains(&id) {
                    frame.schedule.hotspots.push(id);
                }
            } else {
                frame.schedule.hotspots.retain(|n| *n != id);
            }
        }
        return true;
    }
    if let Write::Rebind(id, version, replacement) = write {
        binding::apply_rebind(frame, id, *replacement, sync_ids);
        frame.binding_versions[id as usize] = version;
        return true;
    }
    if let Write::Control(id, mode) = write {
        if !frame.cells.contains_key(&id) {
            return true;
        }
        if mode == 1 || mode == 3 {
            if let Some(group) = frame.schedule.tiles[&id].group {
                if let Some(group) = frame.schedule.groups.get(&group) {
                    sync_ids.extend(group.members.iter().copied());
                }
                frame.schedule.clear_group(group);
            }
        }
        match mode {
            0 | 1 => frame.schedule.activate(id),
            2 | 3 => {
                frame.schedule.active.retain(|n| *n != id);
                frame.schedule.tiles.get_mut(&id).unwrap().excited = false;
            }
            4 | 5 => {
                // DM's consider_superconductivity already checked the live temperature/capacity.
                // Those external values may not have been imported into this frame yet.
                if !frame.schedule.conducting.contains(&id) {
                    frame.schedule.conducting.push(id);
                }
            }
            _ => unreachable!(),
        }
        return true;
    }
    if let Write::ClearHotspot(id) = write {
        if let Some(cell) = frame.cells.get_mut(&id) {
            cell.hotspot = None;
        }
        frame.schedule.hotspots.retain(|n| *n != id);
        return true;
    }
    if let Write::Expose(id, temperature, volume, strengthen) = write {
        if let Some(cell) = frame.cells.get_mut(&id) {
            if frame.schedule.tiles[&id].has_air {
                let existed = cell.hotspot.is_some();
                cell.hotspot_expose(temperature, volume, strengthen);
                if !existed && cell.hotspot.is_some() {
                    cell.hotspot.as_mut().unwrap().just_spawned =
                        frame.schedule.tiles[&id].current_cycle < frame.epoch;
                    frame.schedule.hotspots.push(id);
                    frame.schedule.activate(id);
                }
            }
        }
        return true;
    }
    if let Write::ThermalNeighbors(id, neighbors) = write {
        let neighbors = neighbors.map(|n| n.filter(|n| frame.cells.contains_key(n)));
        if let Some(tile) = frame.schedule.tiles.get_mut(&id) {
            tile.thermal_neighbors = neighbors;
        }
        return true;
    }
    if let Write::Terrain(id, terrain) = write {
        if let Some(tile) = frame.schedule.tiles.get_mut(&id) {
            let changed = tile.terrain != terrain;
            tile.thermal_enabled = terrain.conductivity != 0.0;
            tile.terrain = terrain;
            return changed;
        }
        return true;
    }
    if let Write::Conducting(id, enabled) = write {
        if frame.cells.contains_key(&id) {
            if enabled && !frame.schedule.conducting.contains(&id) {
                frame.schedule.conducting.push(id);
            } else if !enabled {
                frame.schedule.conducting.retain(|n| *n != id);
            }
        }
        return true;
    }
    if let Write::Commit(id, air, archived, wake) = write {
        let mut visible = true;
        if let Some(cell) = frame.cells.get_mut(&id) {
            if !frame.schedule.tiles[&id].boundary.immutable() {
                visible = cell.air != air || frame.schedule.tiles[&id].archived != archived;
                let changed = cell.air.moles != air.moles
                    || cell.air.keys() != air.keys()
                    || cell.air.temperature != air.temperature;
                cell.air.moles = air.moles;
                cell.air.order = air.order;
                cell.air.temperature = air.temperature;
                cell.air.results = air.results;
                frame.schedule.tiles.get_mut(&id).unwrap().archived = archived;
                if changed && wake {
                    frame.schedule.activate(id);
                }
            }
        }
        return visible;
    }
    if let Write::Remove(id) = write {
        sync_ids.extend(frame.schedule.remove(id));
        frame.cells.remove(&id);
        return true;
    }
    if let Write::Neighbors(id, neighbors) = write {
        let neighbors: Vec<_> = neighbors
            .into_iter()
            .filter(|n| frame.cells.contains_key(n))
            .collect();
        if let Some(tile) = frame.schedule.tiles.get_mut(&id) {
            if tile.neighbors != neighbors {
                tile.neighbors = neighbors;
                frame.schedule.activate(id);
            }
        }
        return true;
    }
    let (id, replacement, release, context) = match write {
        Write::Replace(id, air) => (id, Some(air), None, None),
        Write::Release(id, air) => (id, None, Some(air), None),
        Write::Context(id, context, permafrost, thermal) => {
            (id, None, None, Some((context, permafrost, thermal)))
        }
        Write::Remove(_)
        | Write::Neighbors(_, _)
        | Write::ThermalNeighbors(_, _)
        | Write::Terrain(_, _)
        | Write::Conducting(_, _)
        | Write::Commit(_, _, _, _)
        | Write::ArchiveMetadata(_, _, _)
        | Write::ReplaceFull(_, _, _, _) => unreachable!(),
        Write::Rebind(_, _, _) | Write::HotspotState(_, _) | Write::Topology(_, _, _) => {
            unreachable!()
        }
        Write::Expose(_, _, _, _) | Write::ClearHotspot(_) | Write::Control(_, _) => unreachable!(),
    };
    // A preceding removal invalidates later queued writes to that handle.
    if let Some(cell) = frame.cells.get_mut(&id) {
        if let Some(air) = replacement {
            if !frame.schedule.tiles[&id].boundary.immutable() {
                let changed = cell.air.moles != air.moles
                    || cell.air.keys() != air.keys()
                    || cell.air.temperature != air.temperature;
                // copy_from copies gas/temperature, keeping the receiver's volume, class and reaction results.
                cell.air.moles = air.moles;
                cell.air.order = air.order;
                cell.air.temperature = air.temperature;
                let archived = &mut frame.schedule.tiles.get_mut(&id).unwrap().archived;
                archived.moles.fill(0.0);
                archived.order = cell.air.order.clone();
                if changed {
                    frame.schedule.activate(id);
                }
            }
        }
        if let Some(air) = release {
            if !frame.schedule.tiles[&id].boundary.immutable() {
                merge(&mut cell.air, &air);
            }
            frame.schedule.activate(id);
        }
        if let Some((context, permafrost, thermal)) = context {
            cell.context = context;
            cell.freeze_leaves_permafrost = permafrost;
            frame.schedule.tiles.get_mut(&id).unwrap().thermal_enabled = thermal;
        }
    }
    !context_only
}

pub(super) fn drain_effects(frame: &mut Frame, id: CellId, binding_version: u32) -> Vec<Command> {
    // Topology/context writes normally have no effects; borrowing mutably would copy a cold page.
    if frame
        .cells
        .get(&id)
        .is_none_or(|cell| cell.commands.is_empty())
    {
        return Vec::new();
    }
    frame.cells.get_mut(&id).map_or_else(Vec::new, |cell| {
        cell.commands
            .drain(..)
            .map(|effect| Command {
                cell: id,
                binding_version,
                effect,
            })
            .collect()
    })
}
