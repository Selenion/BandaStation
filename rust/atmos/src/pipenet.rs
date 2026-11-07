//! Numeric part of datum/pipeline.reconcile_air; DM supplies the original mixture order.
use crate::{mixture_heat_capacity, transport::Sample, Mixture};
use std::collections::BTreeSet;

#[path = "pipenet_pool.rs"]
mod pool;
pub use pool::{Handle, Pool, PoolError};

/// An invalid batch is rejected before any mixture is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingMixture(pub usize);

/// Reconcile unique mixtures in caller order. Reactions run separately on the owning pipeline.
pub fn reconcile(mixtures: &mut [Sample], order: &[usize]) -> Result<(), MissingMixture> {
    let mut seen = BTreeSet::new();
    let mut unique = Vec::with_capacity(order.len());
    for &id in order {
        if id >= mixtures.len() {
            return Err(MissingMixture(id));
        }
        if seen.insert(id) {
            unique.push(id);
        }
    }
    let mut total = Mixture::new(0.0, 0.0, false);
    let mut capacity = 0.0;
    let mut energy = 0.0;
    for &id in &unique {
        let air = &mixtures[id].air;
        total.volume += air.volume;
        // reconcile_air uses values_dot directly, including for empty turf mixtures.
        let heat = mixture_heat_capacity(&air.moles, false);
        for &gas in air.keys() {
            total.add(gas, air.moles[gas]);
        }
        capacity += heat;
        energy += air.temperature * heat;
    }
    if total.volume == 0.0 {
        return Ok(());
    }
    total.temperature = if capacity != 0.0 {
        energy / capacity
    } else {
        0.0
    };
    total.garbage_collect();
    for id in unique {
        let sample = &mut mixtures[id];
        if sample.boundary.immutable() {
            continue;
        }
        let ratio = sample.air.volume / total.volume;
        // copy_from_ratio does not reset archives/results or collect the scaled trace gases.
        sample.air.moles.clone_from(&total.moles);
        sample.air.order.clone_from(&total.order);
        sample.air.temperature = total.temperature;
        for &gas in total.keys() {
            sample.air.moles[gas] = total.moles[gas] * ratio;
        }
    }
    Ok(())
}
