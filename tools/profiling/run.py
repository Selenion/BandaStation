"""Build a profiling harness, stage an isolated local server, collect a baseline."""

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
from pathlib import Path
import shutil
import subprocess
import time

from runtime_check import validate_runtime_logs

ROOT = Path(__file__).resolve().parents[2]


def windows_thread_sample(pid, thread_id=None):
    """The process's oldest thread is its initial BYOND thread; never change its affinity."""
    from ctypes import wintypes

    class Entry(ctypes.Structure):
        _fields_ = [(name, wintypes.DWORD) for name in ("size", "usage", "thread_id", "process_id")] + [
            ("base_priority", wintypes.LONG), ("delta_priority", wintypes.LONG), ("flags", wintypes.DWORD)]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenThread.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetThreadTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4

    def read(identifier):
        handle = kernel.OpenThread(0x800, False, identifier)  # QUERY_LIMITED_INFORMATION
        if not handle:
            return None
        try:
            times = [ctypes.c_uint64() for _ in range(4)]
            if kernel.GetThreadTimes(handle, *(ctypes.byref(value) for value in times)):
                return times[0].value, identifier, (times[2].value + times[3].value) / 1e7
        finally:
            kernel.CloseHandle(handle)

    if thread_id is not None:
        return read(thread_id)
    kernel.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel.Thread32First.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    kernel.Thread32Next.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    snapshot = kernel.CreateToolhelp32Snapshot(4, 0)  # TH32CS_SNAPTHREAD
    if snapshot == wintypes.HANDLE(-1).value:
        return None
    candidates = []
    try:
        entry = Entry()
        entry.size = ctypes.sizeof(entry)
        more = kernel.Thread32First(snapshot, ctypes.byref(entry))
        while more:
            if entry.process_id == pid:
                value = read(entry.thread_id)
                if value:
                    candidates.append(value)
            more = kernel.Thread32Next(snapshot, ctypes.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)
    return min(candidates) if candidates else None


def windows_process_sample(pid, main_thread_id=None):
    """Read counters for this run's console DreamDaemon, without extra packages."""
    from ctypes import wintypes

    class Memory(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in ["peak_working", "working", "peak_paged", "paged",
                                               "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile", "private"]]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
    psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Memory), wintypes.DWORD]
    handle = kernel.OpenProcess(0x410, False, pid)  # QUERY_INFORMATION | VM_READ
    if not handle:
        return None
    try:
        memory = Memory()
        memory.cb = ctypes.sizeof(memory)
        times = [ctypes.c_uint64() for _ in range(4)]
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(memory), memory.cb):
            return None
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            return None
        sample = dict(utc=datetime.now(timezone.utc).isoformat(), cpu_seconds=(times[2].value + times[3].value) / 1e7,
                      working_set_bytes=memory.working, private_bytes=memory.private,
                      os_peak_working_set_bytes=memory.peak_working, os_peak_commit_bytes=memory.peak_pagefile)
        thread = windows_thread_sample(pid, main_thread_id)
        if thread:
            sample.update(main_thread_id=thread[1], main_thread_cpu_seconds=thread[2])
        return sample
    finally:
        kernel.CloseHandle(handle)


def capture(daemon, output, scenario, rust_enabled=False, rust_threaded=False):
    samples = []
    main_thread_id = None
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(daemon), "tgstation.dmb", "-trusted", "-close", "-invisible", "-ip", "127.0.0.1",
                                    "-logself", "-params", f"log-directory=local-profile&profile-scenario={scenario}&profile-rust={int(rust_enabled)}&profile-threaded={int(rust_threaded)}"],
                                   cwd=output, stdout=log, stderr=subprocess.STDOUT)
        started = time.monotonic()
        try:
            while process.poll() is None:
                if time.monotonic() - started > 600:
                    process.kill()
                    process.wait()
                    raise TimeoutError("Local profiling run exceeded 600 seconds")
                if os.name == "nt":
                    sample = windows_process_sample(process.pid, main_thread_id)
                    if sample:
                        main_thread_id = sample.get("main_thread_id")
                        samples.append(sample)
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    pass
        finally:
            # Interrupting a diagnostic must not leave its local server running.
            if process.poll() is None:
                process.kill()
                process.wait()
            if samples:
                (output / "process-samples.json").write_text(json.dumps(samples), encoding="utf-8")
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, process.args)
    return time.monotonic() - started


def stage(target):
    """Hard-link immutable runtime resources; copy all mutable config/data files."""
    resources = ["_maps", "build/behavior_trees", "code/datums/greyscale/json_configs",
                 "icons", "sound/runtime", "strings", "tgui/public", "modular_bandastation"]
    for resource in resources:
        source = ROOT / resource
        for directory, _, names in os.walk(source):
            relative = Path(directory).relative_to(ROOT)
            (target / relative).mkdir(parents=True, exist_ok=True)
            for name in names:
                if name.endswith((".dm", ".dme")):
                    continue
                dest = target / relative / name
                if not dest.exists():
                    os.link(Path(directory) / name, dest)
    shutil.copytree(ROOT / "config", target / "config", dirs_exist_ok=True)
    with (target / "config/config.txt").open("a", encoding="utf-8") as config:
        config.write("\nAUTO_PROFILE\nPROFILER_INTERVAL 6000\n")
    # This deployment is intentionally disconnected from the live database.
    (target / "config/dbconfig.txt").write_text("# Local profile: SQL disabled\n", encoding="utf-8")
    (target / "data").mkdir(exist_ok=True)
    for dll in ROOT.glob("*.dll"):
        shutil.copy2(dll, target / dll.name)


