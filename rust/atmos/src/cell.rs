//! Numerical turf/hotspot lifecycle; game objects are represented by ordered commands.
use crate::{catalog::*, quantize, Context, Effect, Holder, Mixture, Outcome};

#[cfg(test)]
mod sharing_tests {
    use super::*;

    #[test]
    fn sharing_preserves_float_bits_gas_order_results_and_pending_effects() {
        let base = Cell::new(Mixture::new(0.0, 2500.0, false), Context::default());
        assert!(base.same_bits(&base.clone()));
        let mut other = base.clone();
        other.air.temperature = -0.0;
        assert!(!base.same_bits(&other));
        other = base.clone();
        other.context.working_power = -0.0;
        assert!(!base.same_bits(&other));
        other = base.clone();
        other.air.results[0] = Some(0.0);
        assert!(!base.same_bits(&other));
        let mut first = base.clone();
        first.air.set(OXYGEN, 1.0);
        first.air.set(NITROGEN, 1.0);
        other = base.clone();
        other.air.set(NITROGEN, 1.0);
        other.air.set(OXYGEN, 1.0);
        assert!(!first.same_bits(&other));
        other = base.clone();
        other.hotspot = Some(Hotspot {
            temperature: 1000.0,
            volume: 100.0,
            just_spawned: true,
            bypassing: false,
            cold_fire: false,
        });
        assert!(!other.same_bits(&other.clone()));
        other = base.clone();
        other.commands.push(WorldCommand {
            action: WorldAction::BurnTile,
            air: base.air.clone(),
        });
        assert!(!other.same_bits(&other.clone()));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hotspot {
    pub temperature: f32,
    pub volume: f32,
    pub just_spawned: bool,
    pub bypassing: bool,
    pub cold_fire: bool,
}

#[derive(Clone, Debug)]
pub enum WorldAction {
    Reaction(Effect),
    FireExposure { temperature: f32, volume: f32 },
    BurnTile,
    TurfExposure { temperature: f32, gas: bool },
    Pressure { difference: f32, direction: u8 },
}

/// The snapshot records numeric provenance; DM executes the action after frame publication.
#[derive(Clone, Debug)]
pub struct WorldCommand {
    pub action: WorldAction,
    pub air: Mixture,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub air: Mixture,
    pub context: Context,
    pub hotspot: Option<Hotspot>,
    pub commands: Vec<WorldCommand>,
    pub flags: u8,
    /// Predicted result of MakeSlippery(PERMAFROST), resolved from the DM wet-floor state.
    pub freeze_leaves_permafrost: bool,
}

impl Cell {
    // Numeric equality alone would merge signed zero or absent reaction results visible to DM.
    pub(crate) fn same_bits(&self, other: &Self) -> bool {
        self.hotspot.is_none()
            && other.hotspot.is_none()
            && self.commands.is_empty()
            && other.commands.is_empty()
            && self.flags == other.flags
            && self.freeze_leaves_permafrost == other.freeze_leaves_permafrost
            && self.context.same_bits(&other.context)
            && self.air == other.air
            && self.air.temperature.to_bits() == other.air.temperature.to_bits()
            && self.air.volume.to_bits() == other.air.volume.to_bits()
            && self
                .air
                .moles
                .iter()
                .zip(other.air.moles.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
            && self
                .air
                .results
                .iter()
                .zip(other.air.results.iter())
                .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
    }

    pub fn new(air: Mixture, context: Context) -> Self {
        Self {
            air,
            context,
            hotspot: None,
            commands: Vec::new(),
            flags: 0,
            freeze_leaves_permafrost: true,
        }
    }

    pub fn react(&mut self) -> Outcome {
        let mut context = self.context;
        // freeze_turf updates the floor before water_vapor checks highest_strength.
        if context.turf() && self.air.temperature <= WATER_VAPOR_DEPOSITION_POINT {
            context.freeze_succeeds = true;
            context.permafrost = self.freeze_leaves_permafrost;
        }
        let hotspot = &mut self.hotspot;
        let commands = &mut self.commands;
        let outcome = self
            .air
            .react_with(context, &mut |air, effect| match effect {
                Effect::Hotspot {
                    temperature,
                    volume,
                } => {
                    expose(
                        air,
                        hotspot,
                        commands,
                        context,
                        *temperature,
                        *volume,
                        false,
                    );
                }
                _ => commands.push(WorldCommand {
                    action: WorldAction::Reaction(effect.clone()),
                    air: air.clone(),
                }),
            });
        self.flags = outcome.flags;
        outcome
    }

    pub fn hotspot_expose(&mut self, temperature: f32, volume: f32, strengthen: bool) {
        expose(
            &mut self.air,
            &mut self.hotspot,
            &mut self.commands,
            self.context,
            temperature,
            volume,
            strengthen,
        );
    }

    pub fn process_hotspot(&mut self) -> bool {
        let Some(mut spot) = self.hotspot.take() else {
            return false;
        };
        if spot.just_spawned {
            spot.just_spawned = false;
            self.hotspot = Some(spot);
            return false;
        }
        spot.cold_fire = spot.temperature <= FREON_MAXIMUM_BURN_TEMPERATURE;
        if (spot.temperature < FIRE_MINIMUM_TEMPERATURE_TO_EXIST && !spot.cold_fire)
            || spot.volume <= 1.0
            || self.air.moles[OXYGEN] < 0.5
            || [PLASMA, TRITIUM, HYDROGEN, FREON]
                .iter()
                .all(|g| self.air.moles[*g] < 0.5)
        {
            return false;
        }
        perform_exposure(&mut self.air, &mut spot, &mut self.commands, self.context);
        let sampled = !spot.bypassing && !spot.cold_fire;
        if spot.bypassing && !spot.cold_fire {
            self.commands.push(WorldCommand {
                action: WorldAction::BurnTile,
                air: self.air.clone(),
            });
        }
        self.hotspot = Some(spot);
        sampled
    }
}

fn remove_ratio(air: &mut Mixture, ratio: f32) -> Mixture {
    if ratio <= 0.0 {
        return Mixture::new(TCMB, air.volume, false);
    }
    let mut removed = Mixture::new(air.temperature, air.volume, air.turf_heat_capacity);
    for &gas in &air.order {
        let amount = quantize(air.moles[gas] * ratio.min(1.0));
        removed.set(gas, amount);
        air.moles[gas] -= amount;
    }
    air.garbage_collect();
    removed
}

pub(crate) fn merge(air: &mut Mixture, giver: &Mixture) {
    if (air.temperature - giver.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
        let own_capacity = air.heat_capacity();
        let giver_capacity = giver.heat_capacity();
        let combined = own_capacity + giver_capacity;
        if combined != 0.0 {
            air.temperature =
                (giver.temperature * giver_capacity + air.temperature * own_capacity) / combined;
        }
    }
    for &gas in giver.keys() {
        air.add(gas, giver.moles[gas]);
    }
}

fn expose(
    air: &mut Mixture,
    hotspot: &mut Option<Hotspot>,
    commands: &mut Vec<WorldCommand>,
    context: Context,
    temperature: f32,
    volume: f32,
    strengthen: bool,
) {
    if air.moles[OXYGEN] < 0.5 {
        return;
    }
    let hot_fuel = air.moles[PLASMA] > 0.5 || air.moles[TRITIUM] > 0.5 || air.moles[HYDROGEN] > 0.5;
    let cold_fuel = air.moles[FREON] > 0.5;
    if let Some(spot) = hotspot {
        if strengthen {
            if hot_fuel {
                spot.temperature = spot.temperature.max(temperature);
            } else if cold_fuel {
                spot.temperature = spot.temperature.min(temperature);
            }
            if hot_fuel || cold_fuel {
                spot.volume = spot.volume.max(volume);
            }
        }
        return;
    }
    if !((temperature > PLASMA_MINIMUM_BURN_TEMPERATURE && hot_fuel)
        || (temperature < FREON_MAXIMUM_BURN_TEMPERATURE && cold_fuel))
    {
        return;
    }
    let mut spot = Hotspot {
        temperature,
        volume: volume * 25.0,
        just_spawned: true,
        bypassing: false,
        cold_fire: temperature <= FREON_MAXIMUM_BURN_TEMPERATURE,
    };
    perform_exposure(air, &mut spot, commands, context);
    *hotspot = Some(spot);
}

fn perform_exposure(
    air: &mut Mixture,
    spot: &mut Hotspot,
    commands: &mut Vec<WorldCommand>,
    context: Context,
) {
    spot.bypassing = !spot.just_spawned && spot.volume > CELL_VOLUME * 0.95;
    let reference = if spot.cold_fire || spot.bypassing {
        air.clone()
    } else {
        let mut affected = remove_ratio(air, spot.volume / air.volume);
        affected.temperature = spot.temperature;
        let mut inner_context = context;
        inner_context.holder = Holder::Atom;
        let result = affected.react(inner_context);
        for effect in result.effects {
            commands.push(WorldCommand {
                action: WorldAction::Reaction(effect),
                air: affected.clone(),
            });
        }
        merge(air, &affected);
        affected
    };
    spot.volume = 0.0;
    for (index, reaction) in REACTIONS.iter().enumerate() {
        if reaction.expands_hotspot {
            spot.volume += reference.results[index].unwrap_or(0.0) * FIRE_GROWTH_RATE;
        }
    }
    spot.temperature = reference.temperature;
    if !spot.cold_fire {
        commands.push(WorldCommand {
            action: WorldAction::FireExposure {
                temperature: spot.temperature,
                volume: spot.volume,
            },
            air: air.clone(),
        });
    }
}
