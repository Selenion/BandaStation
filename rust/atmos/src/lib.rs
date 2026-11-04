//! Experimental port of BandaStation gas chemistry, exchange and cell physics.
//! World effects are returned to the caller; this crate never owns BYOND objects.

#[rustfmt::skip]
pub mod catalog;
mod amounts;
pub use amounts::GasAmounts;
#[cfg(feature = "byond")]
mod byond;
pub mod cell;
pub mod network;
pub mod phase;
mod reactions;
mod results;
#[cfg(any(feature = "simd", test))]
mod simd;
pub mod storage;
pub use results::ReactionResults;
pub mod thermal;
pub mod transport;
use catalog::*;

pub const REACTING_FLAG: u8 = 1;
pub const STOP_FLAG: u8 = 2;
pub const VOLATILE_FLAG: u8 = 4;

/// A resolved holder. Pipeline variants refer to the member selected by DM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    None,
    Atom,
    Turf,
    OpenTurf,
    PipelineAtom,
    PipelineOpenTurf,
}

/// Environmental answers supplied by DM, including explicit random outcomes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Context {
    pub holder: Holder,
    pub freeze_succeeds: bool,
    pub permafrost: bool,
    pub groundless: bool,
    pub no_slip: bool,
    pub foam: bool,
    pub resin: bool,
    pub radiation_roll: bool,
    pub hot_ice_roll: bool,
    pub defer_random: bool,
    pub working_power: f32,
    pub supermatter_power: f32,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            holder: Holder::None,
            freeze_succeeds: false,
            permafrost: false,
            groundless: false,
            no_slip: false,
            foam: false,
            resin: false,
            radiation_roll: false,
            hot_ice_roll: false,
            defer_random: false,
            working_power: 1.0,
            supermatter_power: 0.0,
        }
    }
}

impl Context {
    pub(crate) fn same_bits(&self, other: &Self) -> bool {
        self == other
            && self.working_power.to_bits() == other.working_power.to_bits()
            && self.supermatter_power.to_bits() == other.supermatter_power.to_bits()
    }
    fn turf(self) -> bool {
        matches!(self.holder, Holder::Turf | Holder::OpenTurf)
    }
    fn open(self) -> bool {
        self.holder == Holder::OpenTurf
    }
    fn resolved_open(self) -> bool {
        matches!(self.holder, Holder::OpenTurf | Holder::PipelineOpenTurf)
    }
    fn location(self) -> bool {
        self.holder != Holder::None
    }
}

/// Call intents, in DM call order. Parameters use kelvins, litres and deciseconds.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Freeze,
    Wet,
    Hotspot { temperature: f32, volume: f32 },
    Radiation { range: f32, threshold: f32 },
    HotIce,
    RadiationChance { range: f32, threshold: f32 },
    HotIceChance,
    Resin { volume: f32 },
    NuclearParticle,
    Hallucination { radius: f32, duration: f32 },
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub flags: u8,
    pub effects: Vec<Effect>,
}

/// Ordered keys are part of the DM scheduling contract, including zero keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Mixture {
    pub moles: GasAmounts,
    pub temperature: f32,
    pub volume: f32,
    pub turf_heat_capacity: bool,
    pub results: ReactionResults,
    order: Vec<usize>,
}

/// DM archives only gas amounts and temperature, never reaction results or gas volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Archive {
    pub moles: GasAmounts,
    pub temperature: f32,
    pub turf_heat_capacity: bool,
    order: Vec<usize>,
}

impl Archive {
    pub(crate) fn same_bits(&self, other: &Self) -> bool {
        self == other
            && self.temperature.to_bits() == other.temperature.to_bits()
            && self
                .moles
                .iter()
                .zip(other.moles.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
    }
    fn copy_from(&mut self, air: &Mixture) {
        self.moles.clone_from(&air.moles);
        self.order.clone_from(&air.order);
        self.temperature = air.temperature;
        self.turf_heat_capacity = air.turf_heat_capacity;
    }

    pub fn empty(turf_heat_capacity: bool) -> Self {
        Self {
            moles: GasAmounts::default(),
            temperature: TCMB,
            turf_heat_capacity,
            order: Vec::new(),
        }
    }
    pub fn keys(&self) -> &[usize] {
        &self.order
    }
    pub fn set(&mut self, gas: usize, amount: f32) {
        if !self.order.contains(&gas) {
            self.order.push(gas);
        }
        self.moles[gas] = amount;
    }
    pub fn total_moles(&self) -> f32 {
        sum_moles(&self.moles, &self.order)
    }
    pub fn heat_capacity(&self) -> f32 {
        mixture_heat_capacity(&self.moles, self.turf_heat_capacity)
    }
    pub fn garbage_collect(&mut self) {
        collect_gases(&mut self.moles, &mut self.order);
    }
}

fn sum_moles(moles: &[f32; GAS_COUNT], order: &[usize]) -> f32 {
    order.iter().map(|&g| moles[g] as f64).sum::<f64>() as f32
}

fn mixture_heat_capacity(moles: &[f32; GAS_COUNT], turf: bool) -> f32 {
    // Match BYOND 516 values_dot: metadata order and one rounding per multiply-add.
    // Other accumulation orders/rounding can change temperature and reaction branches.
    let capacity = moles
        .iter()
        .zip(HEATS.iter())
        .fold(0.0, |sum, (&moles, &heat)| moles.mul_add(heat, sum));
    if capacity == 0.0 && turf {
        HEAT_CAPACITY_VACUUM
    } else {
        capacity
    }
}

fn collect_gases(moles: &mut GasAmounts, order: &mut Vec<usize>) {
    order.retain(|&g| moles[g] > MOLAR_ACCURACY);
    if order.is_empty() {
        moles.fill(0.0);
        return;
    }
    for (gas, amount) in moles.iter_mut().enumerate() {
        if !order.contains(&gas) {
            *amount = 0.0;
        }
    }
}

impl Mixture {
    pub fn new(temperature: f32, volume: f32, turf_heat_capacity: bool) -> Self {
        Self {
            moles: GasAmounts::default(),
            temperature,
            volume,
            turf_heat_capacity,
            results: ReactionResults::default(),
            order: Vec::new(),
        }
    }

