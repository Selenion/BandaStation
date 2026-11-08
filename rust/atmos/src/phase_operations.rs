use super::*;
use crate::{
    operations::{self, Operation, PairOperation, PairResult, ResultValue, Target},
    transport::{Boundary, Sample},
};

impl Store {
    /// Queries borrow published or pending values; they never issue a mutable VM mixture.
    pub(crate) fn query_air(&self, target: Target) -> Option<(&crate::Mixture, &Boundary)> {
        if self.closed {
            return None;
        }
        let published = match target {
            Target::Cell { id, version } => {
                if self.published_binding_version(id) != Some(version) {
                    return None;
                }
                let tile = self.published.schedule.tiles.get(&id)?;
                if !tile.has_air {
                    return None;
                }
                (&self.published.cells.get(&id)?.air, &tile.boundary)
            }
            Target::Port(handle) => {
                let sample = self.pipes.get(handle).ok()?;
                (&sample.air, &sample.boundary)
            }
        };
        Some(
            self.operation_edits
                .as_ref()
                .and_then(|edits| edits.get(&target))
                .map_or(published, |(sample, _)| (&sample.air, &sample.boundary)),
        )
    }

    pub(crate) fn pending_operation_air(&self, id: CellId) -> Option<&Mixture> {
        self.pending_operation_sample(id).map(|sample| &sample.air)
    }

    pub(crate) fn pending_operation_sample(&self, id: CellId) -> Option<&Sample> {
        let version = self.published_binding_version(id)?;
        self.operation_edits
            .as_ref()?
            .get(&Target::Cell { id, version })
            .map(|(sample, _)| sample)
    }

    pub(super) fn intercept_operation_write(&mut self, write: &Write) -> Result<bool, Error> {
        if !matches!(
            write,
            Write::Commit(..) | Write::Release(..) | Write::Replace(..) | Write::ReplaceFull(..)
        ) {
            return Ok(false);
        }
        let id = write.cell_id();
        let Some(version) = self.published_binding_version(id) else {
            return Ok(false);
        };
        let key = Target::Cell { id, version };
        let Some((current, pending_wake)) = self
            .operation_edits
            .as_ref()
            .and_then(|edits| edits.get(&key))
        else {
            return Ok(false);
        };
        let mut sample = current.clone();
        let mut wake = *pending_wake;
        match write {
            Write::Commit(_, air, archive, active) => {
                if !sample.boundary.immutable() {
                    sample.air.moles = air.moles.clone();
                    sample.air.order = air.order.clone();
                    sample.air.temperature = air.temperature;
                    sample.air.results = air.results.clone();
                    sample.archived = archive.clone();
                    wake |= active;
                }
            }
            Write::Release(_, air) => {
                Operation::Merge(Box::new(air.clone()))
                    .apply(&mut sample)
                    .map_err(Error::Operation)?;
                wake = true;
            }
            Write::Replace(_, air) => {
                if !sample.boundary.immutable() {
                    Operation::Copy {
                        source: Box::new(air.clone()),
                        ratio: 1.0,
                        clear_archive: true,
                    }
                    .apply(&mut sample)
                    .map_err(Error::Operation)?;
                    wake = true;
                }
            }
            Write::ReplaceFull(_, air, archive, space) => {
                sample.air = air.clone();
                sample.archived = archive.clone();
                sample.boundary = if *space {
                    crate::transport::Boundary::Space
                } else {
                    crate::transport::Boundary::Mutable
                };
                sample.last_share = 0.0;
            }
            _ => return Ok(false),
        }
        operations::valid_state(&sample).map_err(Error::Operation)?;
        self.operation_edits
            .as_mut()
            .unwrap()
            .insert(key, (sample, wake));
        Ok(true)
    }

