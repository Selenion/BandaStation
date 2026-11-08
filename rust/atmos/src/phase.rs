//! Phase ownership inspired by Paradise MILLA: native work never invokes game objects.
use crate::storage::Storage;
use crate::{
    cell::{merge, Cell, Hotspot, WorldCommand},
    network::{Schedule, Tile},
    thermal::Terrain,
    Archive, Context, Mixture,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[path = "phase_binding.rs"]
mod binding;
use binding::Binding;
pub use binding::Rebinding;
#[path = "phase_write.rs"]
mod writes;
use writes::{apply, drain_effects};
#[path = "phase_operations.rs"]
mod operations;
#[path = "phase_references.rs"]
mod references;

#[cfg(test)]
#[path = "phase_tests.rs"]
mod tests;

pub type CellId = u32;
const MAX_EXACT_DM_INTEGER: u32 = 16_777_215;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Publication {
    #[default]
    Service,
    Temperature,
    Thermal,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub epoch: u32,
    pub cells: Storage<Cell>,
    pub schedule: Schedule,
    binding_versions: Vec<u32>,
}

#[derive(Clone, Debug)]
pub enum Write {
    Replace(CellId, Mixture),
    Release(CellId, Mixture),
    Context(CellId, Context, bool, bool),
    Neighbors(CellId, Vec<CellId>),
    ThermalNeighbors(CellId, [Option<CellId>; 4]),
    Topology(CellId, Vec<CellId>, [Option<CellId>; 4]),
    Terrain(CellId, Terrain),
    Conducting(CellId, bool),
    Commit(CellId, Mixture, Archive, bool),
    ArchiveMetadata(CellId, u32, f32),
    ReplaceFull(CellId, Mixture, Archive, bool),
    Expose(CellId, f32, f32, bool),
    ClearHotspot(CellId),
    Control(CellId, u8),
    Remove(CellId),
    Rebind(CellId, u32, Box<Rebinding>),
    HotspotState(CellId, Option<Hotspot>),
}

impl Write {
    fn publication(&self) -> Publication {
        match self {
            // Exposure can run reactions immediately, unlike importing the hotspot state.
            Self::Replace(..)
            | Self::Release(..)
            | Self::Commit(..)
            | Self::ReplaceFull(..)
            | Self::Expose(..)
            | Self::Rebind(..) => Publication::Full,
            Self::Context(..)
            | Self::Neighbors(..)
            | Self::ThermalNeighbors(..)
            | Self::Topology(..)
            | Self::Terrain(..)
            | Self::Conducting(..)
            | Self::ArchiveMetadata(..)
            | Self::ClearHotspot(..)
            | Self::Control(..)
            | Self::Remove(..)
            | Self::HotspotState(..) => Publication::Service,
        }
    }

    fn cell_id(&self) -> CellId {
        match self {
            Self::Replace(id, _)
            | Self::Release(id, _)
            | Self::Context(id, _, _, _)
            | Self::Neighbors(id, _)
            | Self::ThermalNeighbors(id, _)
            | Self::Topology(id, _, _)
            | Self::Terrain(id, _)
            | Self::Conducting(id, _)
            | Self::Commit(id, _, _, _)
            | Self::ArchiveMetadata(id, _, _)
            | Self::ReplaceFull(id, _, _, _)
            | Self::Expose(id, _, _, _)
            | Self::ClearHotspot(id)
            | Self::Control(id, _)
            | Self::Remove(id) => *id,
            Self::Rebind(id, _, _) | Self::HotspotState(id, _) => *id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Command {
    pub cell: CellId,
    pub binding_version: u32,
    pub effect: WorldCommand,
}

pub struct Job {
    epoch: u32,
    frame: Arc<Frame>,
    spare: Option<Frame>,
    sync_ids: BTreeSet<CellId>,
    resync_all: bool,
}

pub struct Completed {
    epoch: u32,
    frame: Frame,
    commands: Vec<Command>,
    changed: BTreeMap<CellId, Publication>,
}

impl Job {
    /// No BYOND references or callbacks are allowed in the worker's inputs.
    pub fn calculate(self) -> Completed {
        let mut frame = match (self.spare, self.resync_all) {
            (Some(mut spare), false) => {
                spare.synchronize(&self.frame, &self.sync_ids);
                spare
            }
            (spare, _) => {
                // Release the obsolete buffer before allocating: a third full frame can exhaust i686.
                drop(spare);
                (*self.frame).clone()
            }
        };
        let mut commands = Vec::new();
        commands.extend(
            frame
                .schedule
                .tick(&mut frame.cells, self.epoch)
                .into_iter()
                .map(|(cell, effect)| Command {
                    cell,
                    binding_version: self.frame.binding_versions[cell as usize],
                    effect,
                }),
        );
        frame.epoch = self.epoch;
        let changed = frame.changed_cells(&self.frame, frame.schedule.touched.iter().copied());
        Completed {
            epoch: self.epoch,
            frame,
            commands,
            changed,
        }
    }
}

impl Frame {
    fn synchronize(&mut self, source: &Self, ids: &BTreeSet<CellId>) {
        self.epoch = source.epoch;
        self.binding_versions
            .resize(source.binding_versions.len(), 0);
        self.cells.synchronize(&source.cells, ids.iter().copied());
        self.schedule
            .tiles
            .synchronize(&source.schedule.tiles, ids.iter().copied());
        for id in ids {
            self.binding_versions[*id as usize] = source.binding_versions[*id as usize];
        }
        self.schedule.copy_queues(&source.schedule);
    }

    fn changed_cells(
        &self,
        previous: &Self,
        candidates: impl Iterator<Item = CellId>,
    ) -> BTreeMap<CellId, Publication> {
        candidates
            .filter_map(|id| {
                let cell = &self.cells[&id];
                let old = &previous.cells[&id];
                let tile = &self.schedule.tiles[&id];
                let old_tile = &previous.schedule.tiles[&id];
                let air_changed = cell.air != old.air || tile.archived != old_tile.archived;
                let changed = air_changed
                    || cell.hotspot != old.hotspot
                    || cell.flags != old.flags
                    || tile.archived_cycle != old_tile.archived_cycle
                    || tile.current_cycle != old_tile.current_cycle
                    || tile.ticker != old_tile.ticker
                    || tile.excited != old_tile.excited
                    || tile.group != old_tile.group
                    || tile.pressure != old_tile.pressure
                    || tile.direction != old_tile.direction
                    || tile.terrain != old_tile.terrain
                    || (!matches!(tile.boundary, crate::transport::Boundary::Space)
                        && tile.last_share != old_tile.last_share);
                if !changed {
                    return None;
                }
                let publication = if !air_changed {
                    Publication::Service
                } else if tile.has_air
                    && old_tile.has_air
                    && !matches!(tile.boundary, crate::transport::Boundary::Space)
                    && !matches!(old_tile.boundary, crate::transport::Boundary::Space)
                    && same_live_air(&cell.air, &old.air)
                {
                    if tile.archived.same_bits(&old_tile.archived)
                        && tile.archived.temperature != 0.0
                        && tile
                            .archived
                            .keys()
                            .iter()
                            .all(|&gas| tile.archived.moles[gas] != 0.0)
                    {
                        Publication::Temperature
                    } else {
                        Publication::Thermal
                    }
                } else {
                    Publication::Full
                };
                Some((id, publication))
            })
            .collect()
    }
}

fn same_live_air(air: &Mixture, old: &Mixture) -> bool {
    // A legacy write can change the sign of zero without queuing a full publication.
    air.volume.to_bits() == old.volume.to_bits()
        && air.turf_heat_capacity == old.turf_heat_capacity
        && air.keys() == old.keys()
        && air.keys().iter().all(|&gas| {
            air.moles[gas] != 0.0 && air.moles[gas].to_bits() == old.moles[gas].to_bits()
        })
        && air
            .results
            .iter()
            .zip(old.results.iter())
            .all(|(value, previous)| {
                *value != Some(0.0) && value.map(f32::to_bits) == previous.map(f32::to_bits)
            })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    EffectsPending,
    Closed,
    StaleJob,
    MissingCell,
    IdentifierExhausted,
    InvalidTopology,
    DetachedBinding,
    StaleBinding,
    Operation(crate::operations::Error),
    Port(crate::pipenet::PoolError),
}

struct PendingWrite {
    write: Write,
    binding_version: u32,
}

pub struct Store {
    pub visibility: Option<crate::visibility::Visibility>,
    pub pipes: crate::pipenet::Pool,
    references: crate::pipenet::Pool<references::AirReference>,
    operation_edits: Option<BTreeMap<crate::operations::Target, (crate::transport::Sample, bool)>>,
    /// Air edited by DM equipment before the outer safe callback decides whether to publish it.
    #[cfg(feature = "byond")]
    pub(crate) scrubber_edits: Vec<(CellId, u32, Mixture, Archive)>,
    published: Arc<Frame>,
    spare: Option<Frame>,
    sync_ids: BTreeSet<CellId>,
    resync_all: bool,
    running: Option<u32>,
    staged: Option<Completed>,
    next_cell: CellId,
    writes: Vec<PendingWrite>,
    bindings: BTreeMap<CellId, Binding>,
    detached: BTreeSet<CellId>,
    coordinates: BTreeMap<[i32; 3], CellId>,
    changed: BTreeMap<CellId, Publication>,
    topology_dirty: BTreeSet<CellId>,
    commands: Vec<Command>,
    closed: bool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            visibility: None,
            pipes: crate::pipenet::Pool::default(),
            references: crate::pipenet::Pool::default(),
            operation_edits: None,
            #[cfg(feature = "byond")]
            scrubber_edits: Vec::new(),
            published: Arc::new(Frame::default()),
            spare: None,
            sync_ids: BTreeSet::new(),
            resync_all: true,
            running: None,
            staged: None,
            next_cell: 1,
            writes: Vec::new(),
            bindings: BTreeMap::new(),
            detached: BTreeSet::new(),
            coordinates: BTreeMap::new(),
            changed: BTreeMap::new(),
            topology_dirty: BTreeSet::new(),
            commands: Vec::new(),
            closed: false,
        }
    }
}

impl Store {
    pub fn snapshot(&self) -> Arc<Frame> {
        self.published.clone()
    }
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(feature = "byond")]
    pub(crate) fn machine_air(&self, id: CellId, version: u32) -> Option<(&Mixture, &Archive)> {
        if self.closed || self.busy() || self.published_binding_version(id) != Some(version) {
            return None;
        }
        let tile = self.published.schedule.tiles.get(&id)?;
        if !tile.has_air || tile.boundary.immutable() {
            return None;
        }
        if let Some((sample, _)) = self
            .operation_edits
            .as_ref()
            .and_then(|edits| edits.get(&crate::operations::Target::Cell { id, version }))
        {
            return Some((&sample.air, &sample.archived));
        }
        Some((&self.published.cells.get(&id)?.air, &tile.archived))
    }

    #[cfg(feature = "byond")]
    pub(crate) fn scrubber_air(&self, id: CellId, version: u32) -> Option<(&Mixture, &Archive)> {
        let published = self.machine_air(id, version)?;
        Some(
            self.scrubber_edits
                .iter()
                .find_map(|(cell, bound, air, archive)| {
                    (*cell == id && *bound == version).then_some((air, archive))
                })
                .unwrap_or(published),
        )
    }

    pub fn set_processing(
        &mut self,
        active: Vec<CellId>,
        conducting: Vec<CellId>,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        for ids in [&active, &conducting] {
            let mut seen = std::collections::BTreeSet::new();
            if ids
                .iter()
                .any(|id| !self.published.cells.contains_key(id) || !seen.insert(*id))
            {
                return Err(Error::InvalidTopology);
            }
        }
        if active
            .iter()
            .any(|id| !self.published.schedule.tiles[id].has_air)
        {
            return Err(Error::InvalidTopology);
        }
        let frame = Arc::make_mut(&mut self.published);
        let active_set: BTreeSet<_> = active.iter().copied().collect();
        let changed: Vec<_> = frame
            .schedule
            .tiles
            .iter()
            .filter_map(|(id, tile)| (tile.excited != active_set.contains(&id)).then_some(id))
            .collect();
        for id in changed {
            self.sync_ids.insert(id);
            frame.schedule.tiles.get_mut(&id).unwrap().excited = active_set.contains(&id);
        }
        frame.schedule.active = active;
        frame.schedule.conducting = conducting;
        Ok(())
    }

    pub fn track_max_share(&mut self, enabled: bool) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        let schedule = &mut Arc::make_mut(&mut self.published).schedule;
        if enabled {
            schedule.max_shares.get_or_insert_with(BTreeMap::new);
        } else {
            schedule.max_shares = None;
        }
        Ok(())
    }

    pub fn register(&mut self, mut cell: Cell, tile: Tile) -> Result<CellId, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        if self.next_cell > MAX_EXACT_DM_INTEGER {
            return Err(Error::IdentifierExhausted);
        }
        if self.coordinates.contains_key(&tile.coordinates) {
            return Err(Error::InvalidTopology);
        }
        let id = self.next_cell;
        self.next_cell += 1;
        self.coordinates.insert(tile.coordinates, id);
        self.topology_dirty.insert(id);
        self.topology_dirty.extend(tile.neighbors.iter().copied());
        self.topology_dirty
            .extend(tile.thermal_neighbors.iter().flatten().copied());
        self.sync_ids.insert(id);
        // Import numeric state, never callbacks from a calculation outside this store.
        cell.commands.clear();
        // Handles are never reused: a late write cannot target a replacement turf.
        let frame = Arc::make_mut(&mut self.published);
        frame.schedule.add_tile(id, tile);
        frame.cells.insert(id, cell);
        frame.cells.share_previous_page(id, Cell::same_bits);
        frame.binding_versions.resize(id as usize + 1, 0);
        frame.binding_versions[id as usize] = 1;
        self.bindings.insert(
            id,
            Binding {
                version: 1,
                attached: true,
            },
        );
        Ok(id)
    }

    #[cfg(any(feature = "byond", test))]
    pub(crate) fn write_environment(
        &mut self,
        id: CellId,
        context: Context,
        permafrost: bool,
        thermal: bool,
        terrain: Terrain,
    ) -> Result<(), Error> {
        if self.closed || self.busy() {
            self.write(Write::Context(id, context, permafrost, thermal))?;
            return self.write(Write::Terrain(id, terrain));
        }
        let cell = self.published.cells.get(&id).ok_or(Error::MissingCell)?;
        let tile = &self.published.schedule.tiles[&id];
        let context_changed = !cell.context.same_bits(&context)
            || cell.freeze_leaves_permafrost != permafrost
            || tile.thermal_enabled != thermal;
        let terrain_changed =
            !tile.terrain.same_bits(&terrain) || thermal != (terrain.conductivity != 0.0);
        if context_changed {
            self.write(Write::Context(id, context, permafrost, thermal))?;
        }
        if terrain_changed {
            self.write(Write::Terrain(id, terrain))?;
        }
        Ok(())
    }

    pub fn write(&mut self, write: Write) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if matches!(write, Write::ReplaceFull(..) | Write::Remove(_)) {
            let id = write.cell_id();
            if let Some(version) = self.published_binding_version(id) {
                self.retire_air_references(crate::operations::Target::Cell { id, version });
            }
        }
        if self.operation_edits.is_some() && self.intercept_operation_write(&write)? {
            return Ok(());
        }
        let id = write.cell_id();
        if !self.published.cells.contains_key(&id) {
            return Err(Error::MissingCell);
        }
        let binding = self.bindings.get(&id).ok_or(Error::MissingCell)?;
        if !binding.attached && !matches!(write, Write::Remove(_) | Write::Release(_, _)) {
            return Ok(());
        }
        let binding_version = binding.version;
        self.sync_ids.insert(id);
        if matches!(
            write,
            Write::Neighbors(_, _)
                | Write::ThermalNeighbors(_, _)
                | Write::Topology(_, _, _)
                | Write::Remove(_)
                | Write::Rebind(_, _, _)
        ) {
            self.topology_dirty.insert(id);
            let old = &self.published.schedule.tiles[&id];
            self.topology_dirty.extend(old.neighbors.iter().copied());
            self.topology_dirty
                .extend(old.thermal_neighbors.iter().flatten().copied());
            match &write {
                Write::Neighbors(_, ids) => self.topology_dirty.extend(ids.iter().copied()),
                Write::ThermalNeighbors(_, ids) => {
                    self.topology_dirty.extend(ids.iter().flatten().copied())
                }
                Write::Topology(_, gas, thermal) => {
                    self.topology_dirty.extend(gas.iter().copied());
                    self.topology_dirty
                        .extend(thermal.iter().flatten().copied());
                }
                _ => {}
            }
        }
        if let Write::Neighbors(_, neighbors) = &write {
            if neighbors
                .iter()
                .any(|v| *v == id || !self.published.cells.contains_key(v))
            {
                return Err(Error::InvalidTopology);
            }
        }
        if let Write::ThermalNeighbors(_, neighbors) = &write {
            if neighbors
                .iter()
                .flatten()
                .any(|v| *v == id || !self.published.cells.contains_key(v))
            {
                return Err(Error::InvalidTopology);
            }
        }
        if let Write::Topology(_, gas, thermal) = &write {
            if gas
                .iter()
                .chain(thermal.iter().flatten())
                .any(|v| *v == id || !self.published.cells.contains_key(v))
            {
                return Err(Error::InvalidTopology);
            }
        }
        if self.busy() {
            self.writes.push(PendingWrite {
                write,
                binding_version,
            });
        } else {
            let frame = Arc::make_mut(&mut self.published);
            let removed = matches!(write, Write::Remove(_));
            let publication = write.publication();
            let coordinates = frame.schedule.tiles[&id].coordinates;
            let visible = apply(frame, write, &mut self.sync_ids);
            self.commands
                .extend(drain_effects(frame, id, binding_version));
            if removed {
                self.bindings.remove(&id);
                self.detached.remove(&id);
                self.coordinates.remove(&coordinates);
            }
            if visible && frame.cells.contains_key(&id) {
                let pending = self.changed.entry(id).or_default();
                *pending = (*pending).max(publication);
            }
        }
        Ok(())
    }

    pub fn begin(&mut self) -> Result<Job, Error> {
        let cycle = self
            .published
            .epoch
            .checked_add(1)
            .ok_or(Error::IdentifierExhausted)?;
        self.begin_at(cycle)
    }

    /// The world supplies its real SSair cycle after any initialization pause.
    pub fn begin_at(&mut self, cycle: u32) -> Result<Job, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        if self.operation_edits.is_some() {
            return Err(Error::Busy);
        }
        if !self.commands.is_empty() {
            return Err(Error::EffectsPending);
        }
        if !self.detached.is_empty() {
            return Err(Error::DetachedBinding);
        }
        if !self.topology_dirty.is_empty() {
            self.published
                .schedule
                .validate_tiles(self.topology_dirty.iter().copied())
                .map_err(|_| Error::InvalidTopology)?;
            self.topology_dirty.clear();
        }
        if cycle > MAX_EXACT_DM_INTEGER {
            return Err(Error::IdentifierExhausted);
        }
        if cycle <= self.published.epoch {
            return Err(Error::StaleJob);
        }
        let epoch = cycle;
        self.running = Some(epoch);
        Ok(Job {
            epoch,
            frame: self.published.clone(),
            spare: self.spare.take(),
            sync_ids: std::mem::take(&mut self.sync_ids),
            resync_all: std::mem::replace(&mut self.resync_all, false),
        })
    }

    /// Keep the previous frame readable until DM explicitly enters its publication phase.
    pub fn stage(&mut self, completed: Completed) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.running != Some(completed.epoch) || self.staged.is_some() {
            return Err(Error::StaleJob);
        }
        self.staged = Some(completed);
        Ok(())
    }

    pub fn pending_cycle(&self) -> Option<u32> {
        self.running
    }

    pub fn ready(&self) -> bool {
        self.staged.is_some()
    }

    pub fn publish_ready(&mut self, cycle: u32) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.running.is_none() && self.published.epoch == cycle {
            return Ok(());
        }
        if self.running != Some(cycle) || self.staged.is_none() {
            return Err(Error::StaleJob);
        }
        let completed = self.staged.take().unwrap();
        self.finish(completed)
    }

    pub fn finish(&mut self, completed: Completed) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.running != Some(completed.epoch) {
            return Err(Error::StaleJob);
        }
        let mut frame = completed.frame;
        let mut commands = completed.commands;
        for (id, publication) in completed.changed {
            let pending = self.changed.entry(id).or_default();
            *pending = (*pending).max(publication);
        }
        self.sync_ids.extend(frame.schedule.touched.iter().copied());
        // External changes become visible only after the completed native calculation.
        for pending in self.writes.drain(..) {
            let id = pending.write.cell_id();
            let publication = pending.write.publication();
            if matches!(pending.write, Write::Remove(_)) {
                if let Some(tile) = frame.schedule.tiles.get(&id) {
                    self.coordinates.remove(&tile.coordinates);
                }
                self.bindings.remove(&id);
                self.detached.remove(&id);
            }
            let visible = apply(&mut frame, pending.write, &mut self.sync_ids);
            commands.extend(drain_effects(&mut frame, id, pending.binding_version));
            if visible && frame.cells.contains_key(&id) {
                let pending = self.changed.entry(id).or_default();
                *pending = (*pending).max(publication);
            }
        }
        self.commands = commands
            .into_iter()
            .filter(|c| {
                self.bindings
                    .get(&c.cell)
                    .is_some_and(|b| b.attached && b.version == c.binding_version)
            })
            .collect();
        let previous = std::mem::replace(&mut self.published, Arc::new(frame));
        // Externally held snapshots stay immutable. The usual BYOND path holds no Rust Arc.
        self.spare = Arc::try_unwrap(previous).ok();
        self.running = None;
        Ok(())
    }

    pub fn abort(&mut self) {
        self.staged = None;
        if self.running.take().is_some() {
            self.resync_all = true;
            let frame = Arc::make_mut(&mut self.published);
            for pending in self.writes.drain(..) {
                let id = pending.write.cell_id();
                let publication = pending.write.publication();
                let visible = apply(frame, pending.write, &mut self.sync_ids);
                self.commands
                    .extend(drain_effects(frame, id, pending.binding_version));
                if visible && frame.cells.contains_key(&id) {
                    let pending = self.changed.entry(id).or_default();
                    *pending = (*pending).max(publication);
                }
            }
            self.bindings.retain(|id, _| frame.cells.contains_key(id));
            self.detached.retain(|id| frame.cells.contains_key(id));
            self.coordinates
                .retain(|_, id| frame.cells.contains_key(id));
            self.commands.retain(|c| {
                self.bindings
                    .get(&c.cell)
                    .is_some_and(|b| b.attached && b.version == c.binding_version)
            });
        }
    }

    pub fn take_commands(&mut self) -> Result<Vec<Command>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        Ok(std::mem::take(&mut self.commands)
            .into_iter()
            .filter(|c| {
                self.bindings
                    .get(&c.cell)
                    .is_some_and(|b| b.attached && b.version == c.binding_version)
            })
            .collect())
    }

    pub fn pending_commands(&self) -> Result<&[Command], Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        Ok(&self.commands)
    }

    pub fn close(&mut self) {
        self.references = crate::pipenet::Pool::default();
        self.operation_edits = None;
        self.visibility = None;
        self.pipes = crate::pipenet::Pool::default();
        #[cfg(feature = "byond")]
        self.scrubber_edits.clear();
        self.closed = true;
        self.staged = None;
        self.running = None;
        self.commands.clear();
        self.writes.clear();
        self.bindings.clear();
        self.detached.clear();
        self.coordinates.clear();
        self.changed.clear();
        self.published = Arc::new(Frame::default());
        self.spare = None;
        self.sync_ids.clear();
    }

    pub fn cell_at(&self, coordinates: [i32; 3]) -> Option<CellId> {
        self.coordinates.get(&coordinates).copied()
    }

    pub fn take_changes(&mut self) -> Result<Vec<CellId>, Error> {
        Ok(self
            .take_publication_changes()?
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    pub fn take_publication_changes(&mut self) -> Result<Vec<(CellId, Publication)>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        Ok(std::mem::take(&mut self.changed)
            .into_iter()
            .filter(|(id, _)| self.published.cells.contains_key(id))
            .collect())
    }

    #[cfg(feature = "byond")]
    pub(crate) fn needs_full_publication(&self, id: CellId, includes_archive: bool) -> bool {
        let pending = self.changed.get(&id).copied().unwrap_or_default();
        pending == Publication::Full || (!includes_archive && pending == Publication::Thermal)
    }

    /// Startup topology touches every registered cell; it has no output to project yet.
    pub fn discard_changes(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        self.changed.clear();
        Ok(())
    }
}
