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
    dme = ROOT / "local-atmos-phase.test.dme"
    fixtures = ("world_network", "phase_world", "phase_network", "inherit_world", "binding_world", "deferred_world", "api_world", "contact_world", "debug_world", "exchange_world")
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig") + "\n" + "".join(
        f'#include "tools/atmos/{fixture}.dm.in"\n' for fixture in fixtures), encoding="utf-8", newline="\n")
    fixture_hashes = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in (
        "tools/atmos/phase_world.dm.in", "tools/atmos/phase_network.dm.in", "tools/atmos/inherit_world.dm.in",
        "tools/atmos/binding_world.dm.in", "tools/atmos/deferred_world.dm.in", "tools/atmos/api_world.dm.in", "tools/atmos/contact_world.dm.in", "tools/atmos/debug_world.dm.in", "tools/atmos/world_network.dm.in", "tools/atmos/exchange_world.dm.in")}
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
    if any(marker in logs.lower() for marker in ("runtime error:", "runtime in ", "infinite loop")):
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
                "nested_callbacks_share_air", "release_inside_callback_not_lost", "closing_preserves_external_gas"}
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
    assert len(api) == 32, "Incomplete lazy world API checks"
    for key, value in api.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed lazy world API check"))
    contacts = actual["contacts"]
    assert contacts["dm"]["hot"] > 0 and contacts["dm"]["cold"] == 0, "DM contact contract not established"
    if contacts["native"]["hot"] != contacts["dm"]["hot"] or contacts["native"]["cold"] != contacts["dm"]["cold"]:
        failures.append(dict(id="native_contact_burn", difference="Contact exposure differs from original DM", observed=contacts))
    debug = actual["debug"]
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
    assert len(exchange) == 15, "Incomplete native exchange checks"
    for key, value in exchange.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed native exchange check"))
    assert len(debug) == 21, "Incomplete native admin panel checks"
    for key, value in debug.items():
        if value != 1:
            failures.append(dict(id=key, difference="Failed native admin panel check"))
    report = dict(policy="DM object feedback between native calculations; approved by user", byond=actual["byond"], startup_seed=actual["startup_seed"], track_max_share=args.track_max_share,
                  dll_sha256=hashlib.sha256((output / "bandastation_atmos.dll").read_bytes()).hexdigest(),
                  binary_sha256=hashlib.sha256((output / "tgstation.dmb").read_bytes()).hexdigest(),
                  harness_sha256=fixture_hashes["tools/atmos/phase_world.dm.in"],
                  fixture_sha256=fixture_hashes,
                  scenarios=len(pairs), network_scenarios=len(networks), network_ticks=sum(c["input"]["ticks"] for c in networks), checks=checks, inheritance=inheritance, bindings=binding_checks, deferred=deferred, api=api, contacts=contacts, debug=debug, exchange=exchange, exchange_error=exchange_error, exchange_effects=exchange_effects, exchange_spawn=exchange_spawn, failures=failures)
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    print(f"Native phase: {len(pairs)} world scenarios, {len(networks)} connected graphs, {len(checks)} queue/lifetime checks, {len(failures)} failures")
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


if __name__ == "__main__":
    main()
