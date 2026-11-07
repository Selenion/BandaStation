//! Active pipenet arithmetic only; excludes DM, FFI, topology and reactions.
use bandastation_atmos::{catalog::GAS_COUNT, pipenet, transport::Sample, Mixture};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipenet_active_20_gases");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(1));
    for count in [2, 16, 128] {
        let mut mixtures: Vec<_> = (0..count)
            .map(|id| {
                let mut air = Mixture::new(300.0 + id as f32, 200.0, false);
                for gas in 0..GAS_COUNT {
                    air.set(gas, (gas + id + 1) as f32);
                }
                Sample::new(air)
            })
            .collect();
        let order: Vec<_> = (0..count).collect();
        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(BenchmarkId::from_parameter(count), |b| {
            b.iter(|| {
                // A changing input keeps this an active calculation, not a skipped idle pipenet.
                mixtures[0].air.moles[0] = black_box(100.0);
                pipenet::reconcile(black_box(&mut mixtures), black_box(&order)).unwrap();
                black_box(mixtures[0].air.temperature)
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
