pub mod storage;

use bandastation_atmos::catalog::*;
use storage::Storage;

#[derive(Clone, Copy, Default)]
pub struct Order {
    keys: [u8; GAS_COUNT],
    len: u8,
    mask: u32,
}

impl Order {
    pub fn keys(&self) -> &[u8] {
        &self.keys[..self.len as usize]
    }
    fn contains(&self, gas: usize) -> bool {
        self.mask & (1 << gas) != 0
    }
    fn add(&mut self, gas: usize) {
        if !self.contains(gas) {
            self.keys[self.len as usize] = gas as u8;
            self.len += 1;
            self.mask |= 1 << gas;
        }
    }
}

#[derive(Clone, Copy)]
pub struct Metadata {
    pub order: Order,
    pub archived_order: Order,
    pub turf: bool,
    pub volume: f32,
    pub last_share: f32,
}

#[derive(Clone)]
pub struct World<S> {
    pub current: S,
    pub archived: S,
    pub metadata: Vec<Metadata>,
}

impl<S: Storage> World<S> {
    pub fn new(count: usize) -> Self {
        Self {
            current: S::new(count),
            archived: S::new(count),
            metadata: vec![
                Metadata {
                    order: Order::default(),
                    archived_order: Order::default(),
                    turf: true,
                    volume: CELL_VOLUME,
                    last_share: 0.0,
                };
                count
            ],
        }
    }
    pub fn set_gas(&mut self, id: usize, gas: usize, amount: f32) {
        assert!(gas < GAS_COUNT);
        self.metadata[id].order.add(gas);
        self.current.set(id, gas, amount);
    }
    pub fn archive(&mut self, id: usize) {
        for gas in 0..GAS_COUNT {
            self.archived.set(id, gas, self.current.get(id, gas));
        }
        self.archived
            .set_temperature(id, self.current.temperature(id));
        self.metadata[id].archived_order = self.metadata[id].order;
    }
    pub fn bytes(&self) -> usize {
        self.current.bytes()
            + self.archived.bytes()
            + self.metadata.capacity() * std::mem::size_of::<Metadata>()
    }
    fn heat_capacity(&self, id: usize, archived: bool) -> f32 {
        let storage = if archived {
            &self.archived
        } else {
            &self.current
        };
        // Preserve BYOND's reviewed dot-product reduction; SIMD lanes must not reorder it.
        let capacity = HEATS.iter().enumerate().fold(0.0, |sum, (gas, &heat)| {
            storage.get(id, gas).mul_add(heat, sum)
        });
        if capacity == 0.0 && self.metadata[id].turf {
            HEAT_CAPACITY_VACUUM
        } else {
            capacity
        }
    }
    fn total(&self, id: usize) -> f32 {
        // Retain the existing kernel's approved pressure tolerance relative to values_sum.
        self.metadata[id]
            .order
            .keys()
            .iter()
            .map(|&gas| self.current.get(id, gas as usize) as f64)
            .sum::<f64>() as f32
    }
    fn collect(&mut self, id: usize, archived: bool) {
        let storage = if archived {
            &mut self.archived
        } else {
            &mut self.current
        };
        let order = if archived {
            &mut self.metadata[id].archived_order
        } else {
            &mut self.metadata[id].order
        };
        let previous = *order;
        *order = Order::default();
        for &gas in previous.keys() {
            if storage.get(id, gas as usize) > MOLAR_ACCURACY {
                order.add(gas as usize);
            }
        }
        for gas in 0..GAS_COUNT {
            if !order.contains(gas) {
                storage.set(id, gas, 0.0);
            }
        }
    }
    pub fn temperature_share(&mut self, a: usize, b: usize, coefficient: f32) -> f32 {
        let mut temperature = self.archived.temperature(b);
        let delta = self.archived.temperature(a) - temperature;
        if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
            let ca = self.heat_capacity(a, true);
            let cb = self.heat_capacity(b, true);
            if ca > MINIMUM_HEAT_CAPACITY && cb > MINIMUM_HEAT_CAPACITY {
                let heat = (coefficient * delta) * (cb * (ca / (cb + ca)));
                self.current
                    .set_temperature(a, (self.current.temperature(a) - heat / ca).max(TCMB));
                temperature = (temperature + heat / cb).max(TCMB);
                self.current.set_temperature(b, temperature);
            }
        }
        temperature
    }
    pub fn share(&mut self, a: usize, b: usize, ca: f32, cb: f32) -> Option<f32> {
        assert_ne!(a, b);
        let own = self.metadata[a].order;
        let other = self.metadata[b].order;
        let different_keys = own.mask != other.mask;
        let temperature_delta = self.archived.temperature(a) - self.archived.temperature(b);
        let transfer_heat = temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER;
        let old_ca = if transfer_heat {
            self.heat_capacity(a, false)
        } else {
            0.0
        };
        let old_cb = if transfer_heat {
            self.heat_capacity(b, false)
        } else {
            0.0
        };
        // Add missing keys in caller/list order, before changing either mixture.
        for &gas in other.keys() {
            if !own.contains(gas as usize) {
                self.set_gas(a, gas as usize, 0.0);
                self.archived.set(a, gas as usize, 0.0);
                self.metadata[a].archived_order.add(gas as usize);
            }
        }
        for &gas in own.keys() {
            if !other.contains(gas as usize) {
                self.set_gas(b, gas as usize, 0.0);
                self.archived.set(b, gas as usize, 0.0);
                self.metadata[b].archived_order.add(gas as usize);
            }
        }
        let order = self.metadata[a].order;
        let deltas = self
            .current
            .transfer(&self.archived, a, b, order.keys(), ca, cb);
        let mut own_to_other = 0.0;
        let mut other_to_own = 0.0;
        let mut moved = 0.0;
        let mut absolute_moved = 0.0;
        // Ordered reductions stay scalar even when lane arithmetic is vectorized.
        for &gas in order.keys() {
            let delta = deltas[gas as usize];
            if transfer_heat {
                let capacity = delta * HEATS[gas as usize];
                if delta > 0.0 {
                    own_to_other += capacity;
                } else {
                    other_to_own -= capacity;
                }
            }
            moved += delta;
            absolute_moved += delta.abs();
        }
        self.metadata[a].last_share = absolute_moved;
        if transfer_heat {
            let new_ca = old_ca + other_to_own - own_to_other;
            let new_cb = old_cb + own_to_other - other_to_own;
            if new_ca > MINIMUM_HEAT_CAPACITY {
                let temperature = (old_ca * self.current.temperature(a)
                    - own_to_other * self.archived.temperature(a)
                    + other_to_own * self.archived.temperature(b))
                    / new_ca;
                self.current.set_temperature(a, temperature);
            }
            if new_cb > MINIMUM_HEAT_CAPACITY {
                let temperature = (old_cb * self.current.temperature(b)
                    - other_to_own * self.archived.temperature(b)
                    + own_to_other * self.archived.temperature(a))
                    / new_cb;
                self.current.set_temperature(b, temperature);
                if old_cb.abs() > MINIMUM_HEAT_CAPACITY && (new_cb / old_cb - 1.0).abs() < 0.1 {
                    self.temperature_share(a, b, OPEN_HEAT_TRANSFER_COEFFICIENT);
                }
            }
        }
        if different_keys {
            self.collect(a, false);
            self.collect(a, true);
            self.collect(b, false);
            self.collect(b, true);
        }
        if temperature_delta > MINIMUM_TEMPERATURE_TO_MOVE
            || moved.abs() > MINIMUM_MOLES_DELTA_TO_MOVE
        {
            Some(
                (self.archived.temperature(a) * (self.total(a) + moved)
                    - self.archived.temperature(b) * (self.total(b) - moved))
                    * R_IDEAL_GAS_EQUATION
                    / self.metadata[a].volume,
            )
        } else {
            None
        }
    }
}
