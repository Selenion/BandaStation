//! Regressions for DM scheduling semantics that must survive a native port.
use bandastation_atmos::{catalog::*, Context, Effect, Holder, Mixture, STOP_FLAG, VOLATILE_FLAG};

fn index(id: &str) -> usize {
    REACTIONS.iter().position(|r| r.id == id).unwrap()
}

#[test]
fn deferred_random_effects_keep_gas_math_and_request_a_roll_for_each_reaction() {
    for (reaction, temperature, fuel) in
        [("tritfire", 8000.0, TRITIUM), ("freonfire", 140.0, FREON)]
    {
        let mut fixed = Mixture::new(temperature, CELL_VOLUME, true);
        fixed.set(OXYGEN, 200.0);
        fixed.set(fuel, 10.0);
        let mut deferred = fixed.clone();
        for _ in 0..2 {
            let normal = fixed.react_one(
                index(reaction),
                Context {
                    holder: Holder::OpenTurf,
                    ..Context::default()
                },
            );
            let queued = deferred.react_one(
                index(reaction),
                Context {
                    holder: Holder::OpenTurf,
                    defer_random: true,
                    ..Context::default()
                },
            );
            assert_eq!(fixed.moles, deferred.moles);
            assert_eq!(fixed.temperature.to_bits(), deferred.temperature.to_bits());
            assert_eq!(normal.flags, queued.flags);
            let rolls = queued
                .effects
                .iter()
                .filter(|e| matches!(e, Effect::RadiationChance { .. } | Effect::HotIceChance))
                .count();
            // The second freon step may leave the hot-ice temperature window.
            assert!(rolls <= 1);
            if reaction == "tritfire" {
                assert_eq!(rolls, 1);
            }
        }
    }
}

#[test]
fn heat_capacity_matches_byond_rounding_without_reordering_reaction_keys() {
    // Captured from BYOND 516.1688: vapor:random=2 and vapor:random=0.
    // The second fixture also distinguishes fused from separately rounded operations.
    let fixtures = [
        (
            [
                (ZAUKER, 52.847_41),
                (HELIUM, 83.995_91),
                (HYPERNOBLIUM, 2.862_068_4),
                (WATER_VAPOR, 38.501_785),
            ],
            0x46d3_197b,
        ),
        (
            [
                (FREON, 41.436_733),
                (WATER_VAPOR, 21.356_91),
                (NITROUS_OXIDE, 70.992_87),
                (NITRIUM, 45.474_59),
            ],
            0x46e2_a58e,
        ),
    ];
    for (gases, expected_bits) in fixtures {
        for permutation in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 3, 0, 1]] {
            let mut air = Mixture::new(293.15, 2500.0, false);
            let mut keys = Vec::new();
            for i in permutation {
                let (gas, moles) = gases[i];
                air.set(gas, moles);
                keys.push(gas);
            }
            assert_eq!(air.heat_capacity().to_bits(), expected_bits);
            assert_eq!(air.keys(), keys);
        }
    }
}

#[test]
fn new_gas_does_not_schedule_an_extra_reaction_in_the_same_tick() {
    let mut air = Mixture::new(200.0, 2500.0, false);
    air.set(BZ, 5.0);
    air.set(OXYGEN, 10.0);
    air.set(NITROGEN, 20.0);
    air.set(PLASMA, 10.0);
    // N2O formation creates the major gas of BZ formation, but that major
    // gas was absent when the candidate list was assembled.
    air.react(Context::default());
    assert!(air.moles[NITROUS_OXIDE] > 0.0);
    assert_eq!(air.results[index("bzformation")], None);
    assert_eq!(air.moles[BZ], 5.0);
}

#[test]
fn temperature_requirements_use_the_start_of_the_tick() {
    let mut air = Mixture::new(200.0, 2500.0, false);
    air.set(NITRIUM, 0.05);
    air.set(OXYGEN, 0.01);
    air.set(PLASMA, 0.01);
    air.react(Context::default());
    assert!(air.temperature > PLASMA_MINIMUM_BURN_TEMPERATURE);
    assert!(air.results[index("nitrium_decomp")].is_some());
    assert_eq!(air.results[index("plasmafire")], None);
}

#[test]
fn suppression_preserves_previous_results_and_includes_its_strict_temperature_boundary() {
    let mut air = Mixture::new(21.0, 2500.0, false);
    air.set(HYPERNOBLIUM, 5.0);
    air.set(ANTINOBLIUM, 0.25);
    air.results.fill(Some(123.0));
    assert_eq!(air.react(Context::default()).flags, STOP_FLAG);
    assert!(air.results.iter().all(|r| *r == Some(123.0)));
    air.temperature = 20.0;
    assert_ne!(air.react(Context::default()).flags, STOP_FLAG);
}

#[test]
fn permafrost_can_be_volatile_without_consuming_water() {
    let mut air = Mixture::new(190.0, 2500.0, true);
    air.set(WATER_VAPOR, 0.25);
    let result = air.react_one(
        index("vapor"),
        Context {
            holder: Holder::OpenTurf,
            permafrost: true,
            freeze_succeeds: false,
            ..Context::default()
        },
    );
    assert_eq!(result.flags, VOLATILE_FLAG);
    assert_eq!(air.moles[WATER_VAPOR], 0.25);
    assert_eq!(result.effects, [Effect::Freeze]);
}

#[test]
fn antinoblium_cleanup_changes_the_mix_without_a_reacting_flag() {
    let mut air = Mixture::new(300.0, 2500.0, false);
    air.set(ANTINOBLIUM, 1.0);
    air.set(NITROGEN, 0.005);
    assert_eq!(air.react(Context::default()).flags, 0);
    assert_eq!(air.moles[NITROGEN], 0.0);
    assert!(air.moles[ANTINOBLIUM] > 1.0);
    assert!(air.keys().contains(&NITROGEN));
}

#[test]
fn every_catalog_reaction_has_an_executable_implementation() {
    for (i, reaction) in REACTIONS.iter().enumerate() {
        let temperature = if reaction.max_temp.is_finite() {
            (reaction.min_temp + reaction.max_temp) / 2.0
        } else {
            reaction.min_temp.max(293.15) + 200.0
        };
        let mut air = Mixture::new(temperature, 2500.0, false);
        for &(gas, minimum) in reaction.requirements {
            air.set(gas, (minimum * 10.0).max(30.0));
        }
        air.react_one(
            i,
            Context {
                holder: Holder::OpenTurf,
                freeze_succeeds: true,
                supermatter_power: 7000.0,
                ..Context::default()
            },
        );
        assert!(air.temperature.is_finite(), "{}", reaction.id);
        assert!(air.moles.iter().all(|n| n.is_finite()), "{}", reaction.id);
    }
}
