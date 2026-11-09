"""A/B real alarms and analyzers; common Rust physics/Life and one frozen binary/DLL."""
import argparse
import copy
import json
import math
from pathlib import Path
import re

import air_access_bench as access
import atmos_cost as cost
import life_access_bench as life


def measurement_roundoff(before, after):
    """Same UI text/units, with the existing numeric tolerance plus centi-unit formatting."""
    if not isinstance(before, str) or not isinstance(after, str):
        return False
    pattern = r"([-+]?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)"
    left, right = re.split(pattern, before), re.split(pattern, after)
    return (len(left) == len(right) and len(left) > 1 and left[::2] == right[::2]
            and all(math.isclose(float(a), float(b), rel_tol=2e-5, abs_tol=0.010000001)
                    for a, b in zip(left[1::2], right[1::2])))


def compare_observers(expected, actual, count):
    from transport import difference
    normalized = copy.deepcopy(actual)
    accepted = []
    for frame, (before, after) in enumerate(zip(expected, normalized)):
        for reader, (a, b) in enumerate(zip(before[:count], after[:count])):
            if not (isinstance(a, list) and isinstance(b, list) and len(a) == len(b) == 4):
                continue
            names = {"Pressure", "Temperature"}
            for mix in a[3]:
                names.update(g[1] for g in mix["gases"])
            for entry, (left, right) in enumerate(zip(a[2], b[2])):
                if (left.get("name") not in names or left.get("name") != right.get("name")
                        or left.get("value") == right.get("value")):
                    continue
                if measurement_roundoff(left.get("value"), right.get("value")):
                    accepted.append(dict(frame=frame, observer=reader, entry=entry,
                                         before=left["value"], after=right["value"]))
                    right["value"] = left["value"]
    # Raw analyzer scalars, all gas amounts, danger flags and all other text are still checked.
    return difference(expected, normalized, "observer_trace", numeric_strings=False), accepted


def prepare(folder, dll, source):
    life.prepare(folder, dll, source)
    target = folder / "current"
    path = target / "air_access.dm.in"
    text = path.read_text(encoding="utf-8-sig")
    text = access.replace_once(text, 'atmos_life_legacy = mode == "legacy"', "atmos_life_legacy = FALSE")
    text = access.replace_once(text, '\tvar/list/people = create_breathers(targets, 50)',
        '\tvar/list/people = create_breathers(targets, 50)\n\tvar/list/readers = create_readers(targets, mode)\n\tfor(var/index in 1 to length(pumps))\n\t\tvar/obj/machinery/atmospherics/components/unary/vent_pump/pump = pumps[index]\n\t\tpump.name = "Benchmark vent [index]"\n\tvar/obj/item/analyzer/reader = new(targets[1])\n\tfor(var/turf/open/target as anything in targets)\n\t\ttarget.banda_air_requested = FALSE')
    text = access.replace_once(text, '\tvar/life_game_start = world.time',
        '\tvar/life_game_start = world.time\n\tvar/reader_gc_before = SSgarbage.times_fired')
    text = access.replace_once(text, '\tvar/life_game_seconds = (world.time - life_game_start) / 10',
        '\tvar/life_game_seconds = (world.time - life_game_start) / 10\n\tvar/reader_gc_fires = SSgarbage.times_fired - reader_gc_before')
    text = access.replace_once(text, '\tvar/list/life_trace = list()',
        '\tvar/list/life_trace = list()\n\tvar/list/observer_trace = list()\n\tvar/list/port_owner_trace = list()')
    text = access.replace_once(text, '\t\tlife_trace += list(life_counts(people))',
        '\t\tlife_trace += list(life_counts(people))\n\t\tobserver_trace += list(phase.invoke(CALLBACK(src, PROC_REF(poll_readers), readers, pumps, reader, people[1], mode)))\n\t\tport_owner_trace += scanned_port_owners(pumps)')
    text = access.replace_once(text, '"life_trace" = life_trace,',
        '"life_trace" = life_trace, "observer_trace" = observer_trace, "port_owner_trace" = port_owner_trace, "observer_count" = length(readers), "scanned_ports" = min(16, length(pumps)), "gc_fires" = reader_gc_fires,')
    text = access.replace_once(text, '\tQDEL_LIST(people)', '\tqdel(reader)\n\tQDEL_LIST(readers)\n\tQDEL_LIST(people)')
    frozen = target / "readings_original.dm.in"
    cost.write(frozen, (cost.ROOT / "tools/atmos/original_readings_consumers.dm.in").read_bytes())
    text += '\n#include "readings_original.dm.in"\n' + (cost.ROOT / "tools/profiling/readings_observers.dm.in").read_text(encoding="utf-8-sig")
    cost.write(path, text)
    p = json.loads((folder / "provenance.json").read_text(encoding="utf-8"))
    p.update(scope="Alarms and analyzers only; production Rust Life enabled in both modes",
             life_only=False, readings_only=True,
             legacy_revision="e4b067b8c39", original_readers_sha256=cost.sha(frozen),
             observers_per_primary_case=64, tile_scans_per_frame=64, port_scans_per_frame=16,
             fixture_vent_names="Benchmark vent [index]",
             source_hashes={name: cost.sha(target / name) for name in p["source_hashes"]},
             harness_sha256=cost.sha(path))
    cost.write(folder / "provenance.json", json.dumps(p, indent=2))


