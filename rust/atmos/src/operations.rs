//! Ordered gas operations. Returned samples are debited before a caller can use them.
use crate::{
    catalog::*,
    cell::merge,
    quantize,
    transport::{Boundary, Sample},
    Context, Mixture, Outcome,
};

#[derive(Clone, Debug)]
pub enum Operation {
    Ensure(usize),
    Reset(usize),
    SetGas(usize, f32),
    AdjustGas(usize, f32),
    AdjustMany(Vec<(usize, f32)>),
    ClearCurrent,
    ClearArchive,
    ClearResults,
    SetArchivedGas(usize, f32),
    SetArchivedTemperature(f32),
    SetResult {
        index: usize,
        value: Option<f32>,
    },
    SetLastShare(f32),
    Convert {
        from: usize,
        to: usize,
        amount: f32,
    },
    SetTemperature(f32),
    Heat(f32),
    SetVolume(f32),
    Archive,
    Collect,
    Merge(Box<Mixture>),
    Copy {
        source: Box<Mixture>,
        ratio: f32,
        clear_archive: bool,
    },
    Take(f32),
    TakeRatio(f32),
    TakeSpecific {
        gas: usize,
        amount: f32,
    },
    TakeSpecificRatio {
        gas: usize,
        ratio: f32,
    },
    React(Context),
    Electrolyze(Context),
}

#[derive(Clone, Debug)]
pub enum PairOperation {
    Transfer(Operation),
    Share {
        own: f32,
        other: f32,
    },
    TemperatureShare {
        coefficient: f32,
        other_capacity: f32,
    },
    Equalize,
}

#[derive(Default, Debug)]
pub struct PairResult {
    pub removed: Option<Mixture>,
    pub removed_kind: u8,
    pub value: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidGas,
    NonFinite,
    InvalidVolume,
    InvalidTransfer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Cell { id: u32, version: u32 },
    Port(crate::pipenet::Handle),
}

#[derive(Default, Debug)]
pub struct ResultValue {
    pub removed: Option<Mixture>,
    pub removed_kind: u8,
    pub outcome: Outcome,
}

fn receipt_kind(boundary: &Boundary, removed: &mut Option<Mixture>) -> u8 {
    let Some(air) = removed else {
        return 0;
    };
    match boundary {
        Boundary::Space => {
            // Numerical receipts need vacuum capacity; the adapter separately restores the DM type.
            air.turf_heat_capacity = true;
            2
        }
        Boundary::Planetary(_) => 3,
        Boundary::Mutable => u8::from(air.turf_heat_capacity),
    }
}

pub(crate) fn valid_state(sample: &Sample) -> Result<(), Error> {
    finite(sample.air.temperature)?;
    finite(sample.air.volume)?;
    finite(sample.archived.temperature)?;
    for &amount in sample.air.moles.iter().chain(sample.archived.moles.iter()) {
        finite(amount)?;
    }
    Ok(())
}

/// A late validation/overflow error must not leave a partially applied command packet.
pub fn apply_batch(
    sample: &mut Sample,
    operations: &[Operation],
) -> Result<Vec<ResultValue>, Error> {
    for operation in operations {
        operation.validate()?;
    }
    let mut pending = sample.clone();
    let mut results = Vec::with_capacity(operations.len());
    for operation in operations {
        let result = operation.apply(&mut pending)?;
        valid_state(&pending)?;
        if let Some(removed) = &result.removed {
            finite(removed.temperature)?;
            for &amount in removed.moles.iter() {
                finite(amount)?;
            }
        }
        results.push(result);
    }
    *sample = pending;
    Ok(results)
}

fn finite(value: f32) -> Result<(), Error> {
    value.is_finite().then_some(()).ok_or(Error::NonFinite)
}
fn gas(value: usize) -> Result<(), Error> {
    (value < GAS_COUNT).then_some(()).ok_or(Error::InvalidGas)
}

