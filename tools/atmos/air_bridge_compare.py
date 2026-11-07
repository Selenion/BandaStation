"""Compare full gas publication with compact readings/visuals on one DLL and DM build."""
import argparse
import hashlib
import json
import statistics
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from pipes_direct_compare import ROOT, SCENARIOS, read

PROCS = {
    "publication": "/datum/banda_atmos_phase/world_adapter/proc/project_changes",
    "refresh": "/datum/banda_atmos_cell/proc/refresh_cache",
    "borrow": "/datum/banda_atmos_phase/world_adapter/borrow_air",
    "sync": "/datum/banda_atmos_cell/proc/sync_air",
    "readonly": "/turf/open/get_readonly_air",
    "prefetch": "/datum/banda_atmos_phase/world_adapter/proc/prefetch_air_readings",
    "properties": "/datum/banda_atmos_phase/world_adapter/proc/air_reading_offset",
    "machinery": "/datum/controller/subsystem/air/proc/process_atmos_machinery",
    "vent": "/obj/machinery/atmospherics/components/unary/vent_pump/process_atmos",
    "scrubber": "/obj/machinery/atmospherics/components/unary/vent_scrubber/proc/scrub",
    "scrubber_exposure": "/obj/machinery/atmospherics/components/unary/vent_scrubber/proc/scrub_exposed_tile",
    "atoms": "/datum/controller/subsystem/air/proc/process_atoms",
    "heat_pipe": "/obj/machinery/atmospherics/pipe/heat_exchanging/process_atmos",
    "mole_overlays": "/proc/banda_atmos_update_packet_visuals",
    "state_overlays": "/proc/banda_atmos_update_visual_states",
    "update_visuals": "/turf/open/proc/update_visuals",
}
COUNTERS = ("property_batches", "property_hits", "property_fallbacks", "publication_values", "visual_publications")
VENT_COUNTERS = ("vent_transfers", "vent_empty_transfers", "vent_transfer_fallbacks")
SCRUBBER_COUNTERS = ("scrubber_transfers", "scrubber_empty_transfers", "scrubber_transfer_fallbacks")
SCRUBBER_TILE_COUNTERS = ("scrubber_tile_operations", "scrubber_tile_moves")


