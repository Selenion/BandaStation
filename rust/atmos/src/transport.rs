//! BandaStation gas/heat exchange. Each operation reads the caller-provided archives.
use crate::{catalog::*, quantize, Archive, Mixture};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Boundary {
    #[default]
    Mutable,
    Space,
    Planetary(Box<Mixture>),
}

impl Boundary {
    pub fn heat_capacity(&self, air: &Mixture) -> f32 {
        if matches!(self, Self::Space) {
            HEAT_CAPACITY_VACUUM
        } else {
            air.heat_capacity()
        }
    }
    pub fn immutable(&self) -> bool {
        !matches!(self, Self::Mutable)
    }
    fn temperature(&self) -> f32 {
        match self {
            Self::Mutable => unreachable!(),
            Self::Space => TCMB,
            Self::Planetary(air) => air.temperature,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub air: Mixture,
    pub archived: Archive,
    pub last_share: f32,
    pub boundary: Boundary,
}

impl Sample {
    pub fn new(air: Mixture) -> Self {
        Self {
            archived: air.archive(),
            air,
            last_share: 0.0,
            boundary: Boundary::Mutable,
        }
    }

    pub fn archive(&mut self) {
        if !self.boundary.immutable() {
            self.archived = self.air.archive();
        }
    }

    pub fn view(&mut self) -> SampleView<'_> {
        SampleView {
            air: &mut self.air,
            archived: &mut self.archived,
            last_share: &mut self.last_share,
            boundary: &self.boundary,
        }
    }

    pub fn share(&mut self, other: &mut Self, own: f32, their: f32) -> Option<f32> {
        self.view().share(&mut other.view(), own, their)
    }

    pub fn temperature_share(&mut self, other: &mut Self, coefficient: f32) -> f32 {
        self.view()
            .temperature_share(&mut other.view(), coefficient)
    }
}

/// Borrow both cells in place so graph exchanges do not copy mixtures per boundary.
pub struct SampleView<'a> {
    pub air: &'a mut Mixture,
    pub archived: &'a mut Archive,
    pub last_share: &'a mut f32,
    pub boundary: &'a Boundary,
}

