//! Experimental gas-exchange layouts. No production solver or BYOND ABI is replaced.
mod support;

use bandastation_atmos::{transport::Sample, Mixture};
use criterion::{black_box, BatchSize, BenchmarkId, Criterion, Throughput};
use serde_json::{json, Value};
use std::{fs, path::Path, time::Duration};
use support::{
    storage::{avx2_available, Aos, Simd, Soa, Storage},
    World,
};

fn number(value: &Value) -> f32 {
    let value = value
        .as_f64()
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap()) as f32;
    assert!(value.is_finite());
    value
}

fn state<S: Storage>(world: &World<S>, id: usize) -> Value {
    json!({"temperature": world.current.temperature(id), "moles": (0..20).map(|g| world.current.get(id,g)).collect::<Vec<_>>(),
        "keys": world.metadata[id].order.keys(), "archived_temperature": world.archived.temperature(id),
        "archived_moles": (0..20).map(|g| world.archived.get(id,g)).collect::<Vec<_>>(), "archived_keys": world.metadata[id].archived_order.keys()})
}

fn verify<S: Storage>(input: &Value) -> Value {
    let mut world = World::<S>::new(2);
    for (id, tag) in ["a", "b"].iter().enumerate() {
        let seed = &input[tag];
        world
            .current
            .set_temperature(id, number(&seed["temperature"]));
        world.metadata[id].volume = number(&seed["volume"]);
        world.metadata[id].turf = seed["turf"].as_bool().unwrap();
        for pair in seed["gases"].as_array().unwrap() {
            world.set_gas(id, pair[0].as_u64().unwrap() as usize, number(&pair[1]));
        }
    }
    let mut values = Vec::new();
    let mut states = Vec::new();
    let ticks = input["ticks"].as_u64().unwrap();
    assert!((1..=10_000).contains(&ticks));
    for tick in 0..ticks {
        if input["drive"].as_bool().unwrap_or(false) && tick % 32 == 0 {
            world.set_gas(0, 0, 100.0 + (tick % 31) as f32);
            world.set_gas(1, 3, 20.0 + (tick % 13) as f32);
            world
                .current
                .set_temperature(0, number(&input["a"]["temperature"]));
            world
                .current
                .set_temperature(1, number(&input["b"]["temperature"]));
        }
        world.archive(0);
        world.archive(1);
        for _ in 0..input["repeats"].as_u64().unwrap() {
            let value = if input["mode"] == 0 {
                world.share(0, 1, number(&input["ca"]), number(&input["cb"]))
            } else {
                Some(world.temperature_share(0, 1, number(&input["ca"])))
            };
            values.push(value);
            states.push(json!({"a":state(&world,0), "b":state(&world,1), "last_share":world.metadata[0].last_share}));
        }
    }
    json!({"values":values, "states":states})
}

fn production_state(sample: &Sample) -> Value {
    json!({"temperature":sample.air.temperature, "moles":&*sample.air.moles, "keys":sample.air.keys(),
        "archived_temperature":sample.archived.temperature, "archived_moles":&*sample.archived.moles,
        "archived_keys":sample.archived.keys()})
}

fn verify_production(input: &Value) -> Value {
    let mut samples = Vec::new();
    for tag in ["a", "b"] {
        let seed = &input[tag];
        let mut air = Mixture::new(
            number(&seed["temperature"]),
            number(&seed["volume"]),
            seed["turf"].as_bool().unwrap(),
        );
        for pair in seed["gases"].as_array().unwrap() {
            air.set(pair[0].as_u64().unwrap() as usize, number(&pair[1]));
        }
        samples.push(Sample::new(air));
    }
    let (left, right) = samples.split_at_mut(1);
    let (a, b) = (&mut left[0], &mut right[0]);
    let mut values = Vec::new();
    let mut states = Vec::new();
    for tick in 0..input["ticks"].as_u64().unwrap() {
        if input["drive"].as_bool().unwrap_or(false) && tick % 32 == 0 {
            a.air.set(0, 100.0 + (tick % 31) as f32);
            b.air.set(3, 20.0 + (tick % 13) as f32);
            a.air.temperature = number(&input["a"]["temperature"]);
            b.air.temperature = number(&input["b"]["temperature"]);
        }
        a.archive();
        b.archive();
        for _ in 0..input["repeats"].as_u64().unwrap() {
            values.push(if input["mode"] == 0 {
                a.share(b, number(&input["ca"]), number(&input["cb"]))
            } else {
                Some(a.temperature_share(b, number(&input["ca"])))
            });
            states.push(
                json!({"a":production_state(a),"b":production_state(b),"last_share":a.last_share}),
            );
        }
    }
    json!({"values":values,"states":states})
}

