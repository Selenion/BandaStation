use super::{reconcile, Sample};

const MAX_EXACT: u32 = 16_777_215;

/// Slots are reused only with a new generation, both exactly representable in DM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Handle {
    pub slot: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    StaleHandle,
    Exhausted,
}

#[derive(Default)]
pub struct Pool<T = Sample> {
    samples: Vec<T>,
    generations: Vec<u32>,
    live: Vec<bool>,
    free: Vec<usize>,
    count: usize,
}

impl<T: Default> Pool<T> {
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn slots(&self) -> usize {
        self.samples.len()
    }

    fn index(&self, handle: Handle) -> Result<usize, PoolError> {
        let index = handle.slot.checked_sub(1).ok_or(PoolError::StaleHandle)? as usize;
        if self.live.get(index) != Some(&true) || self.generations[index] != handle.generation {
            return Err(PoolError::StaleHandle);
        }
        Ok(index)
    }

    pub fn register(&mut self, sample: T) -> Result<Handle, PoolError> {
        let index = if let Some(index) = self.free.pop() {
            self.samples[index] = sample;
            self.live[index] = true;
            index
        } else {
            if self.samples.len() >= MAX_EXACT as usize {
                return Err(PoolError::Exhausted);
            }
            let index = self.samples.len();
            self.samples.push(sample);
            self.generations.push(1);
            self.live.push(true);
            index
        };
        self.count += 1;
        Ok(Handle {
            slot: index as u32 + 1,
            generation: self.generations[index],
        })
    }

    pub fn get(&self, handle: Handle) -> Result<&T, PoolError> {
        Ok(&self.samples[self.index(handle)?])
    }

    pub fn replace(&mut self, handle: Handle, sample: T) -> Result<(), PoolError> {
        let index = self.index(handle)?;
        self.samples[index] = sample;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.samples
            .iter()
            .zip(&self.live)
            .filter_map(|(value, live)| live.then_some(value))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.samples
            .iter_mut()
            .zip(&self.live)
            .filter_map(|(value, live)| live.then_some(value))
    }

    pub fn release(&mut self, handle: Handle) -> Result<(), PoolError> {
        let index = self.index(handle)?;
        // Release gas/results allocations while retaining a bounded reusable slot table.
        self.samples[index] = T::default();
        self.live[index] = false;
        self.count -= 1;
        if self.generations[index] < MAX_EXACT {
            self.generations[index] += 1;
            self.free.push(index);
        }
        Ok(())
    }
}

impl Pool<Sample> {
    /// Reconciliation reads current gas only; DM still owns archives and reaction state.
    pub fn import_air(&mut self, handle: Handle, mut air: crate::Mixture) -> Result<(), PoolError> {
        let index = self.index(handle)?;
        std::mem::swap(&mut air.results, &mut self.samples[index].air.results);
        self.samples[index].air = air;
        Ok(())
    }

    pub fn reconcile(&mut self, handles: &[Handle]) -> Result<(), PoolError> {
        let indices: Result<Vec<_>, _> = handles.iter().map(|&h| self.index(h)).collect();
        // Every handle is checked before reconcile can modify any sample.
        reconcile(&mut self.samples, &indices?).expect("Validated pool indices");
        Ok(())
    }
}
