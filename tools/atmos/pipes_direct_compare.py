"""Capture and compare packet/direct pipe paths on the same DLL and DM build."""
import argparse
import hashlib
import json
import statistics
import subprocess
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/profiling"))
from compare import metrics

SCENARIOS = [("empty-station", "idle"), ("atmos-fire", "fire"), ("sm-plasma", "sm")]


def read(folder):
    manifest = json.loads((folder / "manifest.json").read_text(encoding="utf-8-sig"))
    metadata = json.loads((folder / "profile-metadata.json").read_text(encoding="utf-8-sig"))
    assert manifest["metadata"] == metadata and not manifest["runtime_validation"]["unexpected_count"], folder
    assert metadata["rust_enabled"] and metadata["rust_threaded"] and metadata["connected_clients"] == 0
    data = metrics(folder, metadata)
    samples = json.loads((folder / "process-samples.json").read_text(encoding="utf-8-sig"))
    samples = [s for s in samples if float(metadata["sample_start_unix"]) <=
               datetime.fromisoformat(s["utc"]).timestamp() <= float(metadata["sample_end_unix"])]
    data["private_last_mib"] = samples[-1]["private_bytes"] / 2**20
    procs = json.loads((folder / "profile-procs.json").read_text(encoding="utf-8-sig"))
    data["pipe_procs"] = [p for p in procs if p["name"] in (
        "/datum/controller/subsystem/air/proc/process_pipenets", "/datum/pipeline/proc/reconcile_air",
        "/datum/banda_atmos_phase/proc/reconcile_pipes")]
    for counter in ("groups", "imports", "packet_imports", "direct_groups"):
        key = "native_pipe_" + counter
        data[counter] = metadata["atmos"][key] - metadata["atmos_start"][key]
    return dict(folder=str(folder.relative_to(ROOT)), metrics=data, runtime=manifest["runtime_validation"]), manifest, metadata


def summarize(pairs):
    summary = []
    for scenario, _ in SCENARIOS:
        item = dict(scenario=scenario, modes={})
        for mode in ("packet", "direct"):
            rows = [row["metrics"] for pair in pairs if pair["scenario"] == scenario for row in pair["runs"] if row["mode"] == mode]
            values = {
                "total_cpu_percent": [r["process"]["cpu_one_core_percent"] for r in rows],
                "main_cpu_percent": [r["process"]["main_thread_cpu_one_core_percent"] for r in rows],
                "other_cpu_percent": [r["process"]["other_threads_cpu_one_core_percent"] for r in rows],
                "worker_cpu_percent": [r["worker"]["cpu_one_core_percent"] for r in rows],
                "cpu_ms_per_tick": [r["normalized"]["estimated_process_cpu_ms_per_completed_tick"] for r in rows],
                "completed_ticks": [r["normalized"]["completed_ticks"] for r in rows],
                "private_last_mib": [r["private_last_mib"] for r in rows],
                "private_sample_peak_mib": [r["process"]["private_mib"][1] for r in rows],
                "lifetime_peak_commit_mib": [r["process"]["observed_lifetime_peak_commit_mib"] for r in rows],
                "pipe_seconds": [next(p["total"] for p in r["pipe_procs"] if p["name"] == "/datum/controller/subsystem/air/proc/process_pipenets") for r in rows],
                "groups": [r["groups"] for r in rows],
                "packet_imports": [r["packet_imports"] for r in rows],
                "direct_groups": [r["direct_groups"] for r in rows],
            }
            item["modes"][mode] = {k:dict(median=statistics.median(v), min=min(v), max=max(v)) for k,v in values.items()}
        summary.append(item)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--capture", action="store_true")
    parser.add_argument("--byond", type=Path)
    parser.add_argument("--dll", type=Path)
    args = parser.parse_args()
    if not args.prefix.replace("-", "").isalnum() or args.repeats < 1:
        parser.error("Invalid prefix/repeats")
    if args.capture and (not args.byond or not args.dll):
        parser.error("--capture requires --byond and --dll")
    sources = ["rust/atmos/src/byond_pipe_direct.rs", "rust/atmos/src/byond_pipes.rs", "rust/atmos/src/pipenet_pool.rs",
               "modular_bandastation/atmos_rust/code/pipes.dm", "modular_bandastation/atmos_rust/code/phase.dm",
               "tools/profiling/run.py", "tools/profiling/local_profile.dm.in"]
    hashes = {p:hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sources}
    binary = None
    pairs = []
    for scenario, suffix in SCENARIOS:
        for repeat in range(1, args.repeats + 1):
            modes = ["packet", "direct"] if repeat % 2 else ["direct", "packet"]
            rows = {}
            for mode in modes:
                name = f"{args.prefix}-{mode}-{suffix}-{repeat:02d}"
                folder = ROOT / "tmp/profiling" / name
                if args.capture and not folder.exists():
                    command = [sys.executable, str(ROOT / "tools/profiling/run.py"), "--byond", str(args.byond),
                               "--name", name, "--scenario", scenario, "--rust-dll", str(args.dll), "--threaded"]
                    if mode == "packet": command.append("--pipe-packets")
                    if binary: command += ["--compiled-binary", str(binary)]
                    print("Starting", name, flush=True)
                    subprocess.run(command, cwd=ROOT, check=True)
                row, manifest, metadata = read(folder)
                if args.capture:
                    assert manifest["dll_sha256"]["bandastation_atmos.dll"] == hashlib.sha256(args.dll.read_bytes()).hexdigest()
                assert metadata["scenario"] == scenario
                assert metadata["atmos"]["native_pipe_direct_enabled"] == (mode == "direct")
                assert (row["metrics"]["direct_groups"] > 0) == (mode == "direct")
                if mode == "direct": assert row["metrics"]["direct_groups"] == row["metrics"]["groups"]
                row.update(mode=mode, binary_sha256=manifest["binary_sha256"],
                           dll_sha256=manifest["dll_sha256"]["bandastation_atmos.dll"],
                           inputs={k:metadata[k] for k in ("scenario", "byond", "map", "startup_seed", "seeded_turfs")},
                           incident=metadata.get("incident"))
                rows[mode] = row
                binary = folder / "tgstation.dmb"
            a, b = rows["packet"], rows["direct"]
            for key in ("binary_sha256", "dll_sha256", "inputs", "incident"):
                assert a[key] == b[key], f"Different {key}"
            if scenario == "sm-plasma": assert a["incident"]["sm_deleted"]
            pairs.append(dict(scenario=scenario, repeat=repeat, runs=[a,b]))
            print(scenario, repeat, "CPU packet/direct", [round(r["metrics"]["process"]["cpu_one_core_percent"],2) for r in (a,b)], flush=True)
    assert hashes == {p:hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sources}, "Source changed during captures"
    results = dict(scope="Same DLL and DM build; diagnostic pipe packet control versus direct BYOND C API boundary",
                   source_sha256=hashes, pairs=pairs, summary=summarize(pairs),
                   limitations=["No connected players", "Background game states and net counts can differ",
                                "Pipe reactions/devices still use DM; current gas is read through C API at each reconcile",
                                "CPU 100% equals one logical core; timings include profiler overhead"])
    target = ROOT / "tools/atmos/PIPE_STAGE3_RESULTS.json"
    target.write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for scenario, _ in SCENARIOS:
        group = [p for p in pairs if p["scenario"] == scenario]
        print(scenario, "median total CPU", [round(statistics.median(p["runs"][i]["metrics"]["process"]["cpu_one_core_percent"] for p in group),2) for i in (0,1)])


if __name__ == "__main__":
    main()
