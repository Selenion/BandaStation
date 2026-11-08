//! A live observation follows one mixture, independently of its turf's replacement.
use super::*;
use crate::{operations::Target, pipenet::Handle, transport::Sample};

#[derive(Default)]
pub(super) struct AirReference {
    target: Option<Target>,
    retired: Option<Arc<Sample>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog::NITROGEN, operations::Operation, transport::Sample};

    fn fixture() -> (Store, Target) {
        let mut store = Store::default();
        let mut air = Mixture::new(333.0, 2500.0, true);
        air.set(NITROGEN, 100.0);
        let id = store
            .register(
                Cell::new(air.clone(), Context::default()),
                Tile::new([1, 1, 1], &air, false),
            )
            .unwrap();
        (store, Target::Cell { id, version: 1 })
    }

    #[test]
    fn live_read_observes_pending_edits_without_copying_and_rejects_released_handles() {
        let (mut store, target) = fixture();
        let before = store.snapshot();
        let first = store.acquire_air_reference(target).unwrap();
        let second = store.acquire_air_reference(target).unwrap();
        assert_ne!(first, second);
        assert!(store.references.get(first).unwrap().retired.is_none());
        assert!(Arc::ptr_eq(&before, &store.snapshot()));
        store.begin_operations().unwrap();
        store
            .operate(target, &[Operation::AdjustGas(NITROGEN, 250.0)], false)
            .unwrap();
        assert_eq!(
            store.query_air_reference(first).unwrap().0.total_moles(),
            350.0
        );
        assert_eq!(store.snapshot().cells[&1].air.total_moles(), 100.0);
        store.finish_operations(false, false).unwrap();
        store.release_air_reference(first).unwrap();
        assert!(store.release_air_reference(first).is_err());
        assert_eq!(store.air_reference_count(), 1);
        let replacement = store.acquire_air_reference(target).unwrap();
        assert_ne!(first, replacement);
        assert!(store.query_air_reference(first).is_none());
        assert_eq!(
            store.query_air_reference(second).unwrap().0.total_moles(),
            350.0
        );
    }

    #[test]
    fn replacement_shares_one_retired_sample_and_never_retargets_old_readers() {
        let (mut store, target) = fixture();
        let first = store.acquire_air_reference(target).unwrap();
        let second = store.acquire_air_reference(target).unwrap();
        store.begin_operations().unwrap();
        store
            .operate(target, &[Operation::AdjustGas(NITROGEN, 3.0)], false)
            .unwrap();
        let mut fresh = Mixture::new(444.0, 100.0, false);
        fresh.set(NITROGEN, 17.0);
        store
            .write(Write::ReplaceFull(1, fresh.clone(), fresh.archive(), false))
            .unwrap();
        assert!(Arc::ptr_eq(
            store
                .references
                .get(first)
                .unwrap()
                .retired
                .as_ref()
                .unwrap(),
            store
                .references
                .get(second)
                .unwrap()
                .retired
                .as_ref()
                .unwrap()
        ));
        assert_eq!(
            store.query_air_reference(first).unwrap().0.total_moles(),
            103.0
        );
        let current = store.acquire_air_reference(target).unwrap();
        assert_eq!(
            store.query_air_reference(current).unwrap().0.total_moles(),
            17.0
        );
        store.finish_operations(false, false).unwrap();
        let version = store.invalidate_binding(1).unwrap();
        assert_eq!(
            store.query_air_reference(first).unwrap().0.total_moles(),
            103.0
        );
        assert_eq!(
            store.query_air_reference(current).unwrap().0.total_moles(),
            17.0
        );
        assert_eq!(version, 2);
        let mut air = Mixture::new(555.0, 2500.0, true);
        air.set(NITROGEN, 7.0);
        store
            .rebind(
                1,
                version,
                Rebinding {
                    cell: Cell::new(air.clone(), Context::default()),
                    tile: Tile::new([1, 1, 1], &air, false),
                    inherit_group: true,
                    inherit_gas: false,
                },
            )
            .unwrap();
        let newest = store
            .acquire_air_reference(Target::Cell { id: 1, version })
            .unwrap();
        assert_eq!(
            store.query_air_reference(newest).unwrap().0.total_moles(),
            7.0
        );
        store.write(Write::Remove(1)).unwrap();
        assert_eq!(
            store.query_air_reference(newest).unwrap().0.total_moles(),
            7.0
        );
        assert_eq!(
            store.query_air_reference(first).unwrap().0.temperature,
            333.0
        );
        let retained = Arc::downgrade(
            store
                .references
                .get(first)
                .unwrap()
                .retired
                .as_ref()
                .unwrap(),
        );
        store.release_air_reference(first).unwrap();
        assert!(retained.upgrade().is_some());
        store.release_air_reference(second).unwrap();
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn port_release_and_worker_publication_keep_reference_identity_and_close_releases_all() {
        let (mut store, target) = fixture();
        let reference = store.acquire_air_reference(target).unwrap();
        let job = store.begin().unwrap();
        assert_eq!(
            store
                .query_air_reference(reference)
                .unwrap()
                .0
                .total_moles(),
            100.0
        );
        store.stage(job.calculate()).unwrap();
        store.publish_ready(1).unwrap();
        assert!(!store.query_air_reference(reference).unwrap().2);
        let handle = store
            .pipes
            .register(Sample::new(Mixture::new(777.0, 100.0, false)))
            .unwrap();
        let pipe = store.acquire_air_reference(Target::Port(handle)).unwrap();
        store.release_pipe(handle).unwrap();
        assert_eq!(
            store.query_air_reference(pipe).unwrap().0.temperature,
            777.0
        );
        for _ in 0..1000 {
            let handle = store.acquire_air_reference(target).unwrap();
            store.release_air_reference(handle).unwrap();
        }
        assert_eq!(store.references.slots(), 3);
        store.close();
        assert_eq!(store.air_reference_count(), 0);
        assert!(store.query_air_reference(reference).is_none());
        assert!(store.query_air_reference(pipe).is_none());
    }
}