    fn validate_operation_target(&self, target: Target) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() {
            return Err(Error::Busy);
        }
        match target {
            Target::Port(handle) => {
                self.pipes.get(handle).map_err(Error::Port)?;
            }
            Target::Cell { id, version } => {
                if self.published_binding_version(id) != Some(version) {
                    return Err(Error::StaleBinding);
                }
                let tile = &self.published.schedule.tiles[&id];
                if !tile.has_air {
                    return Err(Error::MissingCell);
                }
            }
        }
        Ok(())
    }

    fn operation_sample(&self, target: Target) -> Result<Sample, Error> {
        self.validate_operation_target(target)?;
        self.clone_air_sample(target).ok_or(Error::StaleBinding)
    }

    // All addresses are checked before mutation; no DM callback can run while this store is locked.
    fn publish_operation_sample(&mut self, target: Target, sample: Sample, wake: bool) {
        if let Some(edits) = self.operation_edits.as_mut() {
            let pending_wake = edits.remove(&target).is_some_and(|(_, pending)| pending);
            edits.insert(target, (sample, wake || pending_wake));
            return;
        }
        match target {
            Target::Port(handle) => self
                .pipes
                .replace(handle, sample)
                .expect("Validated gas port"),
            Target::Cell { id, .. } => {
                let frame = Arc::make_mut(&mut self.published);
                let cell = frame.cells.get_mut(&id).unwrap();
                let tile = frame.schedule.tiles.get_mut(&id).unwrap();
                let boundary_changed = tile.boundary != sample.boundary;
                let changed = cell.air != sample.air
                    || tile.archived != sample.archived
                    || boundary_changed
                    || tile.last_share != sample.last_share;
                cell.air = sample.air;
                tile.archived = sample.archived;
                tile.last_share = sample.last_share;
                tile.boundary = sample.boundary;
                if boundary_changed {
                    frame.schedule.refresh_space_override(id);
                }
                if changed && wake {
                    frame.schedule.activate(id);
                }
                if changed {
                    let pending = self.changed.entry(id).or_default();
                    *pending = Publication::Full;
                }
                self.sync_ids.insert(id);
            }
        }
    }

    /// A whole packet is staged before publication. Invalid operations cannot leave a partial debit.
    pub fn operate(
        &mut self,
        target: Target,
        commands: &[Operation],
        wake: bool,
    ) -> Result<Vec<ResultValue>, Error> {
        let mut sample = self.operation_sample(target)?;
        if self.operation_edits.is_some()
            && commands.iter().any(|op| matches!(op, Operation::React(_)))
        {
            // World reaction effects need a matching transaction for hotspot/command state as well.
            return Err(Error::EffectsPending);
        }
        for command in commands {
            command.validate().map_err(Error::Operation)?;
        }
        let mut pending_cell = if commands.iter().any(|op| matches!(op, Operation::React(_))) {
            match target {
                Target::Cell { id, .. } => Some(self.published.cells[&id].clone()),
                Target::Port(_) => None,
            }
        } else {
            None
        };
        let mut result = Vec::with_capacity(commands.len());
        for command in commands {
            let outcome =
                if let (Operation::React(context), Some(cell)) = (command, pending_cell.as_mut()) {
                    // Cell::react also advances hotspot creation between reactions, matching world ticks.
                    std::mem::swap(&mut cell.air, &mut sample.air);
                    let previous = cell.context;
                    cell.context = *context;
                    let outcome = cell.react();
                    cell.context = previous;
                    std::mem::swap(&mut cell.air, &mut sample.air);
                    sample.view().garbage_collect();
                    ResultValue {
                        removed: None,
                        removed_kind: 0,
                        outcome,
                    }
                } else {
                    command.apply(&mut sample).map_err(Error::Operation)?
                };
            operations::valid_state(&sample).map_err(Error::Operation)?;
            result.push(outcome);
        }
        self.publish_operation_sample(target, sample, wake);
        if self.operation_edits.is_none() {
            if let (Target::Cell { id, version }, Some(pending)) = (target, pending_cell) {
                let frame = Arc::make_mut(&mut self.published);
                let cell = frame.cells.get_mut(&id).unwrap();
                let changed = cell.hotspot != pending.hotspot || cell.flags != pending.flags;
                cell.hotspot = pending.hotspot;
                cell.flags = pending.flags;
                cell.commands = pending.commands;
                if cell.hotspot.is_some() && !frame.schedule.hotspots.contains(&id) {
                    frame.schedule.hotspots.push(id);
                }
                if changed {
                    self.changed.insert(id, Publication::Full);
                }
                self.commands.extend(drain_effects(frame, id, version));
            }
        }
        Ok(result)
    }

    /// Both endpoints belong to one store, so readers can never observe just one half of a transfer.
    pub fn transfer_air(
        &mut self,
        source: Target,
        destination: Target,
        selector: &Operation,
        wake: bool,
    ) -> Result<Option<Mixture>, Error> {
        if source == destination {
            return Err(Error::Operation(operations::Error::InvalidTransfer));
        }
        Ok(self
            .operate_pair(
                source,
                destination,
                &PairOperation::Transfer(selector.clone()),
                wake,
            )?
            .removed)
    }

    pub fn operate_pair(
        &mut self,
        source: Target,
        destination: Target,
        operation: &PairOperation,
        wake: bool,
    ) -> Result<PairResult, Error> {
        if source == destination {
            return Err(Error::Operation(operations::Error::InvalidTransfer));
        }
        let mut debit = self.operation_sample(source)?;
        let mut credit = self.operation_sample(destination)?;
        let result =
            operations::apply_pair(&mut debit, &mut credit, operation).map_err(Error::Operation)?;
        self.publish_operation_sample(source, debit, wake);
        self.publish_operation_sample(destination, credit, wake);
        Ok(result)
    }

    pub fn begin_operations(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.busy() || self.operation_edits.is_some() {
            return Err(Error::Busy);
        }
        self.operation_edits = Some(BTreeMap::new());
        Ok(())
    }

    pub fn finish_operations(&mut self, failed: bool, wake: bool) -> Result<(), Error> {
        let edits = self.operation_edits.take().ok_or(Error::Busy)?;
        if failed {
            return Ok(());
        }
        // A stale last endpoint must be detected before publishing the first endpoint.
        for &target in edits.keys() {
            self.validate_operation_target(target)?;
        }
        for (target, (sample, pending_wake)) in edits {
            self.publish_operation_sample(target, sample, wake || pending_wake);
        }
        Ok(())
    }

    /// A legacy borrower takes ownership of the pending state until its normal callback commits.
    pub fn take_operation_sample(&mut self, target: Target) -> Result<Option<Sample>, Error> {
        self.validate_operation_target(target)?;
        Ok(self
            .operation_edits
            .as_mut()
            .and_then(|edits| edits.remove(&target))
            .map(|(sample, _)| sample))
    }
}
