use bandastation_atmos::{
    catalog::*,
    operations::{apply_batch, transfer, Error, Operation},
    transport::{Boundary, Sample},
    Mixture,
};
use bandastation_atmos::{
    cell::Cell,
    network::Tile,
    operations::Target,
    phase::{Error as PhaseError, Store, Write},
    Context,
};

fn store_cell(store: &mut Store, x: i32, nitrogen: f32) -> Target {
    let air = sample(nitrogen, 0.0).air;
    let mut tile = Tile::new([x, 1, 1], &air, false);
    tile.excited = false;
    let id = store
        .register(Cell::new(air, Context::default()), tile)
        .unwrap();
    Target::Cell {
        id,
        version: store.binding_version(id).unwrap(),
    }
}

#[test]
fn cell_to_port_transfer_uses_the_same_native_pool_and_checks_both_generations() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 30.0);
    let port = store.pipes.register(sample(0.0, 0.0)).unwrap();
    let removed = store
        .transfer_air(source, Target::Port(port), &Operation::Take(50.0), true)
        .unwrap()
        .unwrap();
    assert_eq!(removed.moles[NITROGEN], 30.0);
    assert_eq!(store.pipes.get(port).unwrap().air.moles[NITROGEN], 30.0);
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 0.0);
    store.pipes.release(port).unwrap();
    let recycled = store.pipes.register(sample(100.0, 0.0)).unwrap();
    assert_eq!(port.slot, recycled.slot);
    assert_ne!(port.generation, recycled.generation);
    let before = store.snapshot();
    assert!(store
        .transfer_air(source, Target::Port(port), &Operation::Take(1.0), true)
        .is_err());
    assert_eq!(before.cells[&1].air, store.snapshot().cells[&1].air);
    assert_eq!(
        store.pipes.get(recycled).unwrap().air.moles[NITROGEN],
        100.0
    );
}

#[test]
fn stale_floor_and_busy_worker_are_rejected_before_issuing_a_sample() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 30.0);
    let destination = store_cell(&mut store, 2, 0.0);
    let invalid = Target::Cell { id: 2, version: 99 };
    assert_eq!(
        store
            .transfer_air(source, invalid, &Operation::Take(50.0), true)
            .unwrap_err(),
        PhaseError::StaleBinding
    );
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 30.0);
    let job = store.begin().unwrap();
    assert_eq!(
        store
            .transfer_air(source, destination, &Operation::Take(50.0), true)
            .unwrap_err(),
        PhaseError::Busy
    );
    store.finish(job.calculate()).unwrap();
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 30.0);
}

#[test]
fn delayed_feedback_uses_post_transport_gas_and_caps_the_next_withdrawal() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 100.0);
    store_cell(&mut store, 2, 0.0);
    store.write(Write::Neighbors(1, vec![2])).unwrap();
    store.write(Write::Neighbors(2, vec![1])).unwrap();
    store.set_processing(vec![1, 2], vec![]).unwrap();
    let job = store.begin_at(1).unwrap();
    let completed = std::thread::spawn(move || job.calculate()).join().unwrap();
    store.stage(completed).unwrap();
    store.publish_ready(1).unwrap();
    let transported = store.snapshot().cells[&2].air.moles[NITROGEN];
    assert!(transported > 0.0);
    store
        .operate(source, &[Operation::AdjustGas(NITROGEN, 10.0)], false)
        .unwrap();
    let total = store
        .snapshot()
        .cells
        .values()
        .map(|cell| cell.air.total_moles())
        .sum::<f32>();
    assert!((total - 110.0).abs() < 0.0001);
    let received = store
        .operate(source, &[Operation::Take(200.0)], false)
        .unwrap()
        .remove(0)
        .removed
        .unwrap();
    assert!((received.total_moles() - (110.0 - transported)).abs() < 0.0001);
    assert_eq!(store.snapshot().cells[&1].air.total_moles(), 0.0);
    assert!(
        (received.total_moles() + store.snapshot().cells[&2].air.total_moles() - 110.0).abs()
            < 0.0001
    );
}

