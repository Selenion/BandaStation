//! Empty gas and archive arrays share a constant zero view without a heap allocation.
use crate::catalog::GAS_COUNT;
use std::fmt;
use std::ops::{Deref, DerefMut};

type Values = [f32; GAS_COUNT];
const ZERO: Values = [0.0; GAS_COUNT];

#[derive(Default)]
pub struct GasAmounts(Option<Box<Values>>);

impl Clone for GasAmounts {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }

    fn clone_from(&mut self, source: &Self) {
        self.0.clone_from(&source.0);
    }
}

impl GasAmounts {
    pub fn fill(&mut self, value: f32) {
        if value.to_bits() == 0 {
            self.0 = None;
        } else if let Some(values) = &mut self.0 {
            values.fill(value);
        } else {
            self.0 = Some(Box::new([value; GAS_COUNT]));
        }
    }
}

impl Deref for GasAmounts {
    type Target = Values;

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap_or(&ZERO)
    }
}

impl DerefMut for GasAmounts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.get_or_insert_with(|| Box::new(ZERO))
    }
}

impl PartialEq for GasAmounts {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            _ => self.deref() == other.deref(),
        }
    }
}

impl fmt::Debug for GasAmounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_arrays_are_implicit_and_writing_one_does_not_change_other_views() {
        let mut first = GasAmounts::default();
        let second = first.clone();
        assert!(first.0.is_none() && second.0.is_none());
        first[0] = 7.0;
        assert_eq!(second[0], 0.0);
        let mut snapshot = first.clone();
        snapshot[0] = 3.0;
        assert_eq!(first[0], 7.0);
        snapshot.fill(0.0);
        assert!(snapshot.0.is_none());
        assert_eq!(snapshot, second);
    }

    #[test]
    fn zero_allocation_history_is_invisible_and_signed_zero_is_preserved() {
        let mut values = GasAmounts::default();
        values[0] = 0.0;
        assert_eq!(values, GasAmounts::default());
        values.fill(-0.0);
        assert!(values.iter().all(|v| v.to_bits() == (-0.0_f32).to_bits()));
        let copy = values.clone();
        assert_eq!(copy[0].to_bits(), (-0.0_f32).to_bits());
    }

    #[test]
    fn sharing_empty_mixtures_does_not_materialize_gas_or_archive_arrays() {
        use crate::{transport::Sample, Mixture};
        let mut a = Sample::new(Mixture::new(293.15, 2500.0, true));
        let mut b = Sample::new(Mixture::new(293.15, 2500.0, true));
        assert_eq!(a.share(&mut b, 0.2, 0.2), None);
        assert!(a.air.moles.0.is_none() && b.air.moles.0.is_none());
        assert!(a.archived.moles.0.is_none() && b.archived.moles.0.is_none());
    }
}
