"""Boot a full BandaStation map with the opt-in native SSair adapter."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import catalog
from native_probe import probe

ROOT = catalog.ROOT
sys.path.insert(0, str(ROOT / "tools/profiling"))
from run import stage, windows_process_sample


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--dll", type=Path, default=ROOT / "rust/atmos/target/i686-pc-windows-msvc/debug/bandastation_atmos.dll")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    output = ROOT / "tmp/atmos-world-adapter" / args.name
    if output.exists():
        parser.error("Use a new name to preserve earlier results")
    subprocess.run([sys.executable, str(ROOT / "tools/atmos/catalog.py"), "--check"], cwd=ROOT, check=True)
    output.mkdir(parents=True)
    stage(output)
    shutil.copy2(args.dll, output / "bandastation_atmos.dll")
    probe(args.byond, output)
    shutil.copy2(ROOT / "_maps/map_jsons/metastation.json", output / "data/next_map.json")
    with (output / "config/config.txt").open("a", encoding="utf-8") as config:
        config.write("\nFORBID_STATION_TRAITS\nRUST_ATMOS_LIBRARY bandastation_atmos.dll\n")
    fixture_hashes = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
                      for p in ("tools/atmos/world_adapter.dm.in", "tools/atmos/world_geometry.dm.in")}
    dme = ROOT / "local-atmos-world-adapter.test.dme"
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig") + '\n#include "tools/atmos/world_adapter.dm.in"\n#include "tools/atmos/world_geometry.dm.in"\n', encoding="utf-8", newline="\n")
    print("Compiling full-world native integration", flush=True)
    subprocess.run([str(args.byond / ("dm.exe" if os.name == "nt" else "DreamMaker")), "-DCBT", str(dme)], cwd=ROOT, check=True, timeout=300)
    for extension in ("dmb", "rsc"):
        shutil.copy2(dme.with_suffix("." + extension), output / ("tgstation." + extension))
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")), "tgstation.dmb", "-trusted", "-close", "-invisible",
                                    "-ip", "127.0.0.1", "-logself", "-params", "log-directory=local-atmos-world-adapter"],
                                   cwd=output, stdout=log, stderr=subprocess.STDOUT)
        samples = []
        started = time.monotonic()
        try:
            while process.poll() is None:
                if time.monotonic() - started > 600:
                    process.kill()
                    process.wait()
                    raise TimeoutError("World adapter run exceeded 600 seconds")
                if os.name == "nt":
                    sample = windows_process_sample(process.pid)
                    if sample:
                        samples.append(sample)
                try:
                    process.wait(timeout=0.5)
                except subprocess.TimeoutExpired:
                    pass
        finally:
            (output / "process-samples.json").write_text(json.dumps(samples), encoding="utf-8")
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, process.args)
    logs = "\n".join(p.read_text(encoding="utf-8-sig", errors="replace") for p in output.rglob("*") if p.is_file() and (p.suffix == ".log" or p.name.endswith(".log.json")))
    if any(marker in logs.lower() for marker in ("runtime error:", "runtime in ", "infinite loop")):
        raise SystemExit(f"Runtime in world adapter; inspect {output}")
    actual = json.loads((output / "world-adapter-output.json").read_text(encoding="utf-8-sig"))
    required = {"whole_map_registered", "world_has_no_dm_air_mirror", "source_gas_consumers_survive_tick", "source_cycle_matches_native", "floor_keeps_cell_and_gas",
                "native_freeze_reaches_world", "direct_hotspot_adopted", "native_fire_advances", "world_deletion_clears_native_fire",
                "effect_replacement_keeps_coordinate", "post_replacement_feedback_survives",
                "entering_hot_native_fire_keeps_contact_burn", "entering_cold_native_fire_skips_contact_burn",
                "closed_door_blocks_native_exchange", "opening_door_updates_native_connections", "closing_door_removes_native_connections"}
    assert set(actual["checks"]) == required, "Incomplete world adapter integration"
    failures = [key for key, value in actual["checks"].items() if value != 1]
    report = dict(actual, failures=failures,
                  memory_scope="Sampled process memory during startup and functional checks; not a DM/Rust performance comparison",
                  peak_sampled_working_set_bytes=max((v["working_set_bytes"] for v in samples), default=None),
                  peak_sampled_private_bytes=max((v["private_bytes"] for v in samples), default=None),
                  fixture_sha256=fixture_hashes,
                  dll_sha256=hashlib.sha256((output / "bandastation_atmos.dll").read_bytes()).hexdigest(),
                  binary_sha256=hashlib.sha256((output / "tgstation.dmb").read_bytes()).hexdigest())
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    print(f"World adapter: {actual['registered']} cells, {len(required)} checks, {len(failures)} failures")
    for failure in failures:
        print(f"  {failure}")
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