#[test]
fn operation_transaction_sees_prior_withdrawals_and_publishes_the_packet_once() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 30.0);
    let destination = store_cell(&mut store, 2, 0.0);
    store.begin_operations().unwrap();
    let first = store
        .transfer_air(source, destination, &Operation::Take(20.0), true)
        .unwrap()
        .unwrap();
    let second = store
        .transfer_air(source, destination, &Operation::Take(20.0), false)
        .unwrap()
        .unwrap();
    assert_eq!(first.moles[NITROGEN] + second.moles[NITROGEN], 30.0);
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 30.0);
    assert!(matches!(store.begin(), Err(PhaseError::Busy)));
    store.finish_operations(false, false).unwrap();
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 0.0);
    assert_eq!(store.snapshot().cells[&2].air.moles[NITROGEN], 30.0);
    assert!(store.snapshot().schedule.active.contains(&1));
}

#[test]
fn aborted_addition_has_no_public_gas_and_legacy_write_joins_the_pending_sample() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 30.0);
    store.begin_operations().unwrap();
    store
        .operate(source, &[Operation::AdjustGas(NITROGEN, 10.0)], false)
        .unwrap();
    store.finish_operations(true, false).unwrap();
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 30.0);
    store.begin_operations().unwrap();
    store
        .operate(source, &[Operation::AdjustGas(NITROGEN, 10.0)], false)
        .unwrap();
    let modified = sample(45.0, 0.0);
    store
        .write(Write::Commit(1, modified.air, modified.archived, false))
        .unwrap();
    let removed = store
        .operate(source, &[Operation::Take(50.0)], false)
        .unwrap()
        .remove(0)
        .removed
        .unwrap();
    assert_eq!(removed.moles[NITROGEN], 45.0);
    store.finish_operations(false, false).unwrap();
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 0.0);
}

#[test]
fn materializing_a_legacy_mixture_hands_over_pending_gas_without_publishing_it() {
    let mut store = Store::default();
    let source = store_cell(&mut store, 1, 30.0);
    store.begin_operations().unwrap();
    store
        .operate(source, &[Operation::AdjustGas(NITROGEN, 10.0)], false)
        .unwrap();
    let mut pending = store.take_operation_sample(source).unwrap().unwrap();
    assert_eq!(pending.air.moles[NITROGEN], 40.0);
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 30.0);
    pending.air.set(NITROGEN, 41.0);
    store
        .write(Write::Commit(1, pending.air, pending.archived, false))
        .unwrap();
    store.finish_operations(false, false).unwrap();
    assert_eq!(store.snapshot().cells[&1].air.moles[NITROGEN], 41.0);
}

fn sample(nitrogen: f32, oxygen: f32) -> Sample {
    let mut air = Mixture::new(300.0, CELL_VOLUME, true);
    air.set(NITROGEN, nitrogen);
    air.set(OXYGEN, oxygen);
    Sample::new(air)
}

#[test]
fn withdrawal_is_capped_before_returning_the_same_gas_to_the_receiver() {
    for amount in [0.0, -1.0, 1.0, 29.0, 30.0, 50.0, 1e30] {
        let mut source = sample(20.0, 10.0);
        let mut destination = sample(0.0, 0.0);
        let removed = transfer(&mut source, &mut destination, &Operation::Take(amount)).unwrap();
        if amount <= 0.0 {
            assert!(removed.is_none());
            assert_eq!(source.air.total_moles(), 30.0);
            continue;
        }
        let removed = removed.unwrap();
        for key in [NITROGEN, OXYGEN] {
            assert_eq!(destination.air.moles[key], removed.moles[key]);
            let initial = if key == NITROGEN { 20.0 } else { 10.0 };
            assert!((source.air.moles[key] + destination.air.moles[key] - initial).abs() < 0.0001);
            assert!(source.air.moles[key] >= 0.0);
        }
        assert!(removed.total_moles() <= amount.min(30.0) + 0.0002);
    }
}