def summarize(pairs, counters=COUNTERS):
    result = []
    for scenario in dict.fromkeys(pair["scenario"] for pair in pairs):
        modes = {}
        for mode in ("full", "compact"):
            rows = [r["metrics"] for p in pairs if p["scenario"] == scenario for r in p["runs"] if r["mode"] == mode]
            values = {
                "total_cpu_percent": [r["process"]["cpu_one_core_percent"] for r in rows],
                "main_cpu_percent": [r["process"]["main_thread_cpu_one_core_percent"] for r in rows],
                "other_cpu_percent": [r["process"]["other_threads_cpu_one_core_percent"] for r in rows],
                "worker_cpu_percent": [r["worker"]["cpu_one_core_percent"] for r in rows],
                "cpu_ms_per_tick": [r["normalized"]["estimated_process_cpu_ms_per_completed_tick"] for r in rows],
                "completed_ticks": [r["normalized"]["completed_ticks"] for r in rows],
                "private_last_mib": [r["private_last_mib"] for r in rows],
                "private_start_mib": [r["private_start_mib"] for r in rows],
                "private_window_delta_mib": [r["private_last_mib"] - r["private_start_mib"] for r in rows],
                "private_sample_peak_mib": [r["process"]["private_mib"][1] for r in rows],
                "lifetime_peak_commit_mib": [r["process"]["observed_lifetime_peak_commit_mib"] for r in rows],
            }
            for key in PROCS:
                values[key + "_seconds"] = [r["bridge_procs"][key].get("total", 0) for r in rows]
                values[key + "_calls"] = [r["bridge_procs"][key].get("calls", 0) for r in rows]
            for key in counters:
                values[key] = [r[key] for r in rows]
            if "visual_refreshes" in counters:
                values["scrubber_direct_share_percent"] = [r["scrubber_direct_share_percent"] for r in rows]
            modes[mode] = {k: dict(median=statistics.median(v), min=min(v), max=max(v)) for k, v in values.items()}
        result.append(dict(scenario=scenario, modes=modes))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--capture", action="store_true")
    device = parser.add_mutually_exclusive_group()
    device.add_argument("--vent-transfer", action="store_true", help="Compare vent transfers with both compact bridge features enabled")
    device.add_argument("--scrubber-transfer", action="store_true", help="Compare scrubber transfers with direct vents and the compact bridge enabled")
    device.add_argument("--scrubber-tile", action="store_true", help="Compare direct scrubber transfer with and without the DM tile mixture")
    device.add_argument("--visual-refresh", action="store_true", help="Compare ordinary icon refresh during a fire with and without the DM tile mixture")
    parser.add_argument("--byond", type=Path)
    parser.add_argument("--dll", type=Path)
    args = parser.parse_args()
    if not args.prefix.replace("-", "").isalnum() or args.repeats < 1:
        parser.error("Invalid prefix/repeats")
    if args.capture and (not args.byond or not args.dll):
        parser.error("--capture requires --byond and --dll")
    sources = sorted({p for pattern in ("rust/atmos/src/**/*.rs", "modular_bandastation/atmos_rust/**/*.dm") for p in ROOT.glob(pattern)} | {
        ROOT / p for p in ("tools/profiling/run.py", "tools/profiling/local_profile.dm.in", "code/controllers/subsystem/air.dm",
                          "code/__DEFINES/atmospherics/atmos_helpers.dm", "code/datums/elements/atmos_sensitive.dm",
                          "modular_bandastation/atmos_rust/_atmos_rust.dme",
                          "code/game/turfs/open/_open.dm", "code/modules/atmospherics/environmental/LINDA_turf_tile.dm",
                          "code/modules/atmospherics/machinery/components/unary_devices/vent_pump.dm",
                          "code/modules/atmospherics/machinery/components/unary_devices/vent_scrubber.dm")})
    def hashes():
        return {p.relative_to(ROOT).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}
    before = hashes()
    scrubber = args.scrubber_transfer or args.scrubber_tile
    device_name = "vent" if args.vent_transfer else "scrubber" if scrubber else None
    counters = COUNTERS + (VENT_COUNTERS if args.vent_transfer else SCRUBBER_COUNTERS if scrubber else ())
    if args.scrubber_tile:
        counters += SCRUBBER_TILE_COUNTERS
    if args.visual_refresh:
        counters += SCRUBBER_COUNTERS + SCRUBBER_TILE_COUNTERS + ("visual_refreshes",)
    binary = None
    pairs = []
    for scenario, suffix in (("atmos-fire", "fire"),) if args.visual_refresh else SCENARIOS:
        for repeat in range(1, args.repeats + 1):
            rows = {}
            for mode in (["full", "compact"] if repeat % 2 else ["compact", "full"]):
                name = f"{args.prefix}-{mode}-{suffix}-{repeat:02d}"
                folder = ROOT / "tmp/profiling" / name
                if args.capture and not folder.exists():
                    command = [sys.executable, str(ROOT / "tools/profiling/run.py"), "--byond", str(args.byond),
                               "--name", name, "--scenario", scenario, "--rust-dll", str(args.dll), "--threaded"]
                    if args.visual_refresh:
                        if mode == "full":
                            command.append("--legacy-visual-refresh")
                    else:
                        command.append("--legacy-visual-refresh")
                        if not device_name and mode == "full":
                            command.append("--legacy-air-bridge")
                        if not scrubber and (not args.vent_transfer or mode == "full"):
                            command.append("--legacy-vent-transfer")
                        if not scrubber or (args.scrubber_transfer and mode == "full"):
                            command.append("--legacy-scrubber-transfer")
                        elif not args.scrubber_tile or mode == "full":
                            command.append("--legacy-scrubber-tile")
                    if binary:
                        command += ["--compiled-binary", str(binary)]
                    print("Starting", name, flush=True)
                    subprocess.run(command, cwd=ROOT, check=True)
                row, manifest, metadata = read(folder)
                if args.capture:
                    assert manifest["dll_sha256"]["bandastation_atmos.dll"] == hashlib.sha256(args.dll.read_bytes()).hexdigest()
                assert metadata["scenario"] == scenario
                assert metadata["atmos"]["native_pipe_direct_enabled"]
                compact_bridge = args.visual_refresh or bool(device_name) or mode == "compact"
                for key in ("native_properties_enabled", "native_visual_publication_enabled"):
                    assert metadata["atmos"][key] == compact_bridge, (folder, key)
                if device_name:
                    key = f"native_{device_name}_transfer_enabled"
                    enabled = args.scrubber_tile or mode == "compact"
                    assert metadata["atmos_start"][key] == enabled, folder
                    assert metadata["atmos"][key] == enabled, folder
                if scrubber:
                    assert metadata["atmos_start"]["native_vent_transfer_enabled"], folder
                if args.scrubber_tile:
                    for section in ("atmos_start", "atmos"):
                        assert metadata[section]["native_scrubber_tile_enabled"] == (mode == "compact"), folder
                if args.visual_refresh:
                    for section in ("atmos_start", "atmos"):
                        for key in ("native_vent_transfer_enabled", "native_scrubber_transfer_enabled", "native_scrubber_tile_enabled"):
                            assert metadata[section][key], (folder, key)
                        assert metadata[section]["native_visual_refresh_enabled"] == (mode == "compact"), folder
                samples = json.loads((folder / "process-samples.json").read_text(encoding="utf-8-sig"))
                before_window = [s for s in samples if datetime.fromisoformat(s["utc"]).timestamp() < float(metadata["sample_start_unix"])]
                row["metrics"]["private_start_mib"] = before_window[-1]["private_bytes"] / 2**20
                procs = json.loads((folder / "profile-procs.json").read_text(encoding="utf-8-sig"))
                row["metrics"]["bridge_procs"] = {key: next((p for p in procs if p["name"] == path), {}) for key, path in PROCS.items()}
                for key in counters:
                    row["metrics"][key] = metadata["atmos"]["native_" + key] - metadata["atmos_start"]["native_" + key]
                if args.visual_refresh:
                    metrics = row["metrics"]
                    assert metrics["scrubber_transfers"] > 0 and (metrics["visual_refreshes"] > 0) == (mode == "compact"), folder
                    metrics["scrubber_direct_share_percent"] = 100 * metrics["scrubber_tile_moves"] / metrics["scrubber_transfers"]
                assert (row["metrics"]["property_hits"] > 0) == compact_bridge, folder
                assert (row["metrics"]["visual_publications"] > 0) == compact_bridge, folder
                row.update(mode=mode, binary_sha256=manifest["binary_sha256"],
                           dll_sha256=manifest["dll_sha256"]["bandastation_atmos.dll"],
                           inputs={k: metadata[k] for k in ("scenario", "byond", "map", "startup_seed", "seeded_turfs")},
                           incident=metadata.get("incident"))
                rows[mode] = row
                binary = folder / "tgstation.dmb"
            a, b = rows["full"], rows["compact"]
            for key in ("binary_sha256", "dll_sha256", "inputs", "incident"):
                assert a[key] == b[key], f"Different {key}"
            if scenario == "sm-plasma":
                assert a["incident"]["sm_deleted"]
            if args.scrubber_tile:
                assert a["metrics"]["scrubber_tile_operations"] == 0
            pairs.append(dict(scenario=scenario, repeat=repeat, runs=[a, b]))
            print(scenario, repeat, "CPU control/candidate", [round(r["metrics"]["process"]["cpu_one_core_percent"], 2) for r in (a, b)], flush=True)
    assert before == hashes(), "Source changed during captures"
    if args.visual_refresh:
        scope = "Same DLL and DM build with direct pipe, vent and scrubber transfers. Ordinary icon refresh with a DM tile cache versus compact visual descriptors"
    elif args.scrubber_tile:
        scope = "Same DLL and DM build with Rust scrubber transfer in both modes. Borrowed DM tile air versus direct native tile air"
    elif device_name:
        scope = f"Same DLL and DM build with compact bridge and direct pipes. DM {device_name} transfer versus synchronous Rust transfer"
    else:
        scope = "Same DLL and DM build, direct pipes in both modes; full gas bridge versus compact scalar/visual publication"
    result = dict(scope=scope,
                  source_sha256=before, pairs=pairs, summary=summarize(pairs, counters),
                  limitations=["No connected players", "Background states can diverge despite fixed startup seed",
                               "Inclusive procedure times overlap; worker CPU is already included in process CPU",
                               "Pipe reactions remain in DM. Profiler adds overhead"])
    if args.visual_refresh:
        report_name = "VISUAL_REFRESH_RESULTS"
    elif args.scrubber_tile:
        report_name = "SCRUBBER_TILE_RESULTS"
    elif device_name:
        report_name = f"{device_name.upper()}_TRANSFER_RESULTS"
    else:
        report_name = "AIR_BRIDGE_RESULTS"
    target = ROOT / "tools/atmos" / (report_name + ".json")
    target.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("Saved", target, flush=True)


if __name__ == "__main__":
    main()
