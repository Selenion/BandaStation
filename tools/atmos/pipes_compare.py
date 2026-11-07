"""Summarize the diagnostic pre-pipe/native-pipe pairs using existing OS accounting."""
import hashlib
import json
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/profiling"))
from compare import metrics


def main():
    pairs = []
    for scenario, suffix in [("empty-station", "idle"), ("atmos-fire", "fire"), ("sm-plasma", "sm")]:
        rows = []
        for mode in ["control", "native"]:
            folder = ROOT / "tmp/profiling" / f"pipes-{mode}-{suffix}-01"
            metadata = json.loads((folder / "profile-metadata.json").read_text(encoding="utf-8-sig"))
            manifest = json.loads((folder / "manifest.json").read_text(encoding="utf-8-sig"))
            assert metadata["scenario"] == scenario and metadata["rust_threaded"]
            assert metadata["connected_clients"] == 0
            assert metadata["atmos"]["native_pipes_enabled"] == (mode == "native")
            assert manifest["runtime_validation"]["unexpected_count"] == 0
            data = metrics(folder, metadata)
            samples = json.loads((folder / "process-samples.json").read_text(encoding="utf-8-sig"))
            samples = [s for s in samples if float(metadata["sample_start_unix"]) <=
                       datetime.fromisoformat(s["utc"]).timestamp() <= float(metadata["sample_end_unix"])]
            data["private_last_mib"] = samples[-1]["private_bytes"] / 2**20
            procs = json.loads((folder / "profile-procs.json").read_text(encoding="utf-8-sig"))
            data["pipe_procs"] = [p for p in procs if p["name"] in (
                "/datum/controller/subsystem/air/proc/process_pipenets", "/datum/pipeline/proc/reconcile_air",
                "/datum/banda_atmos_phase/proc/reconcile_pipes")]
            data["native_pipe_groups_in_window"] = metadata["atmos"]["native_pipe_groups"] - metadata["atmos_start"]["native_pipe_groups"]
            data["native_pipe_imports_in_window"] = metadata["atmos"]["native_pipe_imports"] - metadata["atmos_start"]["native_pipe_imports"]
            rows.append(dict(mode=mode, folder=str(folder.relative_to(ROOT)), metrics=data,
                             binary_sha256=manifest["binary_sha256"], dll_sha256=manifest["dll_sha256"]["bandastation_atmos.dll"],
                             inputs={k:metadata[k] for k in ("scenario", "byond", "map", "startup_seed", "seeded_turfs")},
                             incident=metadata.get("incident"),
                             runtime=manifest["runtime_validation"]))
        assert rows[0]["binary_sha256"] == rows[1]["binary_sha256"], "Different DM builds"
        assert rows[0]["inputs"] == rows[1]["inputs"], "Different scenario inputs"
        if scenario == "sm-plasma":
            assert rows[0]["incident"] == rows[1]["incident"], "Different SM inputs or outcome"
            assert rows[0]["incident"]["sm_deleted"], "SM explosion did not complete"
        pairs.append(dict(scenario=scenario, runs=rows))
    results = dict(scope="Diagnostic old Rust DLL versus native pipes on the same DM build; one pair per scenario, not a speedup claim",
                   pairs=pairs, limitations=["Zero connected players", "Different background states and pipenet counts",
                   "Control idle compiler finished near the start of its sample; no competing test servers",
                   "DM compatibility mixtures still synchronize on every reconcile; reactions remain DM"])
    paths = ["rust/atmos/src/pipenet_pool.rs", "rust/atmos/src/pipenet.rs", "rust/atmos/src/byond_pipes.rs",
             "rust/atmos/src/byond_packets.rs", "rust/atmos/src/phase.rs", "rust/atmos/tests/pipenet.rs",
             "modular_bandastation/atmos_rust/code/pipes.dm", "modular_bandastation/atmos_rust/code/phase.dm",
             "code/controllers/subsystem/air.dm", "code/modules/atmospherics/machinery/datum_pipeline.dm",
             "tools/atmos/pipes_world.dm.in", "tools/atmos/pipes_world.py", "tools/profiling/local_profile.dm.in"]
    results["source_sha256"] = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths}
    target = ROOT / "tools/atmos/PIPE_STAGE2_RESULTS.json"
    target.write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for pair in pairs:
        print(pair["scenario"])
        for row in pair["runs"]:
            m = row["metrics"]
            print(row["mode"], "CPU", round(m["process"]["cpu_one_core_percent"],2),
                  "main", round(m["process"]["main_thread_cpu_one_core_percent"],2),
                  "memory last/max", round(m["private_last_mib"],1), round(m["process"]["private_mib"][1],1),
                  "completed", m["normalized"]["completed_ticks"])


if __name__ == "__main__":
    main()