impl Operation {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Ensure(key) | Self::Reset(key) => gas(*key),
            Self::SetGas(key, value) | Self::AdjustGas(key, value) => {
                gas(*key)?;
                finite(*value)
            }
            Self::SetArchivedGas(key, value) => {
                gas(*key)?;
                finite(*value)
            }
            Self::AdjustMany(values) => {
                for &(key, value) in values {
                    gas(key)?;
                    finite(value)?;
                }
                Ok(())
            }
            Self::SetResult { index, value } => {
                if *index >= REACTION_COUNT {
                    return Err(Error::InvalidGas);
                }
                value.map_or(Ok(()), finite)
            }
            Self::Convert { from, to, amount } => {
                gas(*from)?;
                gas(*to)?;
                finite(*amount)
            }
            Self::TakeSpecific { gas: key, amount } => {
                gas(*key)?;
                finite(*amount)
            }
            Self::TakeSpecificRatio { gas: key, ratio } => {
                gas(*key)?;
                finite(*ratio)
            }
            Self::SetVolume(value) => {
                finite(*value)?;
                (*value >= 0.0).then_some(()).ok_or(Error::InvalidVolume)
            }
            Self::SetTemperature(value)
            | Self::SetArchivedTemperature(value)
            | Self::SetLastShare(value)
            | Self::Heat(value)
            | Self::Take(value)
            | Self::TakeRatio(value) => finite(*value),
            Self::Merge(source) | Self::Copy { source, .. } => {
                finite(source.temperature)?;
                for &key in source.keys() {
                    gas(key)?;
                    finite(source.moles[key])?;
                }
                if let Self::Copy { ratio, .. } = self {
                    finite(*ratio)?;
                }
                Ok(())
            }
            Self::Archive
            | Self::Collect
            | Self::ClearCurrent
            | Self::ClearArchive
            | Self::ClearResults => Ok(()),
            Self::React(context) | Self::Electrolyze(context) => {
                finite(context.working_power)?;
                finite(context.supermatter_power)
            }
        }
    }

    /// Result-producing operations must run against current, exclusively owned state.
    pub fn returns_sample(&self) -> bool {
        matches!(
            self,
            Self::Take(_)
                | Self::TakeRatio(_)
                | Self::TakeSpecific { .. }
                | Self::TakeSpecificRatio { .. }
        )
    }

    pub fn apply(&self, sample: &mut Sample) -> Result<ResultValue, Error> {
        self.validate()?;
        let mut result = ResultValue::default();
        if sample.boundary.immutable() {
            match self {
                Self::Take(_) | Self::TakeRatio(_) if sample.boundary == Boundary::Space => {
                    result.removed = Some(Mixture::new(TCMB, CELL_VOLUME, false));
                }
                Self::Archive | Self::Merge(_) | Self::Copy { .. } | Self::React(_) => {}
                // Preserve immutable reset semantics for the remaining setters/removals.
                _ => {
                    let boundary = std::mem::take(&mut sample.boundary);
                    result = self.apply(sample)?;
                    sample.boundary = boundary;
                    if matches!(
                        self,
                        Self::SetGas(..)
                            | Self::AdjustGas(..)
                            | Self::AdjustMany(..)
                            | Self::Convert { .. }
                            | Self::Collect
                            | Self::Take(..)
                            | Self::TakeRatio(..)
                            | Self::TakeSpecific { .. }
                            | Self::TakeSpecificRatio { .. }
                            | Self::Electrolyze(_)
                    ) {
                        sample.view().garbage_collect();
                    }
                }
            }
            result.removed_kind = receipt_kind(&sample.boundary, &mut result.removed);
            return Ok(result);
        }
        match self {
            Self::Ensure(key) => {
                sample.air.set(*key, sample.air.moles[*key]);
                sample.archived.set(*key, sample.archived.moles[*key]);
            }
            Self::Reset(key) => {
                sample.air.set(*key, 0.0);
                sample.archived.set(*key, 0.0);
            }
            Self::SetGas(key, value) => {
                sample.air.set(*key, *value);
                sample.view().garbage_collect();
            }
            Self::AdjustGas(key, amount) => {
                let amount = quantize(*amount).max(-sample.air.moles[*key]);
                sample.air.set(*key, sample.air.moles[*key] + amount);
                sample.view().garbage_collect();
            }
            Self::AdjustMany(values) => {
                // adjust_multiple_gases has no per-value QUANTIZE and collects only after the loop.
                for &(key, amount) in values {
                    sample
                        .air
                        .set(key, (sample.air.moles[key] + amount).max(0.0));
                }
                sample.view().garbage_collect();
            }
            Self::ClearCurrent => {
                sample.air.moles.fill(0.0);
                sample.air.order.clear();
            }
            Self::ClearArchive => {
                sample.archived.moles.fill(0.0);
                sample.archived.order.clear();
            }
            Self::ClearResults => sample.air.results.fill(None),
            Self::SetArchivedGas(key, value) => sample.archived.set(*key, *value),
            Self::SetArchivedTemperature(value) => sample.archived.temperature = *value,
            Self::SetResult { index, value } => sample.air.results[*index] = *value,
            Self::SetLastShare(value) => sample.last_share = *value,
            Self::Convert { from, to, amount } => {
                let amount = quantize(*amount).max(0.0).min(sample.air.moles[*from]);
                sample.air.set(*from, sample.air.moles[*from] - amount);
                sample.air.set(*to, sample.air.moles[*to] + amount);
                sample.archived.set(*from, sample.archived.moles[*from]);
                sample.archived.set(*to, sample.archived.moles[*to]);
                sample.view().garbage_collect();
            }
            Self::SetTemperature(value) => sample.air.temperature = *value,
            Self::Heat(joules) => {
                let capacity = sample.air.heat_capacity();
                if capacity > 0.0 {
                    sample.air.temperature = (sample.air.temperature + joules / capacity).max(TCMB);
                }
            }
            Self::SetVolume(value) => sample.air.volume = *value,
            Self::Archive => sample.archive(),
            Self::Collect => sample.view().garbage_collect(),
            Self::Merge(source) => merge(&mut sample.air, source),
            Self::Copy {
                source,
                ratio,
                clear_archive,
            } => {
                sample.air.moles.fill(0.0);
                sample.air.order.clear();
                sample.air.temperature = source.temperature;
                if *clear_archive {
                    sample.archived.moles.fill(0.0);
                    sample.archived.order.clear();
                }
                for &key in source.keys() {
                    sample.air.set(key, source.moles[key] * ratio);
                    if *clear_archive {
                        sample.archived.set(key, 0.0);
                    }
                }
            }
            Self::Take(amount) => {
                let total = sample.air.total_moles();
                let amount = amount.min(total);
                if amount > 0.0 {
                    result.removed = Some(take_ratio(sample, amount / total));
                }
            }
            Self::TakeRatio(ratio) => {
                result.removed = Some(take_ratio(sample, *ratio));
            }
            Self::TakeSpecific { gas, amount } => {
                result.removed = take_specific(sample, *gas, *amount, false);
            }
            Self::TakeSpecificRatio { gas, ratio } => {
                if *ratio > 0.0 {
                    result.removed = take_specific(
                        sample,
                        *gas,
                        quantize(sample.air.moles[*gas] * ratio.min(1.0)),
                        true,
                    );
                }
            }
            Self::React(context) => {
                result.outcome = sample.air.react(*context);
                sample.view().garbage_collect();
            }
            Self::Electrolyze(context) => {
                for (index, _) in REACTIONS.iter().enumerate().filter(|(_, r)| r.phase == 0) {
                    let outcome = sample.air.react_one(index, *context);
                    result.outcome.flags |= outcome.flags;
                    result.outcome.effects.extend(outcome.effects);
                }
                sample.view().garbage_collect();
            }
        }
        result.removed_kind = receipt_kind(&sample.boundary, &mut result.removed);
        Ok(result)
    }
}