#[test]
fn repeated_withdrawals_can_never_issue_more_than_the_initial_inventory() {
    let mut source = sample(20.0, 10.0);
    let mut first = sample(0.0, 0.0);
    let mut second = sample(0.0, 0.0);
    transfer(&mut source, &mut first, &Operation::Take(25.0)).unwrap();
    transfer(&mut source, &mut second, &Operation::Take(25.0)).unwrap();
    for key in [NITROGEN, OXYGEN] {
        let expected = if key == NITROGEN { 20.0 } else { 10.0 };
        assert!((first.air.moles[key] + second.air.moles[key] - expected).abs() < 0.0001);
        assert_eq!(source.air.moles[key], 0.0);
    }
}

#[test]
fn specific_and_ratio_withdrawals_keep_composition_and_default_sample_semantics() {
    let mut air = sample(30.0, 20.0);
    air.air.volume = 800.0;
    let results = apply_batch(
        &mut air,
        &[
            Operation::TakeSpecific {
                gas: NITROGEN,
                amount: 50.0,
            },
            Operation::TakeSpecificRatio {
                gas: OXYGEN,
                ratio: 0.25,
            },
            Operation::TakeRatio(0.0),
        ],
    )
    .unwrap();
    let first = results[0].removed.as_ref().unwrap();
    assert_eq!(first.moles[NITROGEN], 30.0);
    assert_eq!(first.moles[OXYGEN], 0.0);
    assert_eq!(first.volume, CELL_VOLUME);
    assert_eq!(results[1].removed.as_ref().unwrap().moles[OXYGEN], 5.0);
    let empty = results[2].removed.as_ref().unwrap();
    assert_eq!(
        (empty.temperature, empty.volume, empty.turf_heat_capacity),
        (TCMB, 800.0, false)
    );
    assert_eq!(air.air.moles[OXYGEN], 15.0);
}

#[test]
fn relative_additions_do_not_restore_gas_that_was_already_transported() {
    let mut source = sample(100.0, 0.0);
    let mut neighbor = sample(0.0, 0.0);
    transfer(&mut source, &mut neighbor, &Operation::Take(50.0)).unwrap();
    apply_batch(&mut source, &[Operation::AdjustGas(NITROGEN, 10.0)]).unwrap();
    assert_eq!(
        (source.air.moles[NITROGEN], neighbor.air.moles[NITROGEN]),
        (60.0, 50.0)
    );
}

#[test]
fn ensure_reset_set_and_adjust_have_distinct_archive_and_gas_semantics() {
    let mut air = sample(30.0, 20.0);
    apply_batch(&mut air, &[Operation::Ensure(NITROGEN)]).unwrap();
    assert_eq!(
        (air.air.moles[NITROGEN], air.archived.moles[NITROGEN]),
        (30.0, 30.0)
    );
    apply_batch(&mut air, &[Operation::Reset(NITROGEN)]).unwrap();
    assert!(air.air.keys().contains(&NITROGEN));
    assert_eq!(
        (air.air.moles[NITROGEN], air.archived.moles[NITROGEN]),
        (0.0, 0.0)
    );
    apply_batch(&mut air, &[Operation::SetGas(NITROGEN, 12.0)]).unwrap();
    assert_eq!(air.air.moles[NITROGEN], 12.0);
    apply_batch(&mut air, &[Operation::AdjustGas(NITROGEN, -50.0)]).unwrap();
    assert_eq!(air.air.moles[NITROGEN], 0.0);
    assert!(!air.air.keys().contains(&NITROGEN));
}

#[test]
fn conversion_checks_reactant_before_crediting_product() {
    let mut air = sample(30.0, 0.0);
    apply_batch(
        &mut air,
        &[Operation::Convert {
            from: NITROGEN,
            to: OXYGEN,
            amount: 50.0,
        }],
    )
    .unwrap();
    assert_eq!(
        (air.air.moles[NITROGEN], air.air.moles[OXYGEN]),
        (0.0, 30.0)
    );
}