def finish_capture(output, manifest, elapsed):
    validation = validate_runtime_logs(output)
    manifest["runtime_validation"] = validation
    (output / "runtime-validation.json").write_text(json.dumps(validation, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if validation["unexpected_count"]:
        (output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
        raise SystemExit(f"Unexpected runtime in profiling run; inspect {output / 'runtime-validation.json'}")
    metadata_path = output / "profile-metadata.json"
    if not metadata_path.exists():
        raise SystemExit(f"No completed profile. Inspect {output / 'tgstation.log'}")
    manifest["total_wall_seconds"] = elapsed
    manifest["metadata"] = json.loads(metadata_path.read_text(encoding="utf-8-sig"))
    manifest["binary_sha256"] = hashlib.sha256((output / "tgstation.dmb").read_bytes()).hexdigest()
    (output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    if validation["known_count"]:
        print(f"Accepted with {validation['known_count']} known baseline runtimes; see runtime-validation.json")
    print(f"Captured profile: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--map", default="metastation")
    parser.add_argument("--name", default="baseline")
    parser.add_argument("--scenario", choices=["empty-station", "atmos-fire", "atmos-flood", "sm-plasma"], default="empty-station")
    parser.add_argument("--source-root", type=Path, default=ROOT)
    parser.add_argument("--compiled-binary", type=Path)
    parser.add_argument("--rust-dll", type=Path)
    parser.add_argument("--threaded", action="store_true")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    if args.threaded and not args.rust_dll:
        parser.error("--threaded requires --rust-dll")
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("--name must contain letters, digits, hyphens or underscores")
    source_root = args.source_root.resolve()
    map_source = source_root / "_maps/map_jsons" / (args.map + ".json")
    if map_source.parent != source_root / "_maps/map_jsons" or not map_source.is_file():
        parser.error("Unknown map")
    output = ROOT / "tmp/profiling" / args.name
    if output.exists():
        parser.error(f"Use a new --name to preserve the previous capture: {output}")
    output.mkdir(parents=True)
    dme = source_root / "local-profile.test.dme"
    source = (source_root / "tgstation.dme").read_text(encoding="utf-8-sig")
    source += f'\n#include "{(ROOT / "tools/profiling/local_profile.dm.in").as_posix()}"\n'
    dme.write_text(source, encoding="utf-8", newline="\n")
    stage(output)
    with (output / "config/config.txt").open("a", encoding="utf-8") as config:
        config.write("\nFORBID_STATION_TRAITS\n")
        if args.rust_dll:
            config.write("RUST_ATMOS_LIBRARY bandastation_atmos.dll\n")
        if args.threaded:
            config.write("RUST_ATMOS_THREADED\n")
    if args.rust_dll:
        shutil.copy2(args.rust_dll, output / "bandastation_atmos.dll")
    shutil.copy2(map_source, output / "data/next_map.json")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    manifest = dict(revision=revision, map=args.map, scenario=args.scenario, warmup_seconds=30,
                    sample_seconds=60, profiler="BYOND world.Profile", rust_enabled=bool(args.rust_dll), rust_threaded=args.threaded, source_root=str(source_root),
                    baseline_source=json.loads((source_root / "baseline-source.json").read_text(encoding="utf-8")) if (source_root / "baseline-source.json").exists() else None,
                    harness_sha256=hashlib.sha256((ROOT / "tools/profiling/local_profile.dm.in").read_bytes()).hexdigest(),
                    limitations=["No connected players; SendMaps is not representative of 50+ players.",
                                 "Fixed startup seed; scheduling and subsequent random background events can diverge.",
                                 "Native profiling adds overhead; use identical profiling settings in comparisons."])
    manifest["platform"] = platform.platform()
    manifest["logical_cpus"] = os.cpu_count()
    manifest["dll_sha256"] = {dll.name: hashlib.sha256(dll.read_bytes()).hexdigest() for dll in output.glob("*.dll")}
    (output / "manifest.json").write_text(json.dumps(manifest, indent="\t") + "\n", encoding="utf-8")
    print(f"Prepared {output}", flush=True)
    if args.prepare_only:
        return
    compiler = args.byond / ("dm.exe" if os.name == "nt" else "DreamMaker")
    if not args.compiled_binary:
        subprocess.run([str(compiler), "-DCBT", str(dme)], cwd=source_root, check=True, timeout=300)
    binary = args.compiled_binary or dme.with_suffix(".dmb")
    for extension in ("dmb", "rsc"):
        shutil.copy2(binary.with_suffix("." + extension), output / ("tgstation." + extension))
    daemon = args.byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")
    elapsed = capture(daemon, output, args.scenario, bool(args.rust_dll), args.threaded)
    finish_capture(output, manifest, elapsed)


if __name__ == "__main__":
    main()