fn take_ratio(sample: &mut Sample, ratio: f32) -> Mixture {
    if ratio <= 0.0 {
        return Mixture::new(TCMB, sample.air.volume, false);
    }
    let mut removed = Mixture::new(
        sample.air.temperature,
        sample.air.volume,
        sample.air.turf_heat_capacity,
    );
    for index in 0..sample.air.keys().len() {
        let key = sample.air.keys()[index];
        // Quantization must never hand the receiver more than was debited from the source.
        let amount = quantize(sample.air.moles[key] * ratio.min(1.0))
            .max(0.0)
            .min(sample.air.moles[key].max(0.0));
        removed.set(key, amount);
        sample.air.moles[key] -= amount;
    }
    sample.view().garbage_collect();
    removed
}

fn take_specific(
    sample: &mut Sample,
    key: usize,
    amount: f32,
    allow_empty: bool,
) -> Option<Mixture> {
    let amount = amount.max(0.0).min(sample.air.moles[key].max(0.0));
    if amount <= 0.0 && !allow_empty {
        return None;
    }
    // remove_specific creates new type with its default volume, unlike remove(amount).
    let mut removed = Mixture::new(
        sample.air.temperature,
        CELL_VOLUME,
        sample.air.turf_heat_capacity,
    );
    removed.set(key, amount);
    sample.air.moles[key] -= amount;
    sample.view().garbage_collect();
    Some(removed)
}

