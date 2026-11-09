"""Isolate Life's ambient-air API; each case uses a fresh server process and running MC/GC."""
import argparse
import json
from pathlib import Path
from statistics import median

import air_access_bench as access
import atmos_cost as cost
from atmos_cost_report import process_window, workload_errors


def prepare(folder, dll, source):
    access.prepare(folder, dll, source, breathers=50)
    target = folder / "current"
    life = target / "code/modules/mob/living/life.dm"
    cost.write(life, access.replace_once(life.read_text(encoding="utf-8-sig"),
        "if(SSair.native_atmos?.air_references_ready)",
        "if(SSair.native_atmos?.air_references_ready && !atmos_life_legacy)"))
    path = target / "air_access.dm.in"
    text = path.read_text(encoding="utf-8-sig")
    a = text.index("/datum/controller/subsystem/atmos_cost/proc/capture()")
    b = text.index("/datum/controller/subsystem/atmos_cost/proc/run_case(", a)
    capture = '''var/global/atmos_life_legacy = FALSE

/datum/controller/subsystem/atmos_cost/proc/capture()
\tset waitfor = FALSE
\tsleep(5 SECONDS)
\tCONFIG_SET(number/drift_dump_threshold, 1000000)
\tfor(var/datum/controller/subsystem/subsystem as anything in Master.subsystems)
\t\tsubsystem.profiler_focused = FALSE
\t\tif(subsystem == SSgarbage || subsystem == SStime_track)
\t\t\tcontinue
\t\tsubsystem.can_fire = FALSE
\tSStime_track.wait = 10
\tsleep(2 SECONDS)
\tvar/mode = world.params["life-mode"]
\tatmos_life_legacy = mode == "legacy"
\tvar/list/results = list(run_case(world.params["life-scenario"], mode, text2num(world.params["life-repeat"]), text2num(world.params["life-profile"])))
\tWRITE_FILE(file("atmos-cost-results.json"), json_encode(results))
\tlog_world("ATMOS_COST_COMPLETE")
\tshutdown()

'''
    text = text[:a] + capture + text[b:]
    text = access.replace_once(text, 'phase.air_references_ready = mode == "api"', 'phase.air_references_ready = TRUE')
    text = access.replace_once(text, "\t// Setup is excluded; the same RNG stream begins immediately before the fixed frame sequence.",
        "\t// New turfs/vents re-enable smoothing; let MC finish setup before measuring Life.\n\tvar/life_smoothing_start = world.time\n\twhile(SSicon_smooth.can_fire || SSicon_smooth.state != SS_IDLE)\n\t\tif(world.time - life_smoothing_start > 10 SECONDS)\n\t\t\tCRASH(\"Life benchmark smoothing did not drain\")\n\t\tsleep(world.tick_lag)\n\t// Setup is excluded; the same RNG stream begins immediately before the fixed frame sequence.")
    text = access.replace_once(text, "\tvar/start_unix = rustg_unix_timestamp()",
        "\tvar/list/life_background_start = list()\n\tfor(var/datum/controller/subsystem/subsystem as anything in Master.subsystems)\n\t\tlife_background_start[subsystem] = subsystem.times_fired\n\tSStime_track.first_run = TRUE\n\tSStime_track.next_fire = world.time + world.tick_lag\n\tvar/life_game_start = world.time\n\tvar/start_unix = rustg_unix_timestamp()")
    text = access.replace_once(text, "\tvar/end_unix = rustg_unix_timestamp()",
        "\tvar/end_unix = rustg_unix_timestamp()\n\tvar/life_game_seconds = (world.time - life_game_start) / 10\n\tvar/list/life_background_fires = list()\n\tfor(var/datum/controller/subsystem/subsystem as anything in Master.subsystems)\n\t\tif(subsystem == SSair || subsystem == SSmobs || subsystem == SSgarbage || subsystem == SStime_track)\n\t\t\tcontinue\n\t\tvar/fires = subsystem.times_fired - life_background_start[subsystem]\n\t\tif(fires)\n\t\t\tlife_background_fires[subsystem.type] = fires")
    text = access.replace_once(text, "\tvar/list/breathing_trace = list()",
        "\tvar/list/breathing_trace = list()\n\tvar/list/life_trace = list()\n\tvar/list/td_trace = list()\n\tvar/last_td_time = world.time")
    text = access.replace_once(text, "\t\tphase.invoke(CALLBACK(src, PROC_REF(breathe_people), people))",
        "\t\tSSmobs.times_fired = frame\n\t\tphase.invoke(CALLBACK(src, PROC_REF(breathe_people), people))\n\t\tlife_trace += list(life_counts(people))")
    text = access.replace_once(text, "\t\treading_trace += list(list(values_sum(held), values_sum(readonly)))",
        "\t\treading_trace += list(list(values_sum(held), values_sum(readonly)))\n\t\tsleep(world.tick_lag)\n\t\tif(SStime_track.last_tick_byond_time > last_td_time)\n\t\t\tlast_td_time = SStime_track.last_tick_byond_time\n\t\t\ttd_trace += list(list(last_td_time, SStime_track.time_dilation_current))")
    text = access.replace_once(text, '"issued" = length(held),',
        '"issued" = length(held), "life_trace" = life_trace, "td_trace" = td_trace, "game_seconds" = life_game_seconds, "background_fires" = life_background_fires, "mc_running" = !!Master.processing, "gc_running" = SSgarbage.can_fire, "time_track_running" = SStime_track.can_fire, "life_legacy" = atmos_life_legacy,')
    text = access.replace_once(text, "\t\tperson.breathe(2)", "\t\tperson.Life(2)")
    text += '''
/mob/living/carbon/human/air_access_breather
\tvar/life_calls = 0
\tvar/environment_calls = 0

/mob/living/carbon/human/air_access_breather/Life(seconds_per_tick = SSMOBS_DT)
\t. = ..()
\tlife_calls++

/mob/living/carbon/human/air_access_breather/handle_environment(datum/gas_mixture/environment, seconds_per_tick, times_fired, datum/banda_atmos_air_reference/air_reference)
\t. = ..()
\tenvironment_calls++

/datum/controller/subsystem/atmos_cost/proc/life_counts(list/people)
\tvar/calls = 0
\tvar/environment_calls = 0
\tvar/list/body = list()
\tfor(var/mob/living/carbon/human/air_access_breather/person as anything in people)
\t\tcalls += person.life_calls
\t\tenvironment_calls += person.environment_calls
\t\tbody += list(list(person.bodytemperature, person.coretemperature, person.get_brute_loss(), person.get_fire_loss(), person.get_oxy_loss(), person.stat, person.seconds_in_low_pressure))
\treturn list(calls, body, environment_calls)
'''
    cost.write(path, text)
    p = json.loads((folder / "provenance.json").read_text())
    p.update(life_only=True, mc_running=True, gc_running=True, td_interval_ds=10, one_case_per_process=True,
             source_hashes={name: cost.sha(target / name) for name in p["source_hashes"]},
             harness_sha256=cost.sha(path), scope="Life environment API only; breathing uses live references in both variants")
    cost.write(folder / "provenance.json", json.dumps(p, indent=2))


