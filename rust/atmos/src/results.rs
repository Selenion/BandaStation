//! Most mixtures, including all canonical space, have no reaction results.
use crate::catalog::REACTION_COUNT;
use std::ops::{Index, IndexMut};

type Values = [Option<f32>; REACTION_COUNT];
const EMPTY: Values = [None; REACTION_COUNT];

#[derive(Clone, Debug, Default)]
pub struct ReactionResults(Option<Box<Values>>);

impl ReactionResults {
    fn values(&self) -> &Values {
        self.0.as_deref().unwrap_or(&EMPTY)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Option<f32>> {
        self.values().iter()
    }

    pub fn fill(&mut self, value: Option<f32>) {
        if let Some(values) = &mut self.0 {
            // Reuse the array for ongoing fire instead of allocating it every reaction tick.
            values.fill(value);
        } else if value.is_some() {
            self.0 = Some(Box::new([value; REACTION_COUNT]));
        }
    }
}

impl PartialEq for ReactionResults {
    fn eq(&self, other: &Self) -> bool {
        // Allocation history must not make a numerically unchanged cell appear changed.
        self.values() == other.values()
    }
}

impl Index<usize> for ReactionResults {
    type Output = Option<f32>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.values()[index]
    }
}

impl IndexMut<usize> for ReactionResults {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0.get_or_insert_with(|| Box::new(EMPTY))[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_results_need_no_array_and_distinguish_present_zero() {
        let mut results = ReactionResults::default();
        results.fill(None);
        assert!(results.0.is_none());
        assert!(results.iter().all(Option::is_none));
        results[2] = Some(0.0);
        assert_eq!(results[2], Some(0.0));
        assert_eq!(results[3], None);
        assert_ne!(results, ReactionResults::default());
    }

    #[test]
    fn snapshots_are_independent_and_clearing_preserves_value_equality() {
        let mut original = ReactionResults::default();
        original[0] = Some(7.0);
        let mut copy = original.clone();
        copy[0] = Some(-3.0);
        assert_eq!(original[0], Some(7.0));
        assert_eq!(copy[0], Some(-3.0));
        copy.fill(None);
        assert!(copy.0.is_some());
        assert_eq!(copy, ReactionResults::default());
        copy.fill(Some(123.0));
        assert!(copy.iter().all(|value| *value == Some(123.0)));
    }
}