impl Store {
    pub fn acquire_air_reference(&mut self, target: Target) -> Result<Handle, Error> {
        if self.query_air(target).is_none() {
            return Err(if self.closed {
                Error::Closed
            } else {
                Error::StaleBinding
            });
        }
        self.references
            .register(AirReference {
                target: Some(target),
                retired: None,
            })
            .map_err(Error::Port)
    }

    pub fn release_air_reference(&mut self, handle: Handle) -> Result<(), Error> {
        self.references.release(handle).map_err(Error::Port)
    }

    pub fn air_reference_count(&self) -> usize {
        self.references.len()
    }

    pub fn release_pipe(&mut self, handle: Handle) -> Result<(), Error> {
        self.retire_air_references(Target::Port(handle));
        self.pipes.release(handle).map_err(Error::Port)
    }

    /// The flag distinguishes a live mixture from a retained predecessor of the same coordinate.
    pub(crate) fn query_air_reference(
        &self,
        handle: Handle,
    ) -> Option<(&Mixture, &crate::transport::Boundary, bool)> {
        if self.closed {
            return None;
        }
        let reference = self.references.get(handle).ok()?;
        if let Some(sample) = &reference.retired {
            Some((&sample.air, &sample.boundary, true))
        } else {
            self.query_air(reference.target?)
                .map(|(air, boundary)| (air, boundary, false))
        }
    }

    pub(crate) fn air_reference_sample(&self, handle: Handle) -> Option<Sample> {
        let reference = self.references.get(handle).ok()?;
        if let Some(sample) = &reference.retired {
            Some((**sample).clone())
        } else {
            self.clone_air_sample(reference.target?)
        }
    }

    pub(super) fn clone_air_sample(&self, target: Target) -> Option<Sample> {
        self.query_air(target)?;
        if let Some((sample, _)) = self
            .operation_edits
            .as_ref()
            .and_then(|edits| edits.get(&target))
        {
            return Some(sample.clone());
        }
        match target {
            Target::Port(handle) => self.pipes.get(handle).ok().cloned(),
            Target::Cell { id, .. } => {
                let tile = &self.published.schedule.tiles[&id];
                Some(Sample {
                    air: self.published.cells[&id].air.clone(),
                    archived: tile.archived.clone(),
                    last_share: self.published.schedule.last_share(id),
                    boundary: tile.boundary.clone(),
                })
            }
        }
    }

    pub(super) fn retire_air_references(&mut self, target: Target) {
        if !self
            .references
            .iter()
            .any(|reference| reference.target == Some(target) && reference.retired.is_none())
        {
            return;
        }
        let Some(sample) = self.clone_air_sample(target) else {
            return;
        };
        let sample = Arc::new(sample);
        for reference in self.references.iter_mut() {
            if reference.target == Some(target) && reference.retired.is_none() {
                reference.retired = Some(sample.clone());
            }
        }
    }
}
