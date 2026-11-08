"""Fixed finite-domain comparison of checkpoint, current atomic effects and portions.

Does not edit production DM. Quotas replace time budgets only in frozen diagnostic
sources, so repetitions execute the same number of portion boundaries. These are
cost measurements, not whole-station TD or throughput measurements.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from run import ROOT, stage, windows_process_sample
from runtime_check import validate_runtime_logs

CHECKPOINT = "c65fbacb60bddd493e079d4aa03456aae52dee64"
PREVIOUS = ROOT / "tmp/atmos-simplification-ab-01"
DLL = ROOT / "tmp/atmos-operations-01/target/i686-pc-windows-msvc/release/bandastation_atmos.dll"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)  # Frozen trees share read-only files through hard links.
    path.write_bytes(value if isinstance(value, bytes) else value.encode("utf-8"))


def prepare(folder, reuse=None):
    if folder.exists():
        raise SystemExit("Preserve previous results; use another --name")
    folder.mkdir(parents=True)
    if reuse:
        # Reuse unchanged DM/assets; preserve old sources and binaries, add a new DME only.
        previous = json.loads((reuse / "provenance.json").read_text())
        roots = {m:str((reuse/m).resolve()) for m in ("before","current")}
        stem = f"{folder.name}.test"
        harness = folder / "atmos_cost.dm.in"
        write(harness, (ROOT/"tools/profiling/atmos_cost.dm.in").read_bytes())
        for mode,path in roots.items():
            target=Path(path)
            source=(target/"tgstation.dme").read_text(encoding="utf-8-sig")
            write(target/f"{stem}.dme", ("#define ATMOS_COST_CHUNKS\n" if mode=="current" else "") + source + f'\n#include "{harness.as_posix()}"\n')
        previous.update(build_roots=roots, binary_stem=stem, reused_provenance=str(reuse/"provenance.json"), harness_sha256=sha(harness))
        write(folder/"provenance.json",json.dumps(previous,indent=2))
        return
    source_hashes = {}
    for mode in ("before", "current"):
        target = folder / mode
        shutil.copytree(PREVIOUS / ("before" if mode == "before" else "after"), target, copy_function=os.link,
                        ignore=shutil.ignore_patterns("*.dmb", "*.rsc", "target", "tmp", ".git"))
        for name in ("code/controllers/master.dm", "code/controllers/subsystem/air.dm", "modular_bandastation/atmos_rust/code/world.dm"):
            data = subprocess.check_output(["git", "show", f"{CHECKPOINT}:{name}"], cwd=ROOT) if mode == "before" else (ROOT / name).read_bytes()
            write(target / name, data)
        if mode == "current":
            write(target / "modular_bandastation/atmos_rust/code/effects.dm", (ROOT / "modular_bandastation/atmos_rust/code/effects.dm").read_bytes())
            path = target / "modular_bandastation/atmos_rust/code/world.dm"
            code = path.read_text(encoding="utf-8-sig")
            signature = "/datum/banda_atmos_safe/world_commands/world/on_run(list/commands, start = 1, budgeted = FALSE)\n"
            assert code.count(signature) == 1
            code = code.replace(signature, signature + "\tvar/cost_groups = 0\n")
            condition = "\t\tif(budgeted && (next > length(commands) || commands[next] != commands[index]) && (TICK_USAGE > Master.current_ticklimit || SSair.state != SS_RUNNING))\n\t\t\treturn next"
            assert code.count(condition) == 1
            code = code.replace(condition, "\t\tif(budgeted && (next > length(commands) || commands[next] != commands[index]))\n\t\t\tcost_groups++\n\t\t\tif(cost_groups >= atmos_cost_quota)\n\t\t\t\treturn next")
            write(path, code)
        for name, definition in (("access.dm", "copy_readonly"), ("cell.dm", "execute")):
            path = target / "modular_bandastation/atmos_rust/code" / name
            code = path.read_text(encoding="utf-8-sig")
            signature = f"/datum/banda_atmos_cell/proc/{definition}("
            assert code.count(signature) == 1
            write(path, code.replace(signature, f"/datum/banda_atmos_cell/proc/cost_original_{definition}("))
        write(target / "tools/profiling/atmos_cost.dm.in", (ROOT / "tools/profiling/atmos_cost.dm.in").read_bytes())
        source = (target / "tgstation.dme").read_text(encoding="utf-8-sig")
        write(target / "atmos-cost.test.dme", ("#define ATMOS_COST_CHUNKS\n" if mode == "current" else "") + source + '\n#include "tools/profiling/atmos_cost.dm.in"\n')
        source_hashes[mode] = {str(p.relative_to(target)).replace("\\", "/"): sha(p) for p in target.rglob("*") if p.suffix in (".dm", ".dme", ".in")}
    write(folder / "provenance.json", json.dumps(dict(checkpoint=CHECKPOINT, dll_sha256=sha(DLL), source_hashes=source_hashes,
          quota=128, frames=80, side=64, issued_stride=4, outlets_stride=64, map="runtimestation_minimal", notes="Master stopped; fixed finite graph; portion quota only in frozen benchmark sources. No production changes."), indent=2))


def compile_build(folder, byond, mode):
    provenance=json.loads((folder/"provenance.json").read_text())
    target=Path(provenance.get("build_roots",{}).get(mode, folder/mode))
    stem=provenance.get("binary_stem","atmos-cost.test")
    with (folder / f"compile-{mode}.log").open("w", encoding="utf-8") as stream:
        subprocess.run([str(byond / "dm.exe"), "-DCBT", f"{stem}.dme"], cwd=target, stdout=stream, stderr=subprocess.STDOUT, check=True, timeout=420)


def capture(folder, byond, mode, smoke, case_name=None, extra_params=""):
    if case_name and not case_name.replace("-", "").replace("_", "").isalnum():
        raise ValueError("Invalid case directory")
    output = folder / (case_name or (mode + ("-smoke" if smoke else "-run")))
    if output.exists():
        raise SystemExit("Preserve previous capture; use another directory")
    output.mkdir()
    stage(output, proc_profile=False)
    with (output / "config/config.txt").open("a", encoding="utf-8") as stream:
        stream.write("\nFORBID_STATION_TRAITS\n")
    shutil.copy2(ROOT / "_maps/map_jsons/runtimestation_minimal.json", output / "data/next_map.json")
    shutil.copy2(DLL, output / "bandastation_atmos.dll")
    provenance=json.loads((folder/"provenance.json").read_text())
    target=Path(provenance.get("build_roots",{}).get(mode, folder/mode))
    stem=provenance.get("binary_stem","atmos-cost.test")
    for suffix in ("dmb", "rsc"):
        shutil.copy2(target/f"{stem}.{suffix}", output / f"tgstation.{suffix}")
    samples = []
    tid = None
    params = "log-directory=atmos-cost" + ("&cost-size=8&cost-frames=4&cost-repeats=1" if smoke else "")
    if extra_params:
        params += "&" + extra_params
    with (output / "launcher.log").open("w", encoding="utf-8") as stream:
        process = subprocess.Popen([str(byond / "dd.exe"), "tgstation.dmb", "-trusted", "-close", "-invisible", "-ip", "127.0.0.1", "-logself", "-params", params], cwd=output, stdout=stream, stderr=subprocess.STDOUT)
        started = time.monotonic()
        try:
            while process.poll() is None:
                if time.monotonic() - started > 1500:
                    raise TimeoutError("Controlled atmosphere capture timed out")
                sample = windows_process_sample(process.pid, tid)
                if sample:
                    tid = sample.get("main_thread_id")
                    samples.append(sample)
                try:
                    process.wait(timeout=0.01)
                except subprocess.TimeoutExpired:
                    pass
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            write(output / "process-samples.json", json.dumps(samples))
            write(output / "exit.json", json.dumps(dict(exit_code=process.returncode, server_pid=process.pid, binary_sha256=sha(output / "tgstation.dmb"), dll_sha256=sha(output / "bandastation_atmos.dll"), requested_sample_period_seconds=0.01)))
    runtime = validate_runtime_logs(output)
    write(output / "runtime-validation.json", json.dumps(runtime, indent=2, ensure_ascii=False))
    if process.returncode or runtime["unexpected_count"]:
        raise SystemExit(f"Failed controlled capture: {output}")
    if not (output / "atmos-cost-results.json").exists():
        raise SystemExit("Missing completed controlled cases")
    print(f"Accepted {output}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "compile", "capture"))
    parser.add_argument("--name", required=True)
    parser.add_argument("--byond", type=Path, default=Path("E:/Games/BYOND/bin"))
    parser.add_argument("--mode", choices=("before", "current"))
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--reuse-source", type=Path, help="Reuse verified frozen DM/assets; preserve old DME and binaries")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    folder = ROOT / "tmp" / args.name
    if args.action == "prepare":
        prepare(folder, args.reuse_source.resolve() if args.reuse_source else None)
    elif not args.mode:
        parser.error("--mode is required")
    elif args.action == "compile":
        compile_build(folder, args.byond, args.mode)
    else:
        capture(folder, args.byond, args.mode, args.smoke)


if __name__ == "__main__":
    main()
