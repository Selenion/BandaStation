"""Exercise the actual native phase DLL and DM effect executor in an isolated Banda world."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import catalog
import transport
from native_probe import probe

ROOT = catalog.ROOT
sys.path.insert(0, str(ROOT / "tools/profiling"))
from run import stage
from runtime_check import validate_runtime_logs


def original_air_consumers(output):
    """Keep an independent pre-migration oracle rather than comparing two changed callers."""
    revision = "c65fbacb60bddd493e079d4aa03456aae52dee64"
    destination = output / "original-air-consumers.dm.in"
    # A checkout must remain testable after squash/rebase or from a shallow clone.
    shutil.copy2(ROOT / "tools/atmos/original_air_consumers.dm.in", destination)
    return destination, dict(revision=revision, sha256=hashlib.sha256(destination.read_bytes()).hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--track-max-share", action="store_true")
    parser.add_argument("--compiled-binary", type=Path, help="Reuse an unchanged DM build for DLL-only comparisons")
    parser.add_argument("--dll", type=Path, default=ROOT / "rust/atmos/target/i686-pc-windows-msvc/debug/bandastation_atmos.dll")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    output = ROOT / "tmp/atmos-phase" / args.name
    if output.exists():
        parser.error("Use a new name to preserve earlier results")
    subprocess.run([sys.executable, str(ROOT / "tools/atmos/catalog.py"), "--check"], cwd=ROOT, check=True)
    base = json.loads((ROOT / "tools/atmos/world-tick-reference.json").read_text(encoding="utf-8"))
    pairs = list(zip(base["input"], base["observed"]["cases"]))
    inputs = [pair[0] for pair in pairs]
    network = json.loads((ROOT / "tools/atmos/integration-world-reference.json").read_text(encoding="utf-8"))
    networks = [dict(input=i,expected=e) for i,e in zip(network["input"],network["observed"])]
    output.mkdir(parents=True)
    stage(output)
    with (output / "config/config.txt").open("a", encoding="utf-8") as config:
        config.write("\nFORBID_STATION_TRAITS\n")
    shutil.copy2(args.dll, output / "bandastation_atmos.dll")
    probe(args.byond, output)
    shutil.copy2(ROOT / "_maps/map_jsons/metastation.json", output / "data/next_map.json")
    (output / "phase-cases.json").write_text(json.dumps(inputs), encoding="utf-8")
    (output / "phase-network-cases.json").write_text(json.dumps([c["input"] for c in networks]), encoding="utf-8")
    original, original_hash = original_air_consumers(output)
    dme = ROOT / "local-atmos-phase.test.dme"
    fixtures = ("world_network", "phase_world", "phase_network", "inherit_world", "binding_world", "deferred_world", "api_world", "properties_world", "values_world", "live_reference_world", "original_life_consumers", "life_world", "vent_world", "scrubber_world", "contact_world", "debug_world", "exchange_world", "operations_world", "threaded_world", "feedback_world", "threaded_error_world", "teardown_world", "completion_world", "budget_boundary_world", "cycle_world", "effect_boundary_world", "chunk_world")
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig") + "\n" + "".join(
        f'#include "tools/atmos/{fixture}.dm.in"\n' for fixture in fixtures) + f'\n#include "{original.as_posix()}"\n', encoding="utf-8", newline="\n")
    fixture_paths = [f"tools/atmos/{fixture}.dm.in" for fixture in fixtures]
    fixture_hashes = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in fixture_paths}
    print("Compiling native phase integration harness", flush=True)
    defines = ["-DCBT"] + (["-DTRACK_MAX_SHARE"] if args.track_max_share else [])
    if not args.compiled_binary:
        subprocess.run([str(args.byond / ("dm.exe" if os.name == "nt" else "DreamMaker")), *defines, str(dme)], cwd=ROOT, check=True, timeout=300)
    binary = args.compiled_binary or dme.with_suffix(".dmb")
    for extension in ("dmb", "rsc"):
        shutil.copy2(binary.with_suffix("." + extension), output / ("tgstation." + extension))
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(args.byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")), "tgstation.dmb", "-trusted", "-close", "-invisible",
                        "-ip", "127.0.0.1", "-logself", "-params", "log-directory=local-atmos-phase"],
                       cwd=output, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
    logs = "\n".join(p.read_text(encoding="utf-8-sig", errors="replace") for p in output.rglob("*") if p.is_file() and (p.suffix == ".log" or p.name.endswith(".log.json")))
    runtime = validate_runtime_logs(output)
    (output / "runtime-validation.json").write_text(json.dumps(runtime, indent=2, ensure_ascii=False), encoding="utf-8")
    if runtime["unexpected_count"] or "infinite loop" in logs.lower():
        raise SystemExit(f"Runtime in native phase integration; inspect {output}")
    actual = json.loads((output / "phase-output.json").read_text(encoding="utf-8-sig"))
    assert [c["id"] for c in actual["cases"]] == [c["id"] for c in inputs], "Incomplete native run"
    failures = []
    for (_, reference), observed in zip(pairs, actual["cases"]):
        expected = {key: reference[key] for key in ("id", "flags", "temperature", "gases", "hotspot_temperature", "hotspot_volume",
                    "wet_strength", "item_frozen", "protected_item_frozen", "person_freon", "fire_calls")}
        expected["native_hotspot"] = reference["hotspot"]
        # The approved phase policy changes feedback timing, not the numeric tolerances.
        difference = state_difference(expected, observed)
        if difference:
            failures.append(dict(id=reference["id"], difference=difference, expected=expected, actual=observed))
    checks = actual["checks"]
    assert [c["label"] for c in actual["networks"]] == [c["input"]["label"] for c in networks], "Incomplete graph integration"
    for reference, observed in zip(networks, actual["networks"]):
        expected = reference["expected"]
        difference = transport.difference(expected, observed["states"])
        if difference:
            failures.append(dict(id=reference["input"]["label"], difference=difference, expected=expected, actual=observed["states"]))
    required = {"concurrent_write_exercised", "release_not_lost", "safe_callback_applied_once", "detached_snapshot_unchanged",
                "commands_not_replayed", "replacement_not_overwritten", "old_freeze_not_applied", "owner_closed_during_work",
                "nested_callbacks_share_air", "release_inside_callback_not_lost", "closing_preserves_external_gas",
                "native_output_packet_released", "native_input_packet_released"}
    assert set(checks) == required, "Incomplete lifetime/queue checks"
    for key, value in checks.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed phase/lifetime check"))
    inheritance = actual["inheritance"]
    assert all(inheritance[k] == 1 for k in ("source_keeps_group", "source_keeps_cooldowns", "source_keeps_gas")), "Original ChangeTurf contract not established"
    if inheritance["native_groups_before"] != 1 or inheritance["native_groups_after"] != 1:
        failures.append(dict(id="inherit_air_group", difference="Inherited native group was lost", observed=inheritance))
    for key in ("native_keeps_cell", "native_changes_binding_version", "native_keeps_group_state", "native_keeps_gas"):
        if inheritance[key] != 1:
            failures.append(dict(id=key,difference="Failed inherited binding contract",observed=inheritance))
    binding_checks = actual["bindings"]
    assert len(binding_checks) == 16, "Incomplete world binding checks"
    for key,value in binding_checks.items():
        if value != 1:
            failures.append(dict(id=key,difference="Failed world binding check"))
    deferred = actual["deferred"]
    assert len(deferred) == 7, "Incomplete deferred initialization checks"
    for key, value in deferred.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed deferred initialization check"))
    api = actual["api"]
    assert len(api) == 245, "Incomplete lazy world API/property checks"
    for key, value in api.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed lazy world API check"))
    vents = actual["vents"]
    assert set(vents) == {"release", "siphon", "depleted_port", "empty_siphon", "tiny_release", "tiny_siphon", "chain", "held_tile", "port_signal", "siphon_signal", "machine_error", "benchmark"}
    for mode, pair in ((key, pair) for key, pair in vents.items() if key != "benchmark"):
        source, native = pair["dm"], pair["rust"]
        fallback = mode in ("held_tile", "port_signal", "siphon_signal")
        empty = mode in ("empty_siphon", "tiny_release", "tiny_siphon")
        moves = 2 if mode == "chain" else int(not fallback and not empty)
        diff = transport.difference(source["state"], native["state"], mode)
        if diff or native["moves"] != moves or native["fallbacks"] != int(fallback) or native["empty"] != int(empty):
            failures.append(dict(id=f"vent_{mode}", difference=diff or "Wrong transfer path", observed=pair))
        if not (source["aliases_kept"] and native["aliases_kept"] and native["scope_closed"]) or (not fallback and native["full_cache"]):
            failures.append(dict(id=f"vent_{mode}_lifetime", difference="Alias, scope or cache contract failed", observed=pair))
    assert vents["chain"]["dm"]["state"]["second_updates"] == 1, "Second vent did not exercise the fresh reading"
    for mode in ("port_signal", "siphon_signal"):
        assert vents[mode]["dm"]["state"]["redirected"] and vents[mode]["dm"]["state"]["signal_calls"] == 1
    assert vents["empty_siphon"]["dm"]["state"]["excited"] and not vents["empty_siphon"]["dm"]["state"]["updates"]
    assert vents["machine_error"]["dm"]["state"]["caught"] and vents["machine_error"]["dm"]["state"]["updates"] == 1
    for guard in ("stale_binding", "rejected_output"):
        if vents["release"]["rust"]["guards"].get(guard) != 1:
            failures.append(dict(id=f"vent_{guard}", difference="Direct transfer did not reject an invalid call"))
    bench = {(row["repeat"], row["mode"]): row for row in vents["benchmark"]}
    assert set(bench) == {(repeat, mode) for repeat in range(1, 4) for mode in ("dm", "rust")}
    for repeat in range(1, 4):
        control, direct = bench[repeat, "dm"], bench[repeat, "rust"]
        assert all(row["loop_seconds"] > 0 and row["vent_seconds"] > 0 for row in (control, direct))
        if not (control["updates"] == direct["updates"] == direct["moves"] == 32 * 320
                and not direct["fallbacks"] and control["full_caches"] == 32 and not direct["full_caches"]):
            failures.append(dict(id=f"vent_benchmark_{repeat}_coverage", difference="Wrong workload or cache path"))
        diff = transport.difference(control["states"], direct["states"], "vent_benchmark")
        if diff:
            failures.append(dict(id=f"vent_benchmark_{repeat}_state", difference=diff))
    scrubbers = actual["scrubbers"]
    failures.extend(check_scrubbers(scrubbers))
    contacts = actual["contacts"]
    assert contacts["dm"]["hot"] > 0 and contacts["dm"]["cold"] == 0, "DM contact contract not established"
    if contacts["native"]["hot"] != contacts["dm"]["hot"] or contacts["native"]["cold"] != contacts["dm"]["cold"]:
        failures.append(dict(id="native_contact_burn", difference="Contact exposure differs from original DM", observed=contacts))
    debug = actual["debug"]
    threaded = actual["threaded"]
    completion = actual["completion"]
    assert len(completion) == 11, "Incomplete worker completion checks"
    for key, value in completion.items():
        if value != 1:
            failures.append(dict(id=key, difference="Worker completion boundary differs"))
    assert len(threaded) == 31, "Incomplete threaded world checks"
    for key, value in threaded.items():
        if value != 1:
            failures.append(dict(id=key, difference="Threaded world behavior differs"))
    budget_boundary = actual["budget_boundary"]
    assert set(budget_boundary) == {"current", "next_mc"}
    for state in budget_boundary.values():
        assert state["coal_deleted"] == 1 and state["completed_ticks"] == 2, "Incomplete budget boundary diagnostic"
    for side in ("left", "right"):
        difference = transport.difference(budget_boundary["current"][side], budget_boundary["next_mc"][side], "budget_boundary." + side)
        if difference:
            failures.append(dict(id="deferred_effect_cycle", difference=difference))
    cycles = actual["cycles"]
    assert len(cycles) == 5, "Incomplete frame cycle checks"
    for key, value in cycles.items():
        if value != 1:
            failures.append(dict(id=key, difference="Effect frame cycle differs"))
    effect_boundary = actual["effect_boundary"]
    assert set(effect_boundary) == {"dm", "atomic", "split"}, "Incomplete live-list boundary diagnostic"
    assert all(s["fire_calls"] == 1 for s in effect_boundary.values())
    effect_errors = actual["effect_errors"]
    for label, cases in (("effect_boundary", effect_boundary), ("effect_errors", effect_errors)):
        assert set(cases) == {"dm", "atomic", "split"}
        assert cases["dm"]["native_n2"] == 103 and cases["dm"]["caught"] == (label == "effect_errors")
        for mode in ("atomic", "split"):
            if cases[mode] != cases["dm"]:
                failures.append(dict(id=label + "_" + mode, difference="Live-list writes differ from DM", observed=cases))
    retained = actual["retained"]
    assert len(retained) == 11, "Incomplete retained mixture lifetime checks"
    for key, value in retained.items():
        if value != 1:
            failures.append(dict(id=key, difference="Retained mixture lifetime differs"))
    chunks = actual["chunks"]
    assert len(chunks["checks"]) == 9, "Incomplete MC chunk checks"
    for key, value in chunks["checks"].items():
        if value != 1:
            failures.append(dict(id=key, difference="MC chunk contract differs", observed=chunks))
    if chunks["visibility"]["api_readonly"] != chunks["visibility"]["held"]:
        failures.append(dict(id="chunk_api_reads_retained_air", difference="Read API returned a stale retained mixture", observed=chunks))
    feedback = actual["feedback"]
    threaded_errors = actual["threaded_errors"]
    assert set(threaded_errors) == {"dm", "sync", "worker"}, "Incomplete item failure comparison"
    for mode, state in threaded_errors.items():
        if state["first"].get("error") != "BANDA_EXPECTED_ITEM_FIRE_ERROR" or state["first"]["done"] != 1 or state["fire_calls_before_retry"] != 1:
            failures.append(dict(id=f"{mode}_item_error_propagates", observed=state))
        if mode == "dm":
            if state["second"].get("success") != 1:
                failures.append(dict(id="dm_continues_after_item_error", observed=state))
            continue
        for key in ("released_worker", "retry_keeps_gas", "retry_does_not_replay_effect", "failed_effect_not_counted_successful", "next_mc_tick_completes"):
            if state[key] != 1:
                failures.append(dict(id=f"{mode}_{key}", observed=state))
    difference = transport.difference(threaded_errors["sync"]["published"], threaded_errors["worker"]["published"], "item_error.published")
    if difference:
        failures.append(dict(id="item_error_keeps_published_air", difference=difference))
    for state in feedback.values():
        assert state["completed_ticks"] == 2 and state["first"]["coal_deleted"] == 1, "Incomplete coal/equipment feedback check"
    for step in ("first", "final"):
        synchronous = feedback["sync"] if step == "final" else feedback["sync"]["first"]
        worker = feedback["worker"] if step == "final" else feedback["worker"]["first"]
        for side in ("left", "right"):
            difference = transport.difference(synchronous[side], worker[side], f"feedback.{step}.{side}")
            if difference:
                failures.append(dict(id="equipment_after_coal_feedback", difference=difference, observed=feedback))
    exchange = actual["exchange"]
    exchange_error = actual["exchange_error"]
    exchange_effects = actual["exchange_effects"]
    exchange_spawn = actual["exchange_spawn"]
    assert len(exchange_spawn) == 6, "Incomplete shared gas-spawn helper checks"
    for key, value in exchange_spawn.items():
        if value != 1:
            failures.append(dict(id=key, difference="Shared gas-spawn helper differs from DM"))
    assert len(exchange_effects) == 5, "Incomplete lazy effect checks"
    for key, value in exchange_effects.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed lazy effect check"))
    if not exchange_error["dm_caught"] or not exchange_error["rust_caught"] or exchange_error["dm_moles"] != exchange_error["rust_moles"]:
        failures.append(dict(id="equipment_exception_preserves_banda_writes", difference="Equipment failure changes committed air", observed=exchange_error))
    assert len(exchange) == 83, "Incomplete native exchange/operation checks"
    for key, value in exchange.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed native exchange check"))
    assert len(debug) == 21, "Incomplete native admin panel checks"
    for key, value in debug.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed native admin panel check"))
    teardown = actual["teardown"]
    assert set(teardown) == {"space", "baseturf", "inherit", "stationary", "passive_change_keeps_air_lazy"}, "Incomplete fuel teardown cases"
    if teardown["passive_change_keeps_air_lazy"] != 1:
        failures.append(dict(id="passive_change_keeps_air_lazy", difference="Replacement eagerly materialized old air"))
    for transition in ("space", "baseturf", "inherit", "stationary"):
        pair = teardown[transition]
        source, native = pair["dm"], pair["rust"]
        assert source["exposures"] or transition == "stationary", f"Fuel pool did not expose contents during DM {transition} teardown"
        for key in ("before", "exposures", "after"):
            difference = transport.difference(source[key], native[key], f"{transition}.{key}")
            if difference:
                failures.append(dict(id="fuel_teardown", difference=difference))
        for key in ("pool_burns_after_change", "hotspots_after_four_ticks", "pool_burns_after_four_ticks"):
            if source.get(key) != native.get(key):
                failures.append(dict(id=f"fuel_teardown_{transition}_{key}", difference="Fuel/hotspot lifecycle differs", observed=pair))
        required = ("binding_preserved",) if transition == "stationary" else ("binding_preserved", "stale_alias_rejected", "stale_write_keeps_new_air")
        for key in required:
            if native[key] != 1:
                failures.append(dict(id=f"fuel_teardown_{transition}_{key}", difference="New floor binding is not protected", observed=pair))
        if transition != "inherit":
            # Binding/type differ by backend; the observable lifetime and presentation must agree per tick.
            assert len(source["hotspot_trace"]) == len(native["hotspot_trace"]) == 5, "Incomplete per-tick hotspot trace"
            for index, (source_spots, native_spots) in enumerate(zip(source["hotspot_trace"], native["hotspot_trace"])):
                if any(not spot[1] for spot in native_spots):
                    failures.append(dict(id="fuel_hotspot_unbound", difference=f"{transition}.tick{index}: unbound hotspot"))
                difference = transport.difference([s[2:5] for s in source_spots], [s[2:5] for s in native_spots], f"{transition}.tick{index}")
                if difference:
                    failures.append(dict(id="fuel_teardown_tick_lifetime", difference=difference))
            difference = transport.difference(source["pool_trace"], native["pool_trace"], f"{transition}.pool_trace")
            if difference:
                failures.append(dict(id="fuel_teardown_pool_lifetime", difference=difference))
        if transition == "stationary":
            assert any(spot[5] for tick in native["hotspot_trace"] for spot in tick), "Publication-time constructor was not exercised"
    report = dict(policy="DM object feedback between native calculations; approved by user", byond=actual["byond"], startup_seed=actual["startup_seed"], track_max_share=args.track_max_share,
                  dll_sha256=hashlib.sha256((output / "bandastation_atmos.dll").read_bytes()).hexdigest(),
                  binary_sha256=hashlib.sha256((output / "tgstation.dmb").read_bytes()).hexdigest(), feedback=feedback, threaded_errors=threaded_errors,
                  harness_sha256=fixture_hashes["tools/atmos/phase_world.dm.in"],
                  fixture_sha256=fixture_hashes, original_consumers=original_hash,
                  scenarios=len(pairs), network_scenarios=len(networks), network_ticks=sum(c["input"]["ticks"] for c in networks), checks=checks, inheritance=inheritance, bindings=binding_checks, deferred=deferred, api=api, vents=vents, scrubbers=scrubbers, contacts=contacts, debug=debug, exchange=exchange, exchange_error=exchange_error, exchange_effects=exchange_effects, exchange_spawn=exchange_spawn, threaded=threaded, completion=completion, budget_boundary=budget_boundary, cycles=cycles, effect_boundary=effect_boundary, effect_errors=effect_errors, retained=retained, chunks=chunks, teardown=teardown, failures=failures)
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    print(f"Native phase: {len(pairs)} world scenarios, {len(networks)} graphs, {len(checks)} queue/lifetime checks, {len(vents) - 1} vent and {len(scrubbers) - 2 + len(scrubbers['admission'])} scrubber scenarios, {len(failures)} failures")
    for failure in failures:
        print(f"  {failure['id']}: {failure['difference']}")
    raise SystemExit(bool(failures))


def state_difference(expected, actual):
    if expected["id"] != actual["id"]:
        return "Wrong scenario"
    for key in ("flags", "native_hotspot", "wet_strength", "item_frozen", "protected_item_frozen", "person_freon", "fire_calls"):
        if expected[key] != actual[key]:
            return f"{key}: {expected[key]} != {actual[key]}"
    for key in ("temperature", "hotspot_temperature", "hotspot_volume"):
        difference = transport.difference(expected[key], actual[key], key)
        if difference:
            return difference
    if [p[0] for p in expected["gases"]] != [p[0] for p in actual["gases"]]:
        return "Gas keys/order changed"
    return transport.difference([p[1] for p in expected["gases"]], [p[1] for p in actual["gases"]], "moles")


def check_scrubbers(cases):
    # Direct moves, empty operations, fallbacks, and notifications to the pipe network.
    expected = {
        "filter": (1, 0, 0, 1), "trace": (1, 0, 0, 1), "zero_filter": (0, 1, 0, 0),
        "purge": (1, 0, 0, 1), "empty_purge": (0, 1, 0, 1), "tiny_purge": (0, 1, 0, 1),
        "unselected": (0, 0, 0, 0), "pressure": (0, 0, 0, 0), "wide": (2, 1, 0, 2),
        "wide_limit": (1, 0, 0, 1), "signal": (0, 0, 1, 1), "held": (1, 0, 0, 1),
        "read_in_callback": (1, 0, 0, 1), "render_in_callback": (1, 0, 0, 1), "exception": (1, 0, 0, 1),
        "wide_exception": (2, 1, 0, 2),
    }
    cold = {
        "filter": (1, 1, 0), "trace": (1, 1, 0), "zero_filter": (1, 0, 0),
        "purge": (1, 1, 0), "empty_purge": (1, 0, 0), "tiny_purge": (1, 0, 0),
        "unselected": (0, 0, 0), "pressure": (0, 0, 0), "wide": (4, 2, 0),
        "wide_limit": (1, 1, 0), "signal": (0, 0, 1), "held": (0, 0, 1),
        "read_in_callback": (1, 1, 1), "render_in_callback": (1, 1, 1), "exception": (1, 1, 1), "wide_exception": (4, 2, 3),
    }
    assert set(cases) == expected.keys() | {"benchmark", "admission"}
    failures = []
    for mode, counts in expected.items():
        control, direct = (cases[mode][key] for key in ("control", "direct"))
        diff = transport.difference(control["state"], direct["state"], mode)
        observed = (direct["moves"], direct["empty"], direct["fallbacks"], direct["state"]["updates"])
        if diff or observed != counts or not (control["aliases"] and direct["aliases"]):
            failures.append(dict(id=f"scrubber_{mode}", difference=diff or "Wrong path, effects or mixture aliases", observed=cases[mode]))
        if tuple(direct[key] for key in ("cold_ops", "cold_moves", "cached_tiles")) != cold[mode]:
            failures.append(dict(id=f"scrubber_{mode}_cache", difference="Wrong native tile path or unexpected DM cache", observed=direct))
    assert cases["signal"]["control"]["state"]["signals"] == 1
    assert cases["exception"]["control"]["state"]["caught"] == 1
    assert cases["wide_exception"]["control"]["state"]["caught"] == 1
    assert cases["render_in_callback"]["control"]["state"]["rendered_overlays"] == 0
    for mode, rows in cases["admission"].items():
        control, direct = rows["control"], rows["direct"]
        scheduled = mode not in ("absent", "thermal")
        received_air = mode == "mixed"
        if (control != dict(scheduled=int(scheduled), received_air=int(received_air), cache=int(mode != "thermal"), compact=0)
                or direct != dict(scheduled=int(scheduled), received_air=int(received_air), cache=int(received_air),
                                  compact=int(mode not in ("thermal", "mixed")))):
            failures.append(dict(id=f"scrubber_admission_{mode}", difference="Scheduling, exposure signal or cache changed", observed=rows))
    bench = {(row["repeat"], row["mode"]): row for row in cases["benchmark"]}
    assert set(bench) == {(repeat, mode) for repeat in range(1, 4) for mode in ("control", "direct")}
    for repeat in range(1, 4):
        control, direct = (bench[repeat, mode] for mode in ("control", "direct"))
        assert all(row["loop_seconds"] > 0 and row["scrub_seconds"] > 0 for row in (control, direct))
        if (any(row["updates"] != 8192 or row["moves"] != 8192 or row["fallbacks"] for row in (control, direct))
                or control["cached_tiles"] != 32 or control["cold_moves"] != 0
                or direct["cached_tiles"] != 0 or direct["cold_moves"] != 8192):
            failures.append(dict(id=f"scrubber_benchmark_{repeat}_coverage", difference="Wrong workload or transfer path"))
        diff = transport.difference(control["states"], direct["states"], "scrubber_benchmark")
        if diff:
            failures.append(dict(id=f"scrubber_benchmark_{repeat}_state", difference=diff))
    return failures


if __name__ == "__main__":
    main()
