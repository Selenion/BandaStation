//! Numeric overlay descriptors; actual images and plane offsets stay in DM.
use crate::catalog::GAS_COUNT;

#[derive(Clone, Debug)]
pub struct Visibility {
    minimums: [f32; GAS_COUNT],
    step: f32,
    maximum: u32,
}

impl Visibility {
    pub fn from_values(values: &[f32]) -> Result<Self, String> {
        if values.len() != GAS_COUNT + 2
            || values.iter().any(|n| !n.is_finite())
            || values[..GAS_COUNT].iter().any(|n| *n < 0.0 && *n != -1.0)
            || values[GAS_COUNT] <= 0.0
            || values[GAS_COUNT + 1] < 1.0
            || values[GAS_COUNT + 1] > 65535.0
            || values[GAS_COUNT + 1].fract() != 0.0
        {
            return Err("Invalid gas visibility rules".into());
        }
        Ok(Self {
            minimums: values[..GAS_COUNT].try_into().unwrap(),
            step: values[GAS_COUNT],
            maximum: values[GAS_COUNT + 1] as u32,
        })
    }

    pub fn state(&self, gas: usize, amount: f32) -> Option<u32> {
        let minimum = self.minimums[gas];
        (minimum >= 0.0 && amount > minimum)
            .then(|| (amount / self.step).ceil().min(self.maximum as f32) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_threshold_steps_hidden_gases_and_saturation_match_dm() {
        let mut data = vec![-1.0; GAS_COUNT];
        data[3] = 0.25;
        data[4] = 0.5;
        data.extend([0.25, 80.0]);
        let rules = Visibility::from_values(&data).unwrap();
        assert_eq!(rules.state(0, 1e30), None);
        assert_eq!(rules.state(3, 0.25), None);
        assert_eq!(
            rules.state(3, f32::from_bits(0.25_f32.to_bits() + 1)),
            Some(2)
        );
        assert_eq!(rules.state(3, 0.5), Some(2));
        assert_eq!(
            rules.state(3, f32::from_bits(0.5_f32.to_bits() + 1)),
            Some(3)
        );
        assert_eq!(rules.state(4, 0.5), None);
        assert_eq!(rules.state(3, 1e30), Some(80));
        data[GAS_COUNT] = 0.0;
        assert!(Visibility::from_values(&data).is_err());
        assert!(Visibility::from_values(&[]).is_err());
    }
}
