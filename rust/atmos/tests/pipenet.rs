use bandastation_atmos::{catalog::PLASMA, pipenet, transport::Sample, Mixture};
use serde_json::Value;
use std::time::Instant;

fn number(value: &Value) -> f32 {
    value
        .as_f64()
        .map(|n| n as f32)
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
}

fn close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() <= 2e-5_f32.max(expected.abs() * 2e-5),
        "{label}: {actual} != {expected}"
    );
}

#[test]
fn original_dm_reconciliation_contract() {
    let reference: Value =
        serde_json::from_str(include_str!("../../../tools/atmos/pipenet-reference.json")).unwrap();
    assert_eq!(
        reference["input"].as_array().unwrap().len(),
        reference["observed"].as_array().unwrap().len()
    );
    let started = Instant::now();
    let mut steps = 0;
    for (case, expected) in reference["input"]
        .as_array()
        .unwrap()
        .iter()
        .zip(reference["observed"].as_array().unwrap())
    {
        let label = case["label"].as_str().unwrap();
        let mut mixtures: Vec<_> = case["mixtures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|input| {
                let mut air = Mixture::new(
                    number(&input["temperature"]),
                    number(&input["volume"]),
                    false,
                );
                for pair in input["gases"].as_array().unwrap() {
                    air.set(pair[0].as_u64().unwrap() as usize, number(&pair[1]));
                }
                let mut sample = Sample::new(air);
                // The original copy_from_ratio keeps a pre-existing archive independent of current gas.
                sample.archived = bandastation_atmos::Archive::empty(false);
                sample.archived.set(PLASMA, 123.0);
                sample.archived.temperature = 123.0;
                sample.air.results[0] = Some(17.0);
                sample.last_share = 42.0;
                sample
            })
            .collect();
        let order: Vec<_> = case["order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as usize)
            .collect();
        for _ in 0..case["steps"].as_u64().unwrap_or(1) {
            pipenet::reconcile(&mut mixtures, &order).unwrap();
            steps += 1;
        }
        assert_eq!(
            mixtures.len(),
            expected.as_array().unwrap().len(),
            "{label}"
        );
        for ((sample, input), expected) in mixtures
            .iter()
            .zip(case["mixtures"].as_array().unwrap())
            .zip(expected.as_array().unwrap())
        {
            assert_eq!(sample.air.volume, number(&input["volume"]), "{label}");
            close(
                sample.air.temperature,
                number(&expected["temperature"]),
                label,
            );
            let keys: Vec<_> = expected["keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap() as usize)
                .collect();
            assert_eq!(sample.air.keys(), keys, "{label}");
            for (actual, expected) in sample
                .air
                .moles
                .iter()
                .zip(expected["moles"].as_array().unwrap())
            {
                close(*actual, number(expected), label);
            }
            assert_eq!(sample.archived.keys(), &[PLASMA], "{label}");
            assert_eq!(
                sample.archived.moles[PLASMA],
                number(&expected["archive"]),
                "{label}"
            );
            assert_eq!(
                sample.archived.temperature,
                number(&expected["archived_temperature"]),
                "{label}"
            );
            assert_eq!(
                sample.last_share,
                number(&expected["last_share"]),
                "{label}"
            );
            assert_eq!(
                sample.air.results[0],
                Some(number(&expected["result"])),
                "{label}"
            );
        }
    }
    println!("Pipenet: {} cases, {steps} steps, {:.6} s including fixture conversion/assertions (not DreamDaemon)",
        reference["input"].as_array().unwrap().len(), started.elapsed().as_secs_f64());
}

#[test]
fn invalid_reference_cannot_partially_change_mixtures() {
    let mut air = Mixture::new(500.0, 100.0, false);
    air.set(PLASMA, 1.0);
    let mut mixtures = vec![Sample::new(air)];
    let before = mixtures.clone();
    assert_eq!(
        pipenet::reconcile(&mut mixtures, &[0, 1]),
        Err(pipenet::MissingMixture(1))
    );
    assert_eq!(mixtures, before);
}

#[test]
fn empty_turf_capacity_is_not_used_by_pipeline_reconciliation() {
    let empty = Sample::new(Mixture::new(100.0, 100.0, true));
    let mut hot = Mixture::new(600.0, 100.0, false);
    hot.set(PLASMA, 1.0);
    let mut mixtures = vec![empty, Sample::new(hot)];
    pipenet::reconcile(&mut mixtures, &[0, 1]).unwrap();
    assert_eq!(mixtures[0].air.temperature, 600.0);
    assert_eq!(mixtures[1].air.temperature, 600.0);
}

#[test]
fn immutable_copy_is_skipped_without_excluding_its_volume() {
    let mut space = Sample::new(Mixture::new(2.7, 100.0, false));
    space.boundary = bandastation_atmos::transport::Boundary::Space;
    let before = space.clone();
    let mut hot = Mixture::new(600.0, 100.0, false);
    hot.set(PLASMA, 1.0);
    let mut mixtures = vec![space, Sample::new(hot)];
    pipenet::reconcile(&mut mixtures, &[0, 1]).unwrap();
    assert_eq!(mixtures[0], before);
    assert_eq!(mixtures[1].air.moles[PLASMA], 0.5);
}

#[test]
fn pool_reuses_slots_with_new_generations_and_rejects_old_handles() {
    let mut pool = pipenet::Pool::default();
    let old = pool
        .register(Sample::new(Mixture::new(300.0, 0.0, false)))
        .unwrap();
    pool.release(old).unwrap();
    let current = pool
        .register(Sample::new(Mixture::new(500.0, 200.0, false)))
        .unwrap();
    assert_eq!(old.slot, current.slot);
    assert_ne!(old.generation, current.generation);
    assert!(pool.get(old).is_err());
    assert!(pool.release(old).is_err());
    let before = pool.get(current).unwrap().clone();
    assert!(pool.reconcile(&[current, old]).is_err());
    assert_eq!(pool.get(current).unwrap(), &before);
    assert_eq!(pool.len(), 1);
}

#[test]
fn pool_counts_a_shared_reservoir_once_and_keeps_port_state_separate() {
    let mut pool = pipenet::Pool::default();
    let mut air = Mixture::new(300.0, 200.0, false);
    air.set(PLASMA, 10.0);
    let reservoir = pool.register(Sample::new(air)).unwrap();
    let port = pool
        .register(Sample::new(Mixture::new(500.0, 800.0, false)))
        .unwrap();
    pool.reconcile(&[reservoir, reservoir, port]).unwrap();
    assert_eq!(pool.get(reservoir).unwrap().air.moles[PLASMA], 2.0);
    assert_eq!(pool.get(port).unwrap().air.moles[PLASMA], 8.0);
    assert_eq!(pool.get(reservoir).unwrap().air.volume, 200.0);
    assert_eq!(pool.get(port).unwrap().air.volume, 800.0);
}

#[test]
fn pool_churn_does_not_grow_the_slot_table() {
    let mut pool = pipenet::Pool::default();
    for _ in 0..10000 {
        let mut air = Mixture::new(300.0, 200.0, false);
        air.set(PLASMA, 100.0);
        let id = pool.register(Sample::new(air)).unwrap();
        pool.release(id).unwrap();
        assert!(pool.is_empty());
        assert_eq!(pool.slots(), 1);
    }
}

#[test]
fn turf_worker_does_not_snapshot_or_overwrite_pipe_records() {
    let mut store = bandastation_atmos::phase::Store::default();
    let id = store
        .pipes
        .register(Sample::new(Mixture::new(300.0, 200.0, false)))
        .unwrap();
    let job = store.begin().unwrap();
    store
        .pipes
        .replace(id, Sample::new(Mixture::new(500.0, 200.0, false)))
        .unwrap();
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.pipes.get(id).unwrap().air.temperature, 500.0);
    store.close();
    assert!(store.pipes.get(id).is_err());
    assert!(store.pipes.is_empty());
}

#[test]
fn current_import_preserves_archive_results_and_last_share() {
    let mut pool = pipenet::Pool::default();
    let mut sample = Sample::new(Mixture::new(123.0, 100.0, false));
    sample.archived.set(PLASMA, 17.0);
    sample.air.results[0] = Some(42.0);
    sample.last_share = 7.0;
    let id = pool.register(sample.clone()).unwrap();
    let mut replacement = Mixture::new(600.0, 0.0, false);
    replacement.set(PLASMA, 10.0);
    pool.import_air(id, replacement).unwrap();
    let current = pool.get(id).unwrap();
    assert_eq!(current.archived, sample.archived);
    assert_eq!(current.air.results, sample.air.results);
    assert_eq!(current.last_share, sample.last_share);
    assert_eq!(current.air.temperature, 600.0);
    assert_eq!(current.air.volume, 0.0);
    assert_eq!(current.air.moles[PLASMA], 10.0);
}