#[test]
fn copy_and_copy_ratio_preserve_the_original_distinct_archive_rules() {
    for clear_archive in [true, false] {
        let mut air = sample(30.0, 20.0);
        let original_archive = air.archived.clone();
        air.air.results[0] = Some(123.0);
        let mut replacement = Mixture::new(500.0, 999.0, false);
        replacement.set(PLASMA, 8.0);
        apply_batch(
            &mut air,
            &[Operation::Copy {
                source: Box::new(replacement),
                ratio: 0.5,
                clear_archive,
            }],
        )
        .unwrap();
        assert_eq!(
            (air.air.volume, air.air.temperature, air.air.moles[PLASMA]),
            (CELL_VOLUME, 500.0, 4.0)
        );
        assert_eq!(air.air.results[0], Some(123.0));
        if clear_archive {
            assert_eq!(air.archived.keys(), &[PLASMA]);
            assert_eq!(air.archived.moles[PLASMA], 0.0);
        } else {
            assert_eq!(air.archived, original_archive);
        }
    }
}

#[test]
fn heat_uses_live_capacity_and_archive_is_an_explicit_ordered_operation() {
    let mut air = sample(30.0, 20.0);
    let energy = air.air.heat_capacity() * 25.0;
    apply_batch(
        &mut air,
        &[
            Operation::Heat(energy),
            Operation::Archive,
            Operation::SetTemperature(500.0),
        ],
    )
    .unwrap();
    assert_eq!(
        (air.air.temperature, air.archived.temperature),
        (500.0, 325.0)
    );
    assert_eq!(air.air.volume, CELL_VOLUME);
}

#[test]
fn invalid_tail_and_arithmetic_overflow_leave_the_whole_batch_unchanged() {
    for invalid in [
        Operation::SetGas(GAS_COUNT, 1.0),
        Operation::SetVolume(-1.0),
        Operation::Heat(f32::NAN),
    ] {
        let mut air = sample(30.0, 20.0);
        let before = air.clone();
        assert!(apply_batch(&mut air, &[Operation::AdjustGas(NITROGEN, 10.0), invalid]).is_err());
        assert_eq!(air, before);
    }
    let mut air = sample(30.0, 20.0);
    let before = air.clone();
    assert_eq!(
        apply_batch(
            &mut air,
            &[
                Operation::SetGas(NITROGEN, f32::MAX),
                Operation::AdjustGas(NITROGEN, f32::MAX)
            ]
        )
        .unwrap_err(),
        Error::NonFinite
    );
    assert_eq!(air, before);
}

#[test]
fn overflowing_destination_rolls_back_both_sides_of_a_transfer() {
    let mut source = sample(f32::MAX, 0.0);
    let mut destination = source.clone();
    let before = (source.clone(), destination.clone());
    assert_eq!(
        transfer(
            &mut source,
            &mut destination,
            &Operation::TakeSpecific {
                gas: NITROGEN,
                amount: f32::MAX
            }
        )
        .unwrap_err(),
        Error::NonFinite
    );
    assert_eq!((source, destination), before);
}

#[test]
fn space_withdrawal_returns_empty_and_immutable_archive_is_unchanged() {
    let mut air = Sample::new(Mixture::new(TCMB, CELL_VOLUME, false));
    air.boundary = Boundary::Space;
    let before = air.clone();
    let output = apply_batch(&mut air, &[Operation::Take(50.0), Operation::Archive]).unwrap();
    assert_eq!(output[0].removed_kind, 2);
    assert_eq!(output[0].removed.as_ref().unwrap().total_moles(), 0.0);
    assert_eq!(air, before);
}

