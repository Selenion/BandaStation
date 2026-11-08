"""Validate equal controlled work before comparing CPU or exclusive timings."""
import argparse
from bisect import bisect_right
from datetime import datetime
import json
from pathlib import Path
from statistics import median


def gas_totals(state):
    """Opcode 35: six identity fields, length-prefixed air, then metadata."""
    total = {}
    i = 0
    while i < len(state):
        length = int(state[i+6])
        air = state[i+7:i+7+length]
        for n in range(int(air[3])):
            gas, amount = air[4+2*n:6+2*n]
            total[int(gas)] = total.get(int(gas),0) + amount
        i += 7 + length
        i += 1 + int(state[i])
    return total


def process_window(samples, start, end):
    times = [datetime.fromisoformat(s["utc"]).timestamp() for s in samples]
    if not (len(times) > 1 and times == sorted(set(times)) and times[0] <= start < end <= times[-1]):
        raise ValueError("Process samples do not bracket the complete case")

    def at(t, key):
        i = min(len(times) - 2, max(0, bisect_right(times, t) - 1))
        ratio = (t - times[i]) / (times[i + 1] - times[i])
        return samples[i][key] * (1 - ratio) + samples[i + 1][key] * ratio

    result = {key: at(end, key) - at(start, key) for key in ("cpu_seconds", "main_thread_cpu_seconds")}
    boundaries = [start] + [t for t in times if start < t < end] + [end]
    for key in ("private_bytes", "working_set_bytes"):
        values = [at(t, key) / 2**20 for t in boundaries]
        area = sum((values[i] + values[i + 1]) * (boundaries[i + 1] - boundaries[i]) / 2 for i in range(len(values) - 1))
        result[key] = dict(mean_mib=area / (end - start), peak_mib=max(values))
    result["wall_seconds"] = end - start
    result["max_sample_gap_seconds"] = max(b - a for a, b in zip(boundaries, boundaries[1:]))
    return result


def workload_errors(rows, frames, side):
    errors = []
    for row in rows:
        if row["frames"] != frames or row["tile_count"] != side * side or row["issued"] != (side * side + 3) // 4:
            errors.append(dict(case=row["case"], error="Wrong frame/tile/reference count"))
        if len(row["trace"]) != frames or row["worker_after"][1] - row["worker_before"][1] != frames:
            errors.append(dict(case=row["case"], error="Wrong trace or worker job count"))
        if row["scenario"] == "pipe" and gas_totals(row["state"]).get(3,0) <= 0:
            errors.append(dict(case=row["case"], error="No plasma reached the chamber"))
        if row["scenario"] == "pipe" and row.get("vent_transfers",0) <= 0:
            errors.append(dict(case=row["case"], error="Direct Rust vent path was not exercised"))
    for scenario in {r["scenario"] for r in rows}:
        group = [r for r in rows if r["scenario"] == scenario]
        first = group[0]
        for row in group[1:]:
            for key in ("initial_state_hash", "state_hash", "port_hash", "trace", "command_kinds", "commands", "vents"):
                if row.get(key) != first.get(key):
                    errors.append(dict(case=row["case"], reference=first["case"], error=f"Different workload/result: {key}"))
    return errors


def report(folder, smoke=False):
    rows = []
    provenance=json.loads((folder/"provenance.json").read_text())
    suffix = "-smoke" if smoke else "-run"
    for build in ("before", "current"):
        output = folder / (build + suffix)
        runtime = json.loads((output / "runtime-validation.json").read_text())
        assert not runtime["unexpected_count"]
        control=json.loads((output / "exit.json").read_text())
        assert control["exit_code"] == 0 and control["dll_sha256"] == provenance["dll_sha256"]
        samples = json.loads((output / "process-samples.json").read_text())
        for row in json.loads((output / "atmos-cost-results.json").read_text(encoding="utf-8-sig")):
            row["case"] = f'{build}/{row["scenario"]}/{row["mode"]}/{row["repeat"]}/{row["profiled"]}'
            row["variant"] = "checkpoint" if build == "before" else row["mode"]
            row["capture_control"] = control
            row["process"] = process_window(samples, float(row["start_unix"]), float(row["end_unix"]))
            row["worker_cpu_seconds"] = row["worker_after"][0] - row["worker_before"][0]
            profile = json.loads((output / row["profile"]).read_text(encoding="utf-8-sig"))
            if not row["profiled"]:
                unexpected = [p for p in profile if any(p.get(k,0) for k in ("calls","self","total","real","over")) and not (p["name"] == "/world/Profile" and p.get("calls",0) <= 1)]
                assert not unexpected, "Proc profiling was enabled in a primary window"
            row["top_profile_self"] = sorted(profile, key=lambda r:r.get("self", 0), reverse=True)[:30]
            row["stage_profile"] = [p for p in profile if any(n in p["name"] for n in ("project_changes", "copy_readonly", "refresh_effect_readonly", "finish_callback", "cost_original_execute", "process_effect_chunk", "finish_effect_packet", "flush_inputs"))]
            rows.append(row)
    errors = workload_errors(rows, 4 if smoke else 80, 8 if smoke else 64)
    expected = 12 if smoke else 24
    if len(rows) != expected:
        errors.append(dict(error=f"Expected {expected} completed cases, got {len(rows)}"))
    summary = []
    for scenario in ("fire", "pipe"):
        for variant in ("checkpoint", "atomic", "chunks"):
            group = [r for r in rows if r["scenario"] == scenario and r["variant"] == variant and not r["profiled"]]
            diagnostic = [r for r in rows if r["scenario"] == scenario and r["variant"] == variant and r["profiled"]]
            if not group or len(diagnostic) != 1:
                errors.append(dict(error=f"Incomplete group {scenario}/{variant}"))
                continue
            if {r["repeat"] for r in group} != ({1} if smoke else {1,2,3}):
                errors.append(dict(error=f"Incomplete repetitions {scenario}/{variant}"))
            frames = group[0]["frames"]
            def values(key): return [r["process"][key] * 1000 / frames for r in group]
            total, main = values("cpu_seconds"), values("main_thread_cpu_seconds")
            scope = diagnostic[0]
            summary.append(dict(scenario=scenario, variant=variant, cpu_ms=median(total), cpu_ms_range=[min(total), max(total)],
                main_ms=median(main), main_ms_range=[min(main),max(main)], other_threads_ms=median((r['process']['cpu_seconds']-r['process']['main_thread_cpu_seconds'])*1000/frames for r in group), worker_ms=median(r["worker_cpu_seconds"] * 1000 / frames for r in group),
                private_mean_mib=median(r["process"]["private_bytes"]["mean_mib"] for r in group),
                commands=group[0]["commands"], copies=group[0]["copies"], scans=group[0]["scanned"],
                refreshes=group[0]["refreshes"], publications=group[0]["publications"], commits=group[0]["commits"],
                diagnostic_exclusive_ms={k:v / scope["frames"] / 1000 for k,v in scope["exclusive_us"].items()}))
    destination = folder / ("smoke-report.json" if smoke else "report.json")
    destination.write_text(json.dumps(dict(accepted=not errors, errors=errors, summary=summary, rows=rows),ensure_ascii=False,indent=2)+'\n',encoding="utf-8")
    print(json.dumps(dict(accepted=not errors, errors=errors, summary=summary),ensure_ascii=False,indent=2))
    if errors:
        raise SystemExit("Workload differs; do not attribute timing differences to implementation")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    report(args.folder, args.smoke)