fn measure_production(criterion: &mut Criterion) {
    let name = if cfg!(feature = "simd") && avx2_available() {
        "avx2_current20"
    } else {
        "scalar_current20"
    };
    for (count, gases, thermal) in [
        (64, 2, false),
        (4096, 4, false),
        (4096, 4, true),
        (4096, 20, true),
        (65025, 4, true),
        (65025, 20, true),
    ] {
        let mut group = criterion.benchmark_group(format!(
            "production_share/{count}_tiles/{gases}_gases/thermal_{thermal}"
        ));
        group.throughput(Throughput::Elements((count - 1) as u64));
        group.bench_function(name, |b| {
            b.iter_batched_ref(
                || {
                    (0..count)
                        .map(|id| {
                            let mut air = Mixture::new(
                                if thermal {
                                    200.0 + ((id * 79) % 1600) as f32
                                } else {
                                    293.15
                                },
                                2500.0,
                                true,
                            );
                            for gas in 0..gases {
                                air.set(gas, 1.0 + ((id * 17 + gas * 31) % 1000) as f32);
                            }
                            Sample::new(air)
                        })
                        .collect::<Vec<_>>()
                },
                |samples| {
                    for id in 0..count - 1 {
                        let (left, right) = samples.split_at_mut(id + 1);
                        black_box(left[id].share(&mut right[0], 0.2, 1.0 / 7.0));
                    }
                },
                BatchSize::LargeInput,
            );
        });
        group.finish();
    }
}

fn seed<S: Storage>(count: usize, gases: usize, thermal: bool) -> World<S> {
    let mut world = World::<S>::new(count);
    for id in 0..count {
        world.current.set_temperature(
            id,
            if thermal {
                200.0 + ((id * 79) % 1600) as f32
            } else {
                293.15
            },
        );
        for gas in 0..gases {
            world.set_gas(id, gas, 1.0 + ((id * 17 + gas * 31) % 1000) as f32);
        }
        world.archive(id);
    }
    world
}

fn measure<S: Storage>(criterion: &mut Criterion, name: &str) {
    for (count, gases, thermal) in [
        (64, 2, false),
        (4096, 20, false),
        (4096, 20, true),
        (65025, 20, true),
    ] {
        let mut group = criterion.benchmark_group(format!(
            "share/{count}_tiles/{gases}_gases/thermal_{thermal}"
        ));
        // Each fixture preserves this ordered tape; changing storage never sorts links.
        let edges: Vec<_> = (0..count - 1).map(|id| (id, id + 1)).collect();
        group.throughput(Throughput::Elements(edges.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &count, |b, &size| {
            b.iter_batched_ref(
                || seed::<S>(size, gases, thermal),
                |world| {
                    for &(a, b) in &edges {
                        black_box(world.share(a, b, 0.2, 1.0 / 7.0));
                    }
                },
                BatchSize::LargeInput,
            );
        });
        group.finish();
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--verify") {
        assert_eq!(args.len(), 4, "--verify <input.json> <new-output.json>");
        let input: Value = serde_json::from_str(&fs::read_to_string(&args[2]).unwrap()).unwrap();
        let cases = input.as_array().unwrap();
        assert!(
            !Path::new(&args[3]).exists(),
            "Preserve previous verification output"
        );
        let output = json!({"avx2":avx2_available(), "production_simd_compiled":cfg!(feature="simd"),
            "production":cases.iter().map(verify_production).collect::<Vec<_>>(), "aos":cases.iter().map(verify::<Aos>).collect::<Vec<_>>(),
            "simd":if avx2_available() { Some(cases.iter().map(verify::<Simd>).collect::<Vec<_>>()) } else { None },
            "soa":cases.iter().map(verify::<Soa>).collect::<Vec<_>>()});
        fs::write(&args[3], serde_json::to_vec(&output).unwrap()).unwrap();
        return;
    }
    if args.get(1).map(String::as_str) == Some("--layout") {
        println!(
            "{}",
            json!({"avx2":avx2_available(), "tiles":65025,
            "aos_bytes":seed::<Aos>(65025,20,true).bytes(),
            "simd_bytes":if avx2_available() {Some(seed::<Simd>(65025,20,true).bytes())} else {None},
            "soa_bytes":seed::<Soa>(65025,20,true).bytes(),
            "note":"Current + archived gas/temperature and shared metadata; excludes topology, groups, effects, engine and BYOND."})
        );
        return;
    }
    let mut criterion = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .configure_from_args();
    measure::<Aos>(&mut criterion, "aos20");
    if avx2_available() {
        measure::<Simd>(&mut criterion, "avx2_24");
    }
    measure::<Soa>(&mut criterion, "soa20");
    measure_production(&mut criterion);
    criterion.final_summary();
}