    pub fn keys(&self) -> &[usize] {
        &self.order
    }

    pub fn set(&mut self, gas: usize, value: f32) {
        if !self.order.contains(&gas) {
            self.order.push(gas);
        }
        self.moles[gas] = value;
    }

    fn add(&mut self, gas: usize, amount: f32) {
        self.set(gas, self.moles[gas] + amount);
    }

    pub fn total_moles(&self) -> f32 {
        sum_moles(&self.moles, &self.order)
    }

    pub fn heat_capacity(&self) -> f32 {
        mixture_heat_capacity(&self.moles, self.turf_heat_capacity)
    }

    pub fn archive(&self) -> Archive {
        Archive {
            moles: self.moles.clone(),
            temperature: self.temperature,
            turf_heat_capacity: self.turf_heat_capacity,
            order: self.order.clone(),
        }
    }

    fn pressure(&self) -> f32 {
        self.total_moles() * R_IDEAL_GAS_EQUATION * self.temperature / self.volume
    }

    pub fn garbage_collect(&mut self) {
        collect_gases(&mut self.moles, &mut self.order);
    }

    fn adjust(&mut self, gas: usize, amount: f32) {
        self.add(gas, quantize(amount));
        self.garbage_collect();
    }

    fn energy(&mut self, old_capacity: f32, delta: f32, clamp: bool) {
        let capacity = self.heat_capacity();
        if capacity > MINIMUM_HEAT_CAPACITY {
            let temperature = (self.temperature * old_capacity + delta) / capacity;
            self.temperature = if clamp {
                temperature.max(TCMB)
            } else {
                temperature
            };
        }
    }

    /// Run a single reaction with its requirements, without global suppression/GC.
    pub fn react_one(&mut self, index: usize, context: Context) -> Outcome {
        if !self.eligible(index, self.temperature, context) {
            return Outcome::default();
        }
        self.apply(index, context)
    }

    fn eligible(&self, index: usize, temperature: f32, context: Context) -> bool {
        let reaction = &REACTIONS[index];
        temperature >= reaction.min_temp
            && temperature <= reaction.max_temp
            && reaction
                .requirements
                .iter()
                .all(|&(g, n)| self.moles[g] >= n)
            && (reaction.id != "nob_conversion"
                || context.supermatter_power > POWER_PENALTY_THRESHOLD)
    }

    /// Match gas_mixture.react(): snapshot candidates and temperature, live quantities.
    pub fn react(&mut self, context: Context) -> Outcome {
        self.react_with(context, &mut |_, _| {})
    }

    /// Native environment effects may change this mixture before the next candidate runs.
    /// The hook handles those numerical consequences inside Rust, without invoking BYOND.
    pub fn react_with(
        &mut self,
        context: Context,
        on_effect: &mut impl FnMut(&mut Mixture, &Effect),
    ) -> Outcome {
        let mut candidates = Vec::new();
        for phase in 1..=4 {
            for &gas in &self.order {
                for (index, reaction) in REACTIONS.iter().enumerate() {
                    if reaction.phase == phase && reaction.major == gas {
                        candidates.push(index);
                    }
                }
            }
        }
        if candidates.is_empty() {
            return Outcome::default();
        }
        if self.moles[HYPERNOBLIUM] >= REACTION_OPPRESSION_THRESHOLD
            && self.temperature > REACTION_OPPRESSION_MIN_TEMP
        {
            return Outcome {
                flags: STOP_FLAG,
                effects: Vec::new(),
            };
        }
        self.results.fill(None);
        let temperature = self.temperature;
        let mut outcome = Outcome::default();
        for index in candidates {
            if !self.eligible(index, temperature, context) {
                continue;
            }
            let next = self.apply(index, context);
            for effect in &next.effects {
                on_effect(self, effect);
            }
            outcome.flags |= next.flags;
            outcome.effects.extend(next.effects);
        }
        if outcome.flags != 0 {
            self.garbage_collect();
        }
        outcome
    }
}

fn quantize(value: f32) -> f32 {
    // DM round(x, step) rounds ties upwards, including negative deltas.
    (value / MOLAR_ACCURACY + 0.5).floor() * MOLAR_ACCURACY
}
