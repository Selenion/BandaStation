use bandastation_atmos::{
    catalog::*,
    transport::{Boundary, Sample},
    Mixture,
};

fn sample(temperature: f32, gases: &[(usize, f32)], turf: bool) -> Sample {
    let mut mixture = Mixture::new(temperature, CELL_VOLUME, turf);
    for &(gas, amount) in gases {
        mixture.set(gas, amount);
    }
    Sample::new(mixture)
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= expected.abs() * 2e-5 + 2e-5,
        "{actual} != {expected}"
    );
}

// Expected observations are from the pinned Banda DM transport-reference.json.
#[test]
fn a_vacuum_uses_turf_heat_capacity_and_preserves_insertion_order() {
    let mut a = sample(293.15, &[(OXYGEN, 100.0), (NITROGEN, 300.0)], true);
    let mut b = sample(TCMB, &[], true);
    a.share(&mut b, 0.2, 0.2);
    assert_eq!(a.air.moles[OXYGEN], 80.0);
    assert_eq!(b.air.moles[NITROGEN], 60.0);
    assert_eq!(b.air.keys(), &[OXYGEN, NITROGEN]);
    assert!(b.archived.keys().is_empty());
    close(b.air.temperature, 56.7372093);
    assert_eq!(a.last_share, 80.0);
}

#[test]
fn repeated_boundary_exchanges_read_the_same_archive() {
    let mut a = sample(800.0, &[(OXYGEN, 100.0), (PLASMA, 30.0)], false);
    let mut b = sample(200.0, &[(OXYGEN, 20.0), (WATER_VAPOR, 2.0)], false);
    for _ in 0..3 {
        a.share(&mut b, 0.2, 0.2);
    }
    assert_eq!(a.archived.moles[OXYGEN], 100.0);
    assert_eq!(a.air.moles[OXYGEN], 52.0);
    assert_eq!(b.air.moles[OXYGEN], 68.0);
    close(a.air.temperature, 791.743103);
    close(b.air.temperature, 748.076904);
}

#[test]
fn share_cleans_all_trace_gases_and_their_archives() {
    let mut a = sample(293.15, &[(OXYGEN, 0.0001), (NITROGEN, 1.0)], false);
    let mut b = sample(293.15, &[(PLASMA, 0.0)], false);
    a.share(&mut b, 0.2, 0.2);
    assert_eq!(a.air.keys(), &[NITROGEN]);
    assert_eq!(a.archived.keys(), &[NITROGEN]);
    assert_eq!(b.air.keys(), &[NITROGEN]);
    assert!(b.archived.keys().is_empty());
}

#[test]
fn empty_turfs_still_exchange_heat() {
    let mut a = sample(700.0, &[], true);
    let mut b = sample(TCMB, &[], true);
    a.temperature_share(&mut b, 0.4);
    close(a.air.temperature, 560.539978);
    close(b.air.temperature, 142.160004);
    assert_eq!(a.archived.temperature, 700.0);
    assert_eq!(b.archived.temperature, TCMB);
}

#[test]
fn space_does_not_inherit_the_empty_turf_archive_heat_capacity() {
    for space_calls in [false, true] {
        let mut floor = sample(800.0, &[], true);
        let mut space = sample(TCMB, &[], false);
        space.boundary = Boundary::Space;
        if space_calls {
            space.temperature_share(&mut floor, 0.4);
        } else {
            floor.temperature_share(&mut space, 0.4);
        }
        // tg/Banda override only current space heat capacity; the empty archive uses the base proc.
        assert_eq!(floor.air.temperature, 800.0);
        assert_eq!(space.air.temperature, TCMB);
    }
}