#[test]
fn empty_space_receipt_keeps_bandastations_vacuum_heat_capacity_during_transfer() {
    let mut source = Sample::new(Mixture::new(TCMB, CELL_VOLUME, false));
    source.boundary = Boundary::Space;
    let mut destination = sample(30.0, 0.0);
    let removed = transfer(&mut source, &mut destination, &Operation::Take(50.0))
        .unwrap()
        .unwrap();
    assert_eq!(removed.total_moles(), 0.0);
    assert_eq!(removed.heat_capacity(), HEAT_CAPACITY_VACUUM);
    assert_eq!(destination.air.moles[NITROGEN], 30.0);
    assert_eq!(
        destination.air.temperature,
        (TCMB * 7000.0 + 300.0 * 600.0) / 7600.0
    );
}

#[test]
fn multiple_adjustment_preserves_unquantized_values_and_collects_after_all_keys() {
    let mut air = sample(0.00001, 100.0);
    apply_batch(
        &mut air,
        &[Operation::AdjustMany(vec![(OXYGEN, 1.0), (NITROGEN, 1.0)])],
    )
    .unwrap();
    assert_eq!(air.air.keys(), &[NITROGEN, OXYGEN]);
    assert_eq!(air.air.moles[NITROGEN], 1.00001);
    assert_eq!(air.air.moles[OXYGEN], 101.0);
}

#[test]
fn clear_and_metadata_commands_do_not_clear_other_fields() {
    let mut air = sample(30.0, 20.0);
    apply_batch(
        &mut air,
        &[
            Operation::SetArchivedGas(PLASMA, 7.0),
            Operation::SetArchivedTemperature(200.0),
            Operation::SetResult {
                index: 0,
                value: Some(0.0),
            },
            Operation::SetLastShare(17.0),
            Operation::ClearCurrent,
        ],
    )
    .unwrap();
    assert_eq!(air.air.total_moles(), 0.0);
    assert_eq!(air.air.temperature, 300.0);
    assert_eq!(air.archived.moles[PLASMA], 7.0);
    assert_eq!(air.archived.temperature, 200.0);
    assert_eq!(air.air.results[0], Some(0.0));
    assert_eq!(air.last_share, 17.0);
    apply_batch(
        &mut air,
        &[Operation::ClearArchive, Operation::ClearResults],
    )
    .unwrap();
    assert!(air.archived.keys().is_empty());
    assert_eq!(air.air.results[0], None);
    assert_eq!(air.archived.temperature, 200.0);
}

#[test]
fn electrolyze_reuses_the_existing_chemistry_without_running_standard_reactions() {
    let mut air = Sample::new(Mixture::new(300.0, CELL_VOLUME, false));
    air.air.set(WATER_VAPOR, 20.0);
    let context = Context {
        working_power: 1.0,
        ..Context::default()
    };
    apply_batch(&mut air, &[Operation::Electrolyze(context)]).unwrap();
    assert_eq!(
        (
            air.air.moles[WATER_VAPOR],
            air.air.moles[OXYGEN],
            air.air.moles[HYDROGEN]
        ),
        (15.0, 2.5, 5.0)
    );
    // Banda's H2 has specific heat 15; 240 kJ remain in products with capacity 725 J/K.
    assert_eq!(air.air.temperature, 240_000.0 / 725.0);
}

#[test]
fn equalize_keeps_volume_proportions_and_leaves_subthreshold_differences_alone() {
    use bandastation_atmos::operations::{apply_pair, PairOperation};
    let mut source = sample(100.0, 0.0);
    source.air.volume = 1000.0;
    let mut destination = sample(0.0, 0.0);
    destination.air.volume = 3000.0;
    let result = apply_pair(&mut source, &mut destination, &PairOperation::Equalize).unwrap();
    assert_eq!(result.value, Some(1.0));
    assert_eq!(
        (source.air.moles[NITROGEN], destination.air.moles[NITROGEN]),
        (25.0, 75.0)
    );
    let mut source = sample(100.001, 0.0);
    let mut destination = sample(100.0, 0.0);
    let result = apply_pair(&mut source, &mut destination, &PairOperation::Equalize).unwrap();
    assert_eq!(result.value, Some(0.0));
    assert_eq!(
        (source.air.moles[NITROGEN], destination.air.moles[NITROGEN]),
        (100.001, 100.0)
    );
}
