"""A/B the migrated scalar consumers with identical gas inputs and fixed work.

One frozen game binary and DLL; only the tested access capability changes.
With --breathers, scalar queries stay enabled in both modes and the comparison
isolates live references in breathing. Neither mode represents a full player load.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
from statistics import median
import subprocess

import atmos_cost as cost
from atmos_cost_report import process_window, workload_errors


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise ValueError(f"Fixture boundary changed: {before[:100]!r}")
    return text.replace(before, after)


def access_errors(rows, frames, side):
    errors = workload_errors(rows, frames, side)
    for row in rows:
        if len(row.get("reading_trace", [])) != frames:
            errors.append(dict(case=row["case"], error="Missing per-frame consumer observations"))
        if row.get("breathers", 0):
            trace = row.get("breathing_trace", [])
            if len(trace) != frames or trace[-1][0] != frames * row["breathers"] or row.get("retained_references") != 0:
                errors.append(dict(case=row["case"], error="Incomplete breathing or leaked reference"))
            if bool(row.get("references_enabled")) != (row["mode"] == "api"):
                errors.append(dict(case=row["case"], error="Wrong live-reference access path"))
        elif (row["mode"] == "legacy" and row.get("value_queries", 0)) or (row["mode"] == "api" and row.get("value_queries", 0) <= 0):
            errors.append(dict(case=row["case"], error="Wrong gas access path"))
    return errors


def prepare(folder, dll, source, breathers=0):
    if folder.exists():
        raise SystemExit("Preserve earlier results; choose another name")
    folder.mkdir(parents=True)
    target = folder / "current"
    shutil.copytree(source, target, copy_function=os.link,
                    ignore=shutil.ignore_patterns("*.dmb", "*.rsc", "tmp", ".git", "target"))
    files = subprocess.check_output(["rg", "--files", "-g", "*.dm", "-g", "*.dme"], cwd=cost.ROOT, text=True).splitlines()
    for name in files:
        cost.write(target / name, (cost.ROOT / name).read_bytes())
    hashes = {name.replace("\\", "/"): cost.sha(target / name) for name in files}
    for name, proc in (("access.dm", "copy_readonly"), ("cell.dm", "execute")):
        path = target / "modular_bandastation/atmos_rust/code" / name
        cost.write(path, replace_once(path.read_text(encoding="utf-8-sig"),
                   f"/datum/banda_atmos_cell/proc/{proc}(", f"/datum/banda_atmos_cell/proc/cost_original_{proc}("))
    path = target / "modular_bandastation/atmos_rust/code/world.dm"
    code = path.read_text(encoding="utf-8-sig")
    signature = "/datum/banda_atmos_safe/world_commands/world/on_run(list/commands, start = 1, budgeted = FALSE)\n"
    code = replace_once(code, signature, signature + "\tvar/cost_groups = 0\n")
    code = replace_once(code,
        "\t\tif(budgeted && (next > length(commands) || commands[next] != commands[index]) && (TICK_USAGE > Master.current_ticklimit || SSair.state != SS_RUNNING))\n\t\t\treturn next",
        "\t\tif(budgeted && (next > length(commands) || commands[next] != commands[index]))\n\t\t\tcost_groups++\n\t\t\tif(cost_groups >= atmos_cost_quota)\n\t\t\t\treturn next")
    cost.write(path, code)
    harness = (cost.ROOT / "tools/profiling/atmos_cost.dm.in").read_text(encoding="utf-8-sig")
    harness = replace_once(harness,
        '\t\t\t\tvar/list/modes = list("atomic")\n\t\t\t\t#ifdef ATMOS_COST_CHUNKS\n\t\t\t\tmodes = repeat % 2 ? list("atomic", "chunks") : list("chunks", "atomic")\n\t\t\t\t#endif',
        '\t\t\t\tvar/list/modes = repeat % 2 ? list("legacy", "api") : list("api", "legacy")')
    harness = replace_once(harness, 'text2num(world.params["cost-frames"]) || 80', 'text2num(world.params["cost-frames"]) || 40')
    harness = replace_once(harness, '\tphase.threaded = TRUE',
        '\tif(!phase.value_queries_ready)\n\t\tCRASH("Value API required by both benchmark variants")\n\tphase.value_queries_ready = mode == "api"\n\tphase.threaded = TRUE')
    harness = replace_once(harness, 'phase.run_tick(FALSE, mode == "chunks")', 'phase.run_tick(FALSE, TRUE)')
    harness = replace_once(harness,
        '\t\theld += target.return_air()\n\t\treadonly += target.get_readonly_air()',
        '\t\theld += target.GetTemperature()\n\t\treadonly += target.GetHeatCapacity()')
    harness = replace_once(harness, '\t\ttrace += list(list(phase.debug_state(), atmos_cost_commands))',
        '\t\theld.Cut()\n\t\treadonly.Cut()\n\t\tphase.invoke(CALLBACK(src, PROC_REF(issue_copies), phase, targets, held, readonly))\n\t\ttrace += list(list(phase.debug_state(), atmos_cost_commands))\n\t\treading_trace += list(list(values_sum(held), values_sum(readonly)))')
    harness = replace_once(harness, '\tvar/list/trace = list()', '\tvar/list/trace = list()\n\tvar/list/reading_trace = list()')
    harness = replace_once(harness, '\tvar/list/after = worker_state(phase)',
        '\tvar/list/after = worker_state(phase)\n\tvar/full_caches = 0\n\tfor(var/turf/open/tile as anything in targets)\n\t\tif(tile.banda_atmos_cell?.cached_air)\n\t\t\tfull_caches++')
    harness = replace_once(harness, '"issued" = length(held),',
        '"issued" = length(held), "full_caches" = full_caches, "value_queries" = phase.value_queries, "reading_trace" = reading_trace,')
    if breathers:
        harness = replace_once(harness, '\tphase.value_queries_ready = mode == "api"', '\tif(!phase.air_references_ready)\n\t\tCRASH("Live reference API required")\n\tphase.value_queries_ready = TRUE\n\tphase.air_references_ready = mode == "api"')
        signature = '\tvar/datum/banda_atmos_phase/world_adapter/cost_probe/phase = new("bandastation_atmos.dll")'
        harness = replace_once(harness, signature, f'\tvar/list/people = create_breathers(targets, {breathers})\n' + signature)
        harness = replace_once(harness, '\tvar/list/reading_trace = list()', '\tvar/list/reading_trace = list()\n\tvar/list/breathing_trace = list()')
        harness = replace_once(harness, '\t\t#ifdef ATMOS_COST_CHUNKS\n\t\tphase.run_tick', '\t\tphase.invoke(CALLBACK(src, PROC_REF(breathe_people), people))\n\t\tbreathing_trace += list(breathing_counts(people))\n\t\t#ifdef ATMOS_COST_CHUNKS\n\t\tphase.run_tick')
        harness = replace_once(harness, '"issued" = length(held),', '"issued" = length(held), "breathers" = length(people), "breathing_trace" = breathing_trace, "references_enabled" = phase.air_references_ready, "retained_references" = phase.air_reference_count(),')
        harness = replace_once(harness, '\tqdel(network)\n\tqdel(phase)', '\tQDEL_LIST(people)\n\tqdel(network)\n\tqdel(phase)')
        harness += '\n' + (cost.ROOT / "tools/profiling/air_access_breathing.dm.in").read_text(encoding="utf-8-sig")
    cost.write(target / "air-access.test.dme", '#define ATMOS_COST_CHUNKS\n' + (target / "tgstation.dme").read_text(encoding="utf-8-sig") + '\n#include "air_access.dm.in"\n')
    cost.write(target / "air_access.dm.in", harness)
    shutil.copy2(dll, folder / "bandastation_atmos.dll")
    cost.write(folder / "provenance.json", json.dumps(dict(build_roots=dict(current=str(target)), binary_stem="air-access.test",
        dll_sha256=cost.sha(dll), source_hashes=hashes, harness_sha256=cost.sha(target / "air_access.dm.in"),
        frames=40, side=64, quota=128, breathers=breathers, readers="1024 temperature and capacity reads before and after every frame; live breathing comparison if breathers > 0", map="runtimestation_minimal"), indent=2))


def report(folder, smoke):
    output = folder / ("current-smoke" if smoke else "current-run")
    control = json.loads((output / "exit.json").read_text())
    provenance = json.loads((folder / "provenance.json").read_text())
    assert control["exit_code"] == 0 and control["dll_sha256"] == provenance["dll_sha256"]
    assert not json.loads((output / "runtime-validation.json").read_text())["unexpected_count"]
    samples = json.loads((output / "process-samples.json").read_text())
    rows = json.loads((output / "atmos-cost-results.json").read_text(encoding="utf-8-sig"))
    frames, side = (4, 8) if smoke else (40, 64)
    for row in rows:
        row["case"] = f'{row["scenario"]}/{row["mode"]}/{row["repeat"]}/{row["profiled"]}'
        row["process"] = process_window(samples, float(row["start_unix"]), float(row["end_unix"]))
        profile = json.loads((output / row["profile"]).read_text(encoding="utf-8-sig"))
        if not row["profiled"]:
            unexpected = [p for p in profile if any(p.get(k, 0) for k in ("calls", "self", "total", "real", "over"))
                          and not (p["name"] == "/world/Profile" and p.get("calls", 0) <= 1)]
            assert not unexpected, "Proc profiling was enabled in a primary window"
        row["top_profile_self"] = sorted(profile, key=lambda p: p.get("self", 0), reverse=True)[:30]
    errors = access_errors(rows, frames, side)
    if len(rows) != (8 if smoke else 16):
        errors.append(dict(error="Incomplete cases"))
    from sys import path
    path.insert(0, str(cost.ROOT / "tools/atmos"))
    from transport import difference
    for scenario in ("fire", "pipe"):
        group = [r for r in rows if r["scenario"] == scenario]
        for row in group[1:]:
            delta = difference(group[0]["reading_trace"], row["reading_trace"], "readings")
            if delta:
                errors.append(dict(case=row["case"], error=delta))
            if provenance.get("breathers"):
                delta = difference(group[0]["breathing_trace"], row["breathing_trace"], "breathing")
                if delta:
                    errors.append(dict(case=row["case"], error=delta))
    summary = []
    for scenario in ("fire", "pipe"):
        for mode in ("legacy", "api"):
            group = [r for r in rows if r["scenario"] == scenario and r["mode"] == mode and not r["profiled"]]
            diagnostic = [r for r in rows if r["scenario"] == scenario and r["mode"] == mode and r["profiled"]]
            if len(group) != (1 if smoke else 3):
                errors.append(dict(error=f"Incomplete repetitions {scenario}/{mode}"))
                continue
            if len(diagnostic) != 1:
                errors.append(dict(error=f"Incomplete diagnostic {scenario}/{mode}"))
                continue
            summary.append(dict(scenario=scenario, mode=mode, frames=frames,
                cpu_ms=median(r["process"]["cpu_seconds"] * 1000 / frames for r in group),
                cpu_ms_range=[min(r["process"]["cpu_seconds"] * 1000 / frames for r in group), max(r["process"]["cpu_seconds"] * 1000 / frames for r in group)],
                main_ms=median(r["process"]["main_thread_cpu_seconds"] * 1000 / frames for r in group),
                worker_ms=median((r["worker_after"][0] - r["worker_before"][0]) * 1000 / frames for r in group),
                other_threads_ms=median((r["process"]["cpu_seconds"] - r["process"]["main_thread_cpu_seconds"]) * 1000 / frames for r in group),
                wall_ms=median(r["process"]["wall_seconds"] * 1000 / frames for r in group),
                private_mean_mib=median(r["process"]["private_bytes"]["mean_mib"] for r in group),
                private_peak_mib=max(r["process"]["private_bytes"]["peak_mib"] for r in group),
                working_set_mean_mib=median(r["process"]["working_set_bytes"]["mean_mib"] for r in group),
                full_caches=group[0]["full_caches"], scans=group[0]["scanned"], copies=group[0]["copies"], value_queries=group[0]["value_queries"], breathers=group[0].get("breathers",0),
                diagnostic_exclusive_ms={key: value / frames / 1000 for key, value in diagnostic[0]["exclusive_us"].items()}))
    result = dict(accepted=not errors, errors=errors, summary=summary, rows=rows, control=control)
    cost.write(folder / ("smoke-report.json" if smoke else "report.json"), json.dumps(result, ensure_ascii=False, indent=2))
    print(json.dumps(dict(accepted=not errors, errors=errors, summary=summary), ensure_ascii=False, indent=2))
    if errors:
        raise SystemExit("Do not interpret performance with unequal work")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "compile", "capture", "report"))
    parser.add_argument("--name", required=True)
    parser.add_argument("--dll", type=Path)
    parser.add_argument("--source", type=Path, default=cost.ROOT / "tmp/atmos-fixed-cost-02/current")
    parser.add_argument("--byond", type=Path, default=Path("E:/Games/BYOND/bin"))
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--breathers", type=int, default=0, help="Isolate live reference vs compatible mixture access with this many controlled breathers")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    if not 0 <= args.breathers <= 1024:
        parser.error("--breathers must be between 0 and 1024")
    folder = cost.ROOT / "tmp" / args.name
    if args.action == "prepare":
        if not args.dll:
            parser.error("--dll is required for prepare")
        prepare(folder, args.dll.resolve(), args.source.resolve(), args.breathers)
    elif args.action == "compile":
        cost.compile_build(folder, args.byond, "current")
    elif args.action == "capture":
        cost.DLL = folder / "bandastation_atmos.dll"
        cost.capture(folder, args.byond, "current", args.smoke)
    else:
        report(folder, args.smoke)
