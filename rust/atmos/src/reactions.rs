//! Numerical procedures ported from code/modules/atmospherics/.../reactions.dm.
//! The generated catalog supplies all requirements and named DM constants.
use crate::{catalog::*, Context, Effect, Mixture, Outcome, REACTING_FLAG, VOLATILE_FLAG};

macro_rules! min { ($a:expr, $($b:expr),+) => { { let n: f32 = $a; $(let n = n.min($b);)+ n } }; }

impl Mixture {
    pub(crate) fn apply(&mut self, index: usize, context: Context) -> Outcome {
        let mut out = Outcome::default();
        let t = self.temperature;
        let old = self.heat_capacity();
        let m = *self.moles;
        let (mut flags, result, delta, clamp): (u8, Option<f32>, f32, bool);
        match REACTIONS[index].id {
            "vapor" => {
                if !context.turf() {
                    return out;
                }
                flags = 0;
                let consumed = if t <= WATER_VAPOR_DEPOSITION_POINT {
                    out.effects.push(Effect::Freeze);
                    if context.permafrost {
                        flags |= VOLATILE_FLAG;
                    }
                    if context.freeze_succeeds {
                        MOLES_GAS_VISIBLE
                    } else {
                        0.0
                    }
                } else if t <= WATER_VAPOR_CONDENSATION_POINT {
                    if !context.groundless && !context.no_slip {
                        out.effects.push(Effect::Wet);
                    }
                    MOLES_GAS_VISIBLE
                } else {
                    0.0
                };
                if consumed != 0.0 {
                    self.add(WATER_VAPOR, -consumed);
                    self.results[index] = Some(consumed);
                    flags |= REACTING_FLAG;
                }
                out.flags = flags;
                return out;
            }
            "sterilization" => {
                if m[WATER_VAPOR] != 0.0
                    && m[WATER_VAPOR] / self.total_moles() > MIASTER_STERILIZATION_MAX_HUMIDITY
                {
                    return out;
                }
                let r = m[MIASMA].min(
                    MIASTER_STERILIZATION_RATE_BASE
                        + (t - MIASTER_STERILIZATION_TEMP) / MIASTER_STERILIZATION_RATE_SCALE,
                );
                self.add(MIASMA, -r);
                self.add(OXYGEN, r);
                self.temperature += r * MIASTER_STERILIZATION_ENERGY;
                self.results[index] = Some(r);
                out.flags = REACTING_FLAG;
                return out;
            }
            "plasmafire" | "freonfire" => {
                let plasma = REACTIONS[index].id == "plasmafire";
                let fuel = if plasma { PLASMA } else { FREON };
                let scale = if plasma {
                    if t > PLASMA_UPPER_TEMPERATURE {
                        1.0
                    } else {
                        (t - PLASMA_MINIMUM_BURN_TEMPERATURE)
                            / (PLASMA_UPPER_TEMPERATURE - PLASMA_MINIMUM_BURN_TEMPERATURE)
                    }
                } else if t < FREON_TERMINAL_TEMPERATURE {
                    0.0
                } else if t < FREON_LOWER_TEMPERATURE {
                    0.5
                } else {
                    (FREON_MAXIMUM_BURN_TEMPERATURE - t)
                        / (FREON_MAXIMUM_BURN_TEMPERATURE - FREON_TERMINAL_TEMPERATURE)
                };
                if scale <= 0.0 {
                    return out;
                }
                let ratio = OXYGEN_BURN_RATIO_BASE - scale;
                let full = if plasma {
                    PLASMA_OXYGEN_FULLBURN
                } else {
                    FREON_OXYGEN_FULLBURN
                };
                let divisor = if plasma {
                    PLASMA_BURN_RATE_DELTA
                } else {
                    FREON_BURN_RATE_DELTA
                };
                let available = if m[OXYGEN] < m[fuel] * full {
                    m[OXYGEN] / full
                } else {
                    m[fuel]
                };
                let r = available / divisor * scale;
                if r < MINIMUM_HEAT_CAPACITY {
                    return out;
                }
                let r = min!(r, m[fuel], m[OXYGEN] * (1.0 / ratio));
                self.set(fuel, crate::quantize(m[fuel] - r));
                self.set(OXYGEN, crate::quantize(m[OXYGEN] - r * ratio));
                if plasma && m[OXYGEN] / m[PLASMA] >= SUPER_SATURATION_THRESHOLD {
                    self.add(TRITIUM, r);
                } else if plasma {
                    self.add(CARBON_DIOXIDE, r * 0.75);
                    self.add(WATER_VAPOR, r * 0.25);
                } else {
                    self.add(CARBON_DIOXIDE, r);
                    if t < HOT_ICE_FORMATION_MAXIMUM_TEMPERATURE
                        && t > HOT_ICE_FORMATION_MINIMUM_TEMPERATURE
                        && (context.hot_ice_roll || context.defer_random)
                        && context.turf()
                    {
                        out.effects.push(if context.defer_random {
                            Effect::HotIceChance
                        } else {
                            Effect::HotIce
                        });
                    }
                }
                result = Some(r * (1.0 + ratio));
                delta = r * if plasma {
                    FIRE_PLASMA_ENERGY_RELEASED
                } else {
                    -FIRE_FREON_ENERGY_CONSUMED
                };
                flags = REACTING_FLAG | VOLATILE_FLAG;
                clamp = !plasma;
            }
            "h2fire" | "tritfire" => {
                let tritium = REACTIONS[index].id == "tritfire";
                let fuel = if tritium { TRITIUM } else { HYDROGEN };
                let r = min!(
                    m[fuel] / FIRE_HYDROGEN_BURN_RATE_DELTA,
                    m[OXYGEN] / (FIRE_HYDROGEN_BURN_RATE_DELTA * HYDROGEN_OXYGEN_FULLBURN),
                    m[fuel],
                    m[OXYGEN] * (1.0 / 0.5)
                );
                if r <= 0.0 || m[fuel] - r < 0.0 || m[OXYGEN] - r * 0.5 < 0.0 {
                    return out;
                }
                self.add(fuel, -r);
                self.add(OXYGEN, -r * 0.5);
                self.add(WATER_VAPOR, r);
                delta = FIRE_HYDROGEN_ENERGY_RELEASED * r;
                if tritium
                    && context.location()
                    && r > TRITIUM_RADIATION_MINIMUM_MOLES
                    && delta
                        > TRITIUM_RADIATION_RELEASE_THRESHOLD
                            * (self.volume / CELL_VOLUME).powf(ATMOS_RADIATION_VOLUME_EXP)
                    && (context.radiation_roll || context.defer_random)
                {
                    let range = (r.sqrt() / TRITIUM_RADIATION_RANGE_DIVISOR)
                        .min(GAS_REACTION_MAXIMUM_RADIATION_PULSE_RANGE);
                    out.effects.push(if context.defer_random {
                        Effect::RadiationChance {
                            range,
                            threshold: TRITIUM_RADIATION_THRESHOLD,
                        }
                    } else {
                        Effect::Radiation {
                            range,
                            threshold: TRITIUM_RADIATION_THRESHOLD,
                        }
                    });
                }
                result = Some(r);
                flags = REACTING_FLAG | VOLATILE_FLAG;
                clamp = false;
            }
            "nitrousformation" => {
                let r = (m[OXYGEN] * (1.0 / 0.5)).min(m[NITROGEN]);
                if m[OXYGEN] - r * 0.5 < 0.0 || m[NITROGEN] - r < 0.0 {
                    return out;
                }
                self.add(OXYGEN, -r * 0.5);
                self.add(NITROGEN, -r);
                self.add(NITROUS_OXIDE, r);
                result = Some(r);
                delta = r * N2O_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "nitrous_decomp" => {
                let r = m[NITROUS_OXIDE] / N2O_DECOMPOSITION_RATE_DIVISOR
                    * ((t - N2O_DECOMPOSITION_MIN_SCALE_TEMP)
                        * (t - N2O_DECOMPOSITION_MAX_SCALE_TEMP)
                        / N2O_DECOMPOSITION_SCALE_DIVISOR);
                if r <= 0.0 || m[NITROUS_OXIDE] - r < 0.0 {
                    return out;
                }
                self.add(NITROUS_OXIDE, -r);
                self.add(NITROGEN, r);
                self.add(OXYGEN, r / 2.0);
                result = Some(r);
                delta = r * N2O_DECOMPOSITION_ENERGY;
                flags = REACTING_FLAG;
                clamp = false;
            }
            "bzformation" => {
                let environment = self.volume.max(0.0) / self.pressure();
                let ratio = (m[NITROUS_OXIDE] / m[PLASMA]).min(1.0);
                let d = (4.0 * (m[PLASMA] / (m[NITROUS_OXIDE] + m[PLASMA]) - 0.75)).max(0.0);
                let r = min!(
                    0.01 * ratio * environment,
                    m[NITROUS_OXIDE] * (1.0 / 0.4),
                    m[PLASMA] * (1.0 / (0.8 * (1.0 - d)))
                );
                if r <= 0.0
                    || m[NITROUS_OXIDE] - r * 0.4 < 0.0
                    || m[PLASMA] - 0.8 * r * (1.0 - d) < 0.0
                {
                    return out;
                }
                if d > 0.0 {
                    let amount = 0.4 * r * d;
                    self.add(NITROGEN, amount);
                    self.add(OXYGEN, 0.5 * amount);
                }
                self.add(BZ, r * (1.0 - d));
                self.add(NITROUS_OXIDE, -0.4 * r);
                self.add(PLASMA, -0.8 * r * (1.0 - d));
                result = Some(r);
                delta = r
                    * (BZ_FORMATION_ENERGY + d * (N2O_DECOMPOSITION_ENERGY - BZ_FORMATION_ENERGY));
                flags = REACTING_FLAG;
                clamp = true;
            }
            "pluox_formation" => {
                let r = min!(
                    PLUOXIUM_FORMATION_MAX_RATE,
                    m[CARBON_DIOXIDE],
                    m[OXYGEN] * (1.0 / 0.5),
                    m[TRITIUM] * (1.0 / 0.01)
                );
                if r <= 0.0
                    || m[CARBON_DIOXIDE] - r < 0.0
                    || m[OXYGEN] - r * 0.5 < 0.0
                    || m[TRITIUM] - r * 0.01 < 0.0
                {
                    return out;
                }
                self.add(CARBON_DIOXIDE, -r);
                self.add(OXYGEN, -r * 0.5);
                self.add(TRITIUM, -r * 0.01);
                self.add(PLUOXIUM, r);
                self.add(HYDROGEN, r * 0.01);
                result = Some(r);
                delta = r * PLUOXIUM_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "nitrium_formation" => {
                let r = min!(
                    t / NITRIUM_FORMATION_TEMP_DIVISOR,
                    m[TRITIUM],
                    m[NITROGEN],
                    m[BZ] * (1.0 / 0.05)
                );
                if r <= 0.0
                    || m[TRITIUM] - r < 0.0
                    || m[NITROGEN] - r < 0.0
                    || m[BZ] - r * 0.05 < 0.0
                {
                    return out;
                }
                self.add(TRITIUM, -r);
                self.add(NITROGEN, -r);
                self.add(BZ, -r * 0.05);
                self.add(NITRIUM, r);
                result = Some(r);
                delta = -r * NITRIUM_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "nitrium_decomp" => {
                let r = (t / NITRIUM_DECOMPOSITION_TEMP_DIVISOR).min(m[NITRIUM]);
                if r <= 0.0 || m[NITRIUM] - r < 0.0 {
                    return out;
                }
                self.add(NITROGEN, 0.0);
                self.add(HYDROGEN, 0.0);
                self.add(NITRIUM, -r);
                self.add(HYDROGEN, r);
                self.add(NITROGEN, r);
                result = Some(r);
                delta = r * NITRIUM_DECOMPOSITION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "freonformation" => {
                let factor = min!(
                    m[PLASMA] * (1.0 / 0.6),
                    m[BZ] * (1.0 / 0.1),
                    m[CARBON_DIOXIDE] * (1.0 / 0.3)
                );
                let heat = NUM_E.powf(-((t - 800.0) / 200.0).powf(2.0))
                    + 3.0 / (1.0 + NUM_E.powf(-0.001 * (t - 6000.0)));
                let r = min!(
                    heat * factor * 0.05,
                    m[PLASMA] * (1.0 / 0.6),
                    m[CARBON_DIOXIDE] * (1.0 / 0.3),
                    m[BZ] * (1.0 / 0.1)
                );
                if r <= 0.0
                    || m[PLASMA] - r * 0.6 < 0.0
                    || m[CARBON_DIOXIDE] - r * 0.3 < 0.0
                    || m[BZ] - r * 0.1 < 0.0
                {
                    return out;
                }
                self.add(PLASMA, -r * 0.6);
                self.add(CARBON_DIOXIDE, -r * 0.3);
                self.add(BZ, -r * 0.1);
                self.add(FREON, r);
                result = Some(r);
                delta = -(7000.0 / (1.0 + NUM_E.powf(-0.0015 * (t - 6000.0))) + 1000.0) * r * 0.1;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "nobformation" => {
                self.add(HYPERNOBLIUM, 0.0);
                self.add(BZ, 0.0);
                let d = (m[TRITIUM] / (m[TRITIUM] + m[BZ])).clamp(0.001, 1.0);
                let r = min!(
                    (m[NITROGEN] + m[TRITIUM]) * 0.01,
                    m[TRITIUM] * (1.0 / (5.0 * d)),
                    m[NITROGEN] * (1.0 / 10.0)
                );
                if crate::quantize(r) <= 0.0
                    || crate::quantize(m[TRITIUM] - 5.0 * r * d) < 0.0
                    || crate::quantize(m[NITROGEN] - 10.0 * r) < 0.0
                {
                    self.garbage_collect();
                    return out;
                }
                self.add(TRITIUM, -5.0 * r * d);
                self.add(NITROGEN, -10.0 * r);
                self.add(HYPERNOBLIUM, r);
                result = Some(r);
                delta = r * NOBLIUM_FORMATION_ENERGY / m[BZ].max(1.0);
                flags = REACTING_FLAG | VOLATILE_FLAG;
                clamp = true;
            }
            "halon_o2removal" => {
                let r = min!(
                    t / HALON_COMBUSTION_TEMPERATURE_SCALE,
                    m[HALON],
                    m[OXYGEN] * (1.0 / 20.0)
                );
                if r <= 0.0 || m[HALON] - r < 0.0 || m[OXYGEN] - r * 20.0 < 0.0 {
                    return out;
                }
                self.add(HALON, -r);
                self.add(OXYGEN, -r * 20.0);
                self.add(PLUOXIUM, r * 2.5);
                result = Some(r * 5.0);
                delta = -r * HALON_COMBUSTION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
                if r > HALON_COMBUSTION_MINIMUM_RESIN_MOLES
                    && context.open()
                    && !context.foam
                    && !context.resin
                {
                    flags |= VOLATILE_FLAG;
                    out.effects.push(Effect::Resin {
                        volume: HALON_COMBUSTION_RESIN_VOLUME,
                    });
                }
            }
            "healium_formation" => {
                let r = min!(t * 0.3, m[FREON] * (1.0 / 2.75), m[BZ] * (1.0 / 0.25));
                if r <= 0.0 || m[FREON] - r * 2.75 < 0.0 || m[BZ] - r * 0.25 < 0.0 {
                    return out;
                }
                self.add(FREON, -r * 2.75);
                self.add(BZ, -r * 0.25);
                self.add(HEALIUM, r * 3.0);
                result = Some(r * 3.0);
                delta = r * HEALIUM_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "zauker_formation" => {
                let r = min!(
                    t * ZAUKER_FORMATION_TEMPERATURE_SCALE,
                    m[HYPERNOBLIUM] * (1.0 / 0.01),
                    m[NITRIUM] * (1.0 / 0.5)
                );
                if r <= 0.0 || m[HYPERNOBLIUM] - r * 0.01 < 0.0 || m[NITRIUM] - r * 0.5 < 0.0 {
                    return out;
                }
                self.add(HYPERNOBLIUM, -r * 0.01);
                self.add(NITRIUM, -r * 0.5);
                self.add(ZAUKER, r * 0.5);
                result = Some(r * 0.5);
                delta = -r * ZAUKER_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "zauker_decomp" => {
                let r = min!(ZAUKER_DECOMPOSITION_MAX_RATE, m[NITROGEN], m[ZAUKER]);
                if r <= 0.0 || m[ZAUKER] - r < 0.0 {
                    return out;
                }
                self.add(ZAUKER, -r);
                self.add(OXYGEN, r * 0.3);
                self.add(NITROGEN, r * 0.7);
                result = Some(r);
                delta = r * ZAUKER_DECOMPOSITION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "proto_nitrate_formation" => {
                let r = min!(
                    t * 0.005,
                    m[PLUOXIUM] * (1.0 / 0.2),
                    m[HYDROGEN] * (1.0 / 2.0)
                );
                if r <= 0.0 || m[PLUOXIUM] - r * 0.2 < 0.0 || m[HYDROGEN] - r * 2.0 < 0.0 {
                    return out;
                }
                self.add(HYDROGEN, -r * 2.0);
                self.add(PLUOXIUM, -r * 0.2);
                self.add(PROTO_NITRATE, r * 2.2);
                result = Some(r * 2.2);
                delta = r * PN_FORMATION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "proto_nitrate_hydrogen_response" => {
                let r = min!(
                    PN_HYDROGEN_CONVERSION_MAX_RATE,
                    m[HYDROGEN],
                    m[PROTO_NITRATE]
                );
                if r <= 0.0 || m[HYDROGEN] - r < 0.0 {
                    return out;
                }
                self.add(HYDROGEN, -r);
                self.add(PROTO_NITRATE, r * 0.5);
                result = Some(r * 0.5);
                delta = -r * PN_HYDROGEN_CONVERSION_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
            }
            "proto_nitrate_tritium_response" => {
                let r = min!(
                    t / 34.0 * (m[TRITIUM] * m[PROTO_NITRATE])
                        / (m[TRITIUM] + 10.0 * m[PROTO_NITRATE]),
                    m[TRITIUM],
                    m[PROTO_NITRATE] * (1.0 / 0.01)
                );
                if m[TRITIUM] - r < 0.0 || m[PROTO_NITRATE] - r * 0.01 < 0.0 {
                    return out;
                }
                self.add(PROTO_NITRATE, -r * 0.01);
                self.add(TRITIUM, -r);
                self.add(HYDROGEN, r);
                result = Some(r);
                delta = r * PN_TRITIUM_CONVERSION_ENERGY;
                flags = if delta != 0.0 { REACTING_FLAG } else { 0 };
                clamp = true;
                if context.location()
                    && delta
                        > PN_TRITIUM_CONVERSION_RAD_RELEASE_THRESHOLD
                            * (self.volume / CELL_VOLUME).powf(ATMOS_RADIATION_VOLUME_EXP)
                {
                    flags |= VOLATILE_FLAG;
                    out.effects.push(Effect::Radiation {
                        range: (r.sqrt() / PN_TRITIUM_RAD_RANGE_DIVISOR)
                            .min(GAS_REACTION_MAXIMUM_RADIATION_PULSE_RANGE),
                        threshold: PN_TRITIUM_RAD_THRESHOLD,
                    });
                }
            }
            "proto_nitrate_bz_response" => {
                let r = min!(
                    t / 2240.0 * m[BZ] * m[PROTO_NITRATE] / (m[BZ] + m[PROTO_NITRATE]),
                    m[BZ],
                    m[PROTO_NITRATE]
                );
                if r <= 0.0 || m[BZ] - r < 0.0 {
                    return out;
                }
                self.add(BZ, -r);
                self.add(NITROGEN, r * 0.4);
                self.add(HELIUM, r * 1.6);
                self.add(PLASMA, r * 0.8);
                result = Some(r);
                delta = r * PN_BZASE_ENERGY;
                flags = REACTING_FLAG;
                clamp = true;
                if context.location()
                    && delta
                        > PN_BZASE_RAD_RELEASE_THRESHOLD
                            * (self.volume / CELL_VOLUME).powf(ATMOS_RADIATION_VOLUME_EXP)
                {
                    flags |= VOLATILE_FLAG;
                    let particles = (r / PN_BZASE_NUCLEAR_PARTICLE_DIVISOR)
                        .floor()
                        .min(PN_BZASE_NUCLEAR_PARTICLE_MAXIMUM);
                    for _ in 0..particles as usize {
                        out.effects.push(Effect::NuclearParticle);
                    }
                    out.effects.push(Effect::Radiation {
                        range: ((r - particles
                            * PN_BZASE_NUCLEAR_PARTICLE_RADIATION_ENERGY_CONVERSION)
                            .sqrt()
                            / PN_BZASE_RAD_RANGE_DIVISOR)
                            .min(GAS_REACTION_MAXIMUM_RADIATION_PULSE_RANGE),
                        threshold: PN_BZASE_RAD_THRESHOLD,
                    });
                    out.effects.push(Effect::Hallucination {
                        radius: 1.0,
                        duration: r * 20.0,
                    });
                }
            }
            "antinoblium_replication" => {
                let other = self.total_moles() - m[ANTINOBLIUM];
                let cleanup = other < MINIMUM_MOLE_COUNT;
                let r = if cleanup {
                    other
                } else {
                    (m[ANTINOBLIUM] / ANTINOBLIUM_CONVERSION_DIVISOR).min(other)
                };
                for gas in self.order.clone() {
                    if gas == ANTINOBLIUM {
                        continue;
                    }
                    self.set(
                        gas,
                        if cleanup {
                            0.0
                        } else {
                            m[gas] - r * m[gas] / other
                        },
                    );
                }
                self.add(ANTINOBLIUM, r);
                result = Some(r);
                delta = 0.0;
                flags = if cleanup {
                    0
                } else {
                    REACTING_FLAG | VOLATILE_FLAG
                };
                clamp = true;
            }
            "h2o_conversion" => {
                let r = (m[WATER_VAPOR] * (1.0 / 2.0)).min(2.5 * context.working_power.powf(2.0));
                self.adjust(WATER_VAPOR, -r * 2.0);
                self.adjust(OXYGEN, r);
                self.adjust(HYDROGEN, r * 2.0);
                result = None;
                delta = 0.0;
                flags = 0;
                clamp = true;
            }
            "nob_conversion" => {
                self.add(HYPERNOBLIUM, 0.0);
                self.add(ANTINOBLIUM, 0.0);
                let r = m[HYPERNOBLIUM]
                    * (context.supermatter_power - POWER_PENALTY_THRESHOLD).clamp(
                        0.0,
                        CRITICAL_POWER_PENALTY_THRESHOLD - POWER_PENALTY_THRESHOLD,
                    )
                    / (CRITICAL_POWER_PENALTY_THRESHOLD - POWER_PENALTY_THRESHOLD);
                self.adjust(HYPERNOBLIUM, -r);
                self.adjust(ANTINOBLIUM, r);
                let capacity = old + r * (HEATS[ANTINOBLIUM] - HEATS[HYPERNOBLIUM]);
                if capacity > MINIMUM_HEAT_CAPACITY {
                    self.temperature = (t * old / capacity).max(TCMB);
                }
                return out;
            }
            "halon_generation" => {
                self.add(BZ, 0.0);
                self.add(OXYGEN, 0.0);
                self.add(HALON, 0.0);
                let r = (m[BZ]
                    * (1.0
                        - NUM_E.powf(
                            -0.5 * t * context.working_power / FIRE_MINIMUM_TEMPERATURE_TO_EXIST,
                        )))
                .min(m[BZ]);
                self.adjust(BZ, -r);
                self.adjust(OXYGEN, r * 0.2);
                self.adjust(HALON, r * 2.0);
                result = None;
                delta = r * HALON_FORMATION_ENERGY;
                flags = 0;
                clamp = true;
            }
            _ => unreachable!("Catalog reaction has no implementation"),
        }
        if let Some(amount) = result {
            self.results[index] = Some(amount);
        }
        if REACTIONS[index].id != "proto_nitrate_tritium_response" || delta != 0.0 {
            self.energy(old, delta, clamp);
        }
        let id = REACTIONS[index].id;
        let hotspot = match id {
            "plasmafire" | "h2fire" => {
                context.open() && self.temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST
            }
            "tritfire" => {
                context.resolved_open() && self.temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST
            }
            "freonfire" => context.open() && self.temperature < FREON_MAXIMUM_BURN_TEMPERATURE,
            _ => false,
        };
        if hotspot {
            out.effects.push(Effect::Hotspot {
                temperature: self.temperature,
                volume: CELL_VOLUME,
            });
        }
        out.flags = flags;
        out
    }
}