def cases(smoke):
    for profile in (0, 1) if not smoke else (0,):
        for repeat in range(1, 2 if smoke or profile else 4):
            scenarios = ("fire", "pipe") if repeat % 2 else ("pipe", "fire")
            modes = ("legacy", "api") if repeat % 2 else ("api", "legacy")
            for scenario in scenarios:
                for mode in modes:
                    yield scenario, mode, repeat, profile


def label(case, smoke):
    return "-".join(map(str, case)) + ("-smoke" if smoke else "-run")


def validate(rows, frames, side, life_variant=True):
    errors = workload_errors(rows, frames, side)
    import sys
    sys.path.insert(0, str(cost.ROOT / "tools/atmos"))
    from transport import difference
    for row in rows:
        if "background_fires" not in row or row["background_fires"]:
            errors.append(dict(case=row["case"], error="Uncontrolled background subsystem work"))
        if not all(row[k] for k in ("mc_running", "gc_running", "time_track_running", "references_enabled")):
            errors.append(dict(case=row["case"], error="MC/GC/time tracking or common breathing API disabled"))
        if (life_variant and bool(row["life_legacy"]) != (row["mode"] == "legacy")) or row["retained_references"]:
            errors.append(dict(case=row["case"], error="Wrong Life path or retained references"))
        if (row["breathers"] != min(50, side * side // 4)
                or len(row["reading_trace"]) != frames or len(row["breathing_trace"]) != frames
                or len(row["life_trace"]) != frames or row["life_trace"][-1][0] != frames * row["breathers"]
                or row["life_trace"][-1][2] != frames * row["breathers"]):
            errors.append(dict(case=row["case"], error="Incomplete Life work"))
        if life_variant and row["full_caches"] != (row["breathers"] if row["mode"] == "legacy" else 0):
            errors.append(dict(case=row["case"], error="Wrong persistent cache coverage"))
    for scenario in ("fire", "pipe"):
        group = [r for r in rows if r["scenario"] == scenario]
        for row in group[1:]:
            for key in ("life_trace", "breathing_trace", "reading_trace"):
                delta = difference(group[0][key], row[key], key)
                if delta:
                    errors.append(dict(case=row["case"], error=delta))
    return errors


def report(folder, smoke, validator=validate):
    rows = []
    p = json.loads((folder / "provenance.json").read_text())
    pids, binaries = set(), set()
    for case in cases(smoke):
        output = folder / label(case, smoke)
        control = json.loads((output / "exit.json").read_text())
        assert control["exit_code"] == 0 and control["dll_sha256"] == p["dll_sha256"]
        assert not json.loads((output / "runtime-validation.json").read_text())["unexpected_count"]
        pids.add(control["server_pid"])
        binaries.add(control["binary_sha256"])
        data = json.loads((output / "atmos-cost-results.json").read_text(encoding="utf-8-sig"))
        assert len(data) == 1
        row = data[0]
        assert (row["scenario"], row["mode"], row["repeat"], row["profiled"]) == case
        row["case"] = label(case, smoke)
        samples = json.loads((output / "process-samples.json").read_text())
        row["process"] = process_window(samples, float(row["start_unix"]), float(row["end_unix"]))
        profile = json.loads((output / row["profile"]).read_text(encoding="utf-8-sig"))
        if not row["profiled"]:
            assert not [v for v in profile if any(v.get(k, 0) for k in ("calls", "self", "total", "real", "over"))
                        and not (v["name"] == "/world/Profile" and v.get("calls", 0) <= 1)]
        row["top_profile_self"] = sorted(profile, key=lambda v: v.get("self", 0), reverse=True)[:30]
        rows.append(row)
    assert len(binaries) == 1
    errors = validator(rows, 4 if smoke else 40, 8 if smoke else 64)
    summary = []
    for scenario in ("fire", "pipe"):
        for mode in ("legacy", "api"):
            group = [r for r in rows if r["scenario"] == scenario and r["mode"] == mode and not r["profiled"]]
            def cpu(row, key):
                return row["process"][key] * 1000 / row["frames"]
            td = [max(0, (r["process"]["wall_seconds"] / r["game_seconds"] - 1) * 100) for r in group]
            peaks = [max((v[1] for v in r["td_trace"]), default=None) for r in group]
            summary.append(dict(scenario=scenario, mode=mode, frames=group[0]["frames"],
                cpu_ms=median(cpu(r, "cpu_seconds") for r in group), cpu_range=[min(cpu(r, "cpu_seconds") for r in group), max(cpu(r, "cpu_seconds") for r in group)],
                main_ms=median(cpu(r, "main_thread_cpu_seconds") for r in group),
                worker_ms=median((r["worker_after"][0] - r["worker_before"][0]) * 1000 / r["frames"] for r in group),
                wall_ms=median(r["process"]["wall_seconds"] * 1000 / r["frames"] for r in group),
                td_mean_percent=median(td), td_mean_range=[min(td), max(td)], td_peak_percent=max((v for v in peaks if v is not None), default=None),
                private_mean_mib=median(r["process"]["private_bytes"]["mean_mib"] for r in group), private_mean_range=[min(r["process"]["private_bytes"]["mean_mib"] for r in group),max(r["process"]["private_bytes"]["mean_mib"] for r in group)],
                private_peak_mib=max(r["process"]["private_bytes"]["peak_mib"] for r in group),
                caches=group[0]["full_caches"], scans=group[0]["scanned"], copies=group[0]["copies"]))
    result = dict(accepted=not errors, errors=errors, server_processes=len(rows), observed_pid_count=len(pids),
                  binary_sha256=next(iter(binaries)), summary=summary, rows=rows)
    cost.write(folder / ("smoke-report.json" if smoke else "report.json"), json.dumps(result, ensure_ascii=False, indent=2))
    print(json.dumps(dict(accepted=not errors, errors=errors, summary=summary), ensure_ascii=False, indent=2))
    if errors:
        raise SystemExit("Unequal work/state; do not interpret performance")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "compile", "capture", "report"))
    parser.add_argument("--name", required=True)
    parser.add_argument("--dll", type=Path)
    parser.add_argument("--source", type=Path, default=cost.ROOT / "tmp/atmos-live-bench-01/current")
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
        for case in cases(args.smoke):
            scenario, mode, repeat, profile = case
            output = folder / label(case, args.smoke)
            if (output / "exit.json").exists():
                continue
            cost.capture(folder, args.byond, "current", args.smoke, label(case, args.smoke),
                         f"life-mode={mode}&life-scenario={scenario}&life-repeat={repeat}&life-profile={profile}")
    else:
        report(folder, args.smoke)