impl SampleView<'_> {
    pub fn garbage_collect(&mut self) {
        match self.boundary {
            Boundary::Space => {
                self.air.moles.fill(0.0);
                self.air.order.clear();
                self.archived.moles.fill(0.0);
                self.archived.order.clear();
                self.air.temperature = TCMB;
                self.archived.temperature = TCMB;
                return;
            }
            Boundary::Planetary(initial) => {
                self.air.moles.clone_from(&initial.moles);
                self.air.order.clone_from(&initial.order);
                self.archived.moles.clone_from(&initial.moles);
                self.archived.order.clone_from(&initial.order);
                self.air.temperature = initial.temperature;
                self.archived.temperature = initial.temperature;
                return;
            }
            Boundary::Mutable => {}
        }
        // DM's garbage_collect ignores the selective-list arguments passed by share().
        self.air.garbage_collect();
        self.archived.garbage_collect();
    }

    pub fn temperature_share(&mut self, other: &mut Self, coefficient: f32) -> f32 {
        self.temperature_share_capacity(other, coefficient, 0.0)
    }

    fn heat_capacity(&self, archived: bool) -> f32 {
        if matches!(self.boundary, Boundary::Space) {
            // tg/Banda override only current space capacity. The empty archive inherits the base proc.
            if archived {
                0.0
            } else {
                HEAT_CAPACITY_VACUUM
            }
        } else if archived {
            self.archived.heat_capacity()
        } else {
            self.boundary.heat_capacity(self.air)
        }
    }

    pub fn temperature_share_capacity(
        &mut self,
        other: &mut Self,
        coefficient: f32,
        capacity: f32,
    ) -> f32 {
        let mut temperature = other.archived.temperature;
        let delta = self.archived.temperature - temperature;
        if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
            let own_capacity = self.heat_capacity(true);
            let other_capacity = if capacity != 0.0 {
                capacity
            } else {
                other.heat_capacity(true)
            };
            if own_capacity > MINIMUM_HEAT_CAPACITY && other_capacity > MINIMUM_HEAT_CAPACITY {
                let heat = (coefficient * delta)
                    * (other_capacity * (own_capacity / (other_capacity + own_capacity)));
                self.air.temperature = (self.air.temperature - heat / own_capacity).max(TCMB);
                temperature = (temperature + heat / other_capacity).max(TCMB);
                other.air.temperature = temperature;
                if other.boundary.immutable() {
                    other.garbage_collect();
                }
            }
        }
        if self.boundary.immutable() {
            self.air.temperature = self.boundary.temperature();
        }
        temperature
    }

    pub fn temperature_share_solid(
        &mut self,
        coefficient: f32,
        temperature: f32,
        capacity: f32,
    ) -> f32 {
        let mut temperature = temperature;
        let delta = self.archived.temperature - temperature;
        let own_capacity = self.heat_capacity(true);
        if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
            && own_capacity > MINIMUM_HEAT_CAPACITY
            && capacity > MINIMUM_HEAT_CAPACITY
        {
            let heat =
                (coefficient * delta) * (capacity * (own_capacity / (capacity + own_capacity)));
            self.air.temperature = (self.air.temperature - heat / own_capacity).max(TCMB);
            temperature = (temperature + heat / capacity).max(TCMB);
        }
        if self.boundary.immutable() {
            self.air.temperature = self.boundary.temperature();
        }
        temperature
    }

    /// Coefficients account for each cell's number of connected neighbours, including multiz.
    /// None matches DM's null return when no pressure-movement condition is met.
    pub fn share(
        &mut self,
        other: &mut Self,
        own_coefficient: f32,
        other_coefficient: f32,
    ) -> Option<f32> {
        let result = self.share_base(other, own_coefficient, other_coefficient);
        if self.boundary.immutable() {
            other.air.temperature = self.boundary.temperature();
            self.garbage_collect();
        }
        result
    }

    fn share_base(
        &mut self,
        other: &mut Self,
        own_coefficient: f32,
        other_coefficient: f32,
    ) -> Option<f32> {
        let only_other: Vec<_> = other
            .air
            .keys()
            .iter()
            .copied()
            .filter(|g| !self.air.keys().contains(g))
            .collect();
        let only_own: Vec<_> = self
            .air
            .keys()
            .iter()
            .copied()
            .filter(|g| !other.air.keys().contains(g))
            .collect();
        let delta_temperature = self.archived.temperature - other.archived.temperature;
        let transfer_heat = delta_temperature.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER;
        let own_old_capacity = if transfer_heat {
            self.heat_capacity(false)
        } else {
            0.0
        };
        let other_old_capacity = if transfer_heat {
            other.heat_capacity(false)
        } else {
            0.0
        };
        for &gas in &only_other {
            self.air.set(gas, 0.0);
            self.archived.set(gas, 0.0);
        }
        for &gas in &only_own {
            other.air.set(gas, 0.0);
            other.archived.set(gas, 0.0);
        }
        let mut own_to_other = 0.0;
        let mut other_to_own = 0.0;
        let mut moved = 0.0;
        let mut absolute_moved = 0.0;
        #[cfg(feature = "simd")]
        // Packing all 20 lanes loses to scalar arithmetic for ordinary two-gas air.
        let packed = if self.air.order.len() > 2 {
            crate::simd::calculate(
                &self.archived.moles,
                &other.archived.moles,
                own_coefficient,
                other_coefficient,
            )
        } else {
            None
        };
        for &gas in &self.air.order {
            #[cfg(feature = "simd")]
            let mut delta = if let Some(packed) = &packed {
                if packed.active & (1 << gas) == 0 {
                    continue;
                }
                packed.deltas[gas]
            } else {
                quantize(self.archived.moles[gas] - other.archived.moles[gas])
            };
            #[cfg(not(feature = "simd"))]
            let mut delta = quantize(self.archived.moles[gas] - other.archived.moles[gas]);
            #[cfg(feature = "simd")]
            let needs_coefficient = packed.is_none();
            #[cfg(not(feature = "simd"))]
            let needs_coefficient = true;
            if needs_coefficient {
                if delta == 0.0 {
                    continue;
                }
                delta *= if delta > 0.0 {
                    own_coefficient
                } else {
                    other_coefficient
                };
            }
            if transfer_heat {
                let capacity = delta * HEATS[gas];
                if delta > 0.0 {
                    own_to_other += capacity;
                } else {
                    other_to_own -= capacity;
                }
            }
            self.air.moles[gas] -= delta;
            other.air.moles[gas] += delta;
            moved += delta;
            absolute_moved += delta.abs();
        }
        *self.last_share = absolute_moved;
        // Planetary GC replaces the DM lists. share()'s cached aliases still refer to the old lists.
        let own_cached_moles = if matches!(self.boundary, Boundary::Planetary(_)) {
            self.air.total_moles()
        } else {
            0.0
        };
        let other_cached_moles = if matches!(other.boundary, Boundary::Planetary(_)) {
            other.air.total_moles()
        } else {
            0.0
        };
        if transfer_heat {
            let own_new_capacity = own_old_capacity + other_to_own - own_to_other;
            let other_new_capacity = other_old_capacity + own_to_other - other_to_own;
            if own_new_capacity > MINIMUM_HEAT_CAPACITY {
                self.air.temperature = (own_old_capacity * self.air.temperature
                    - own_to_other * self.archived.temperature
                    + other_to_own * other.archived.temperature)
                    / own_new_capacity;
            }
            if other_new_capacity > MINIMUM_HEAT_CAPACITY {
                other.air.temperature = (other_old_capacity * other.air.temperature
                    - other_to_own * other.archived.temperature
                    + own_to_other * self.archived.temperature)
                    / other_new_capacity;
                if other_old_capacity.abs() > MINIMUM_HEAT_CAPACITY
                    && (other_new_capacity / other_old_capacity - 1.0).abs() < 0.1
                {
                    self.temperature_share(other, OPEN_HEAT_TRANSFER_COEFFICIENT);
                }
            }
        }
        if !only_other.is_empty() || !only_own.is_empty() {
            self.garbage_collect();
            other.garbage_collect();
        } else if other.boundary.immutable() {
            other.garbage_collect();
        }
        let own_total = if matches!(self.boundary, Boundary::Planetary(_)) {
            own_cached_moles
        } else {
            self.air.total_moles()
        };
        let other_total = if matches!(other.boundary, Boundary::Planetary(_)) {
            other_cached_moles
        } else {
            other.air.total_moles()
        };
        if delta_temperature > MINIMUM_TEMPERATURE_TO_MOVE
            || moved.abs() > MINIMUM_MOLES_DELTA_TO_MOVE
        {
            Some(
                (self.archived.temperature * (own_total + moved)
                    - other.archived.temperature * (other_total - moved))
                    * R_IDEAL_GAS_EQUATION
                    / self.air.volume,
            )
        } else {
            None
        }
    }
}