/// Validation and withdrawal precede the credit; no stale DM mixture participates in the transfer.
pub fn transfer(
    source: &mut Sample,
    destination: &mut Sample,
    operation: &Operation,
) -> Result<Option<Mixture>, Error> {
    if !operation.returns_sample() {
        return Err(Error::InvalidTransfer);
    }
    let mut pending_source = source.clone();
    let mut pending_destination = destination.clone();
    let removed = operation.apply(&mut pending_source)?.removed;
    if let Some(removed) = &removed {
        Operation::Merge(Box::new(removed.clone())).apply(&mut pending_destination)?;
    }
    valid_state(&pending_source)?;
    valid_state(&pending_destination)?;
    *source = pending_source;
    *destination = pending_destination;
    Ok(removed)
}

pub fn apply_pair(
    source: &mut Sample,
    destination: &mut Sample,
    operation: &PairOperation,
) -> Result<PairResult, Error> {
    let mut left = source.clone();
    let mut right = destination.clone();
    let mut result = PairResult::default();
    match operation {
        PairOperation::Transfer(selector) => {
            result.removed = transfer(&mut left, &mut right, selector)?;
            result.removed_kind = receipt_kind(&left.boundary, &mut result.removed);
        }
        PairOperation::Share { own, other } => {
            finite(*own)?;
            finite(*other)?;
            result.value = left.share(&mut right, *own, *other);
        }
        PairOperation::TemperatureShare {
            coefficient,
            other_capacity,
        } => {
            finite(*coefficient)?;
            finite(*other_capacity)?;
            result.value = Some(left.view().temperature_share_capacity(
                &mut right.view(),
                *coefficient,
                *other_capacity,
            ));
        }
        PairOperation::Equalize => {
            if left.air.volume <= 0.0 || right.air.volume <= 0.0 {
                return Err(Error::InvalidVolume);
            }
            let mut changed = false;
            if (left.air.temperature - right.air.temperature).abs()
                > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
            {
                changed = true;
                let own = left.air.heat_capacity();
                let other = right.air.heat_capacity();
                let temperature =
                    (left.air.temperature * own + right.air.temperature * other) / (own + other);
                left.air.temperature = temperature;
                right.air.temperature = temperature;
            }
            let mut keys = left.air.keys().to_vec();
            for &key in right.air.keys() {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            let volume = left.air.volume + right.air.volume;
            for key in keys {
                Operation::Ensure(key).apply(&mut left)?;
                Operation::Ensure(key).apply(&mut right)?;
                if (left.air.moles[key] / left.air.volume - right.air.moles[key] / right.air.volume)
                    .abs()
                    > 0.1 / (R_IDEAL_GAS_EQUATION * left.air.temperature)
                {
                    changed = true;
                    let total = left.air.moles[key] + right.air.moles[key];
                    left.air.moles[key] = total * (left.air.volume / volume);
                    right.air.moles[key] = total * (right.air.volume / volume);
                }
            }
            left.view().garbage_collect();
            right.view().garbage_collect();
            result.value = Some(if changed { 1.0 } else { 0.0 });
        }
    }
    valid_state(&left)?;
    valid_state(&right)?;
    *source = left;
    *destination = right;
    Ok(result)
}
