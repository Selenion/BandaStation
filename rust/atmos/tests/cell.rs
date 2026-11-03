use bandastation_atmos::{
    catalog::*,
    cell::{Cell, WorldAction},
    Context, Effect, Holder, Mixture,
};

fn cell(temperature: f32, gases: &[(usize, f32)]) -> Cell {
    let mut air = Mixture::new(temperature, CELL_VOLUME, true);
    for &(gas, amount) in gases {
        air.set(gas, amount);
    }
    Cell::new(
        air,
        Context {
            holder: Holder::OpenTurf,
            ..Context::default()
        },
    )
}

#[test]
fn hotspot_birth_changes_the_air_before_the_next_outer_reaction() {
    let mut cell = cell(1644.0, &[(PLASMA, 1.0), (OXYGEN, 0.51), (HYDROGEN, 1.0)]);
    let outcome = cell.react();
    assert_eq!(outcome.flags, 5);
    // Actual Banda world-reference.json, with original hotspot Initialize/perform_exposure.
    assert!((cell.air.moles[OXYGEN] - 0.480445892).abs() < 2e-5);
    assert!((cell.air.temperature - 2044.61047).abs() < 0.05);
    let spot = cell.hotspot.as_ref().unwrap();
    assert!((spot.temperature - 2123.96851).abs() < 0.05);
    assert!((spot.volume - 1326.70215).abs() < 0.05);
    assert_eq!(
        cell.commands
            .iter()
            .filter(|c| matches!(c.action, WorldAction::FireExposure { .. }))
            .count(),
        1
    );
}

#[test]
fn creating_permafrost_is_visible_to_the_same_water_reaction() {
    let mut cell = cell(190.0, &[(WATER_VAPOR, 1.0)]);
    let outcome = cell.react();
    assert_eq!(outcome.flags, 5);
    assert_eq!(cell.air.moles[WATER_VAPOR], 0.75);
    assert!(matches!(
        cell.commands[0].action,
        WorldAction::Reaction(Effect::Freeze)
    ));
}

#[test]
fn an_existing_stronger_wet_effect_does_not_become_permafrost() {
    let mut cell = cell(190.0, &[(WATER_VAPOR, 1.0)]);
    cell.freeze_leaves_permafrost = false;
    assert_eq!(cell.react().flags, 1);
    assert_eq!(cell.air.moles[WATER_VAPOR], 0.75);
}

#[test]
fn oxygen_below_half_a_mole_does_not_create_a_hotspot() {
    let mut cell = cell(1644.0, &[(PLASMA, 1.0), (OXYGEN, 0.4999), (HYDROGEN, 1.0)]);
    let before = cell.air.moles.clone();
    cell.hotspot_expose(1644.0, CELL_VOLUME, false);
    assert!(cell.hotspot.is_none());
    assert!(cell.commands.is_empty());
    assert_eq!(cell.air.moles, before);
}