def validate(rows, frames, side):
    errors = life.validate(rows, frames, side, life_variant=False)
    from transport import difference
    count = (side * side + 63) // 64
    for row in rows:
        row["display_roundoff"] = []
        if row["life_legacy"] or row.get("observer_count") != count:
            errors.append(dict(case=row["case"], error="Wrong common Life or observer count"))
        if row["full_caches"] != (count if row["mode"] == "legacy" else 0):
            errors.append(dict(case=row["case"], error="Wrong observer cache coverage"))
        ports = min(16, row["vents"])
        expected = ports if row["mode"] == "api" else 0
        if row.get("scanned_ports") != ports or row.get("port_owner_trace") != [expected] * frames:
            errors.append(dict(case=row["case"], error="Reading disabled native port ownership"))
        trace = row.get("observer_trace", [])
        if len(trace) != frames or any(len(v) != count + ports for v in trace):
            errors.append(dict(case=row["case"], error="Incomplete observer workload"))
    for scenario in ("fire", "pipe"):
        group = [r for r in rows if r["scenario"] == scenario]
        for row in group[1:]:
            delta, roundoff = compare_observers(group[0]["observer_trace"], row["observer_trace"], count)
            row["display_roundoff"] = roundoff
            if delta:
                errors.append(dict(case=row["case"], error=delta))
    return errors


def validate_completed(folder, smoke):
    """Check each completed prefix before starting another expensive server process."""
    rows = []
    for case in life.cases(smoke):
        output = folder / life.label(case, smoke)
        result = output / "atmos-cost-results.json"
        if not result.exists():
            continue
        data = json.loads(result.read_text(encoding="utf-8-sig"))
        if len(data) != 1:
            raise SystemExit("Incomplete observer result")
        row = data[0]
        row["case"] = output.name
        rows.append(row)
    errors = validate(rows, 4 if smoke else 40, 8 if smoke else 64)
    if errors:
        cost.write(folder / "prefix-validation-errors.json", json.dumps(errors, ensure_ascii=False, indent=2))
        raise SystemExit("Observer/state validation failed; do not interpret performance")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "compile", "capture", "report"))
    parser.add_argument("--name", required=True)
    parser.add_argument("--dll", type=Path)
    parser.add_argument("--source", type=Path, default=cost.ROOT / "tmp/atmos-life-bench-05/current")
    parser.add_argument("--byond", type=Path, default=Path("E:/Games/BYOND/bin"))
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    folder = cost.ROOT / "tmp" / args.name
    if args.action == "prepare":
        if not args.dll:
            parser.error("--dll is required")
        prepare(folder, args.dll.resolve(), args.source.resolve())
    elif args.action == "compile":
        cost.compile_build(folder, args.byond, "current")
    elif args.action == "capture":
        cost.DLL = folder / "bandastation_atmos.dll"
        validate_completed(folder, args.smoke)
        for case in life.cases(args.smoke):
            scenario, mode, repeat, profile = case
            output = folder / life.label(case, args.smoke)
            if (output / "exit.json").exists():
                continue
            cost.capture(folder, args.byond, "current", args.smoke, life.label(case, args.smoke),
                         f"life-mode={mode}&life-scenario={scenario}&life-repeat={repeat}&life-profile={profile}")
            validate_completed(folder, args.smoke)
    else:
        life.report(folder, args.smoke, validator=validate)
