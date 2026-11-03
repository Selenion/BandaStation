use super::*;

pub(super) struct Binding {
    pub version: u32,
    pub attached: bool,
}

#[derive(Clone, Debug)]
pub struct Rebinding {
    pub cell: Cell,
    pub tile: Tile,
    pub inherit_group: bool,
    pub inherit_gas: bool,
}

impl Store {
    pub fn binding_version(&self, id: CellId) -> Result<u32, Error> {
        self.bindings
            .get(&id)
            .map(|binding| binding.version)
            .ok_or(Error::MissingCell)
    }

    /// Physics belongs to this coordinate; commands belong to one incarnation of its DM turf.
    pub fn invalidate_binding(&mut self, id: CellId) -> Result<u32, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let binding = self.bindings.get_mut(&id).ok_or(Error::MissingCell)?;
        binding.version = binding
            .version
            .checked_add(1)
            .filter(|v| *v <= MAX_EXACT_DM_INTEGER)
            .ok_or(Error::IdentifierExhausted)?;
        binding.attached = false;
        self.detached.insert(id);
        self.commands.retain(|command| command.cell != id);
        Ok(binding.version)
    }

    pub fn rebind(
        &mut self,
        id: CellId,
        version: u32,
        replacement: Rebinding,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let binding = self.bindings.get_mut(&id).ok_or(Error::MissingCell)?;
        if binding.version != version || binding.attached {
            return Err(Error::StaleBinding);
        }
        if self.published.schedule.tiles[&id].coordinates != replacement.tile.coordinates
            || (replacement.inherit_group && !replacement.tile.has_air)
        {
            return Err(Error::InvalidTopology);
        }
        binding.attached = true;
        self.detached.remove(&id);
        self.write(Write::Rebind(id, version, Box::new(replacement)))
    }
}

pub(super) fn apply_rebind(
    frame: &mut Frame,
    id: CellId,
    mut replacement: Rebinding,
    sync_ids: &mut BTreeSet<CellId>,
) {
    if let Some(shares) = &mut frame.schedule.max_shares {
        shares.remove(&id);
    }
    let Some(previous) = frame.cells.get(&id) else {
        return;
    };
    if replacement.inherit_gas && !replacement.tile.boundary.immutable() {
        replacement.cell.air.moles.clone_from(&previous.air.moles);
        replacement.cell.air.order = previous.air.order.clone();
        replacement.cell.air.temperature = previous.air.temperature;
        // DM's inherited copy_from resets archive amounts to zero with the inherited key order.
        replacement.tile.archived.moles.fill(0.0);
        replacement.tile.archived.order = previous.air.order.clone();
    }
    let old = &frame.schedule.tiles[&id];
    replacement.tile.neighbors = if replacement.tile.terrain.closed {
        Vec::new()
    } else {
        old.neighbors.clone()
    };
    replacement.tile.thermal_neighbors = old.thermal_neighbors;
    if replacement.inherit_group {
        replacement.tile.group = old.group;
        replacement.tile.excited = old.excited;
    } else {
        if let Some(group) = old.group {
            if let Some(group) = frame.schedule.groups.get(&group) {
                sync_ids.extend(group.members.iter().copied());
            }
            frame.schedule.clear_group(group);
        }
        frame.schedule.active.retain(|n| *n != id);
        if replacement.tile.excited && replacement.tile.has_air {
            frame.schedule.active.push(id);
        }
    }
    if replacement.tile.terrain.closed {
        let linked: Vec<_> = frame
            .schedule
            .tiles
            .iter()
            .filter_map(|(other, tile)| tile.neighbors.contains(&id).then_some(other))
            .collect();
        for other in linked {
            sync_ids.insert(other);
            frame
                .schedule
                .tiles
                .get_mut(&other)
                .unwrap()
                .neighbors
                .retain(|n| *n != id);
        }
    }
    // Open turf.Destroy deletes its hotspot even when ChangeTurf inherits the atmosphere.
    replacement.cell.hotspot = None;
    frame.schedule.hotspots.retain(|n| *n != id);
    frame.cells.insert(id, replacement.cell);
    frame.schedule.tiles.insert(id, replacement.tile);
    frame.schedule.refresh_space_override(id);
}
