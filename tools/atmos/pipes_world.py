"""Boot real Banda SSair pipe reconciliation against the original DM path."""
import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

import catalog
import pipenet
import transport

ROOT = catalog.ROOT
sys.path.insert(0, str(ROOT / "tools/profiling"))
from run import stage
from runtime_check import validate_runtime_logs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--dll", type=Path, required=True)
    parser.add_argument("--compiled-binary", type=Path, help="Reuse a DM build compiled from the same fixture and source")
    args = parser.parse_args()
    if not args.name.replace("-", "").isalnum():
        parser.error("Invalid name")
    output = ROOT / "tmp/atmos-pipes-world" / args.name
    if output.exists():
        parser.error("Use a new name")
    output.mkdir(parents=True)
    stage(output)
    shutil.copy2(args.dll, output / "bandastation_atmos.dll")
    shutil.copy2(ROOT / "_maps/map_jsons/metastation.json", output / "data/next_map.json")
    with (output / "config/config.txt").open("a", encoding="utf-8") as config:
        config.write("\nFORBID_STATION_TRAITS\n")
    cases = pipenet.cases()
    # The long arithmetic chain is covered by the isolated oracle, without running world systems.
    cases = [c for c in cases if "steps" not in c]
    (output / "pipe-cases.json").write_text(json.dumps(cases), encoding="utf-8")
    dme = ROOT / "local-atmos-pipes-world.test.dme"
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig") +
                   '\n#include "tools/atmos/pipes_world.dm.in"\n', encoding="utf-8", newline="\n")
    source_paths = ["tools/atmos/pipes_world.dm.in", "code/controllers/subsystem/air.dm",
                    "code/modules/atmospherics/machinery/datum_pipeline.dm", "modular_bandastation/atmos_rust/code/pipes.dm",
                    "modular_bandastation/atmos_rust/code/phase.dm", "rust/atmos/src/byond_pipe_direct.rs",
                    "rust/atmos/src/byond_pipes.rs", "rust/atmos/src/pipenet_pool.rs"]
    source_hashes = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in source_paths}
    if not args.compiled_binary:
        subprocess.run([str(args.byond / "dm.exe"), "-DCBT", str(dme)], cwd=ROOT, check=True, timeout=360)
    binary = args.compiled_binary or dme
    assert source_hashes == {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in source_paths}, "Source changed during compilation"
    for ext in ("dmb", "rsc"):
        shutil.copy2(binary.with_suffix("." + ext), output / ("tgstation." + ext))
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(args.byond / "dd.exe"), "tgstation.dmb", "-trusted", "-close", "-invisible", "-ip", "127.0.0.1",
                        "-logself"], cwd=output, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=600)
    runtime = validate_runtime_logs(output)
    (output / "runtime-validation.json").write_text(json.dumps(runtime, indent=2), encoding="utf-8")
    assert runtime["unexpected_count"] == 0, runtime
    actual = json.loads((output / "pipes-output.json").read_text(encoding="utf-8-sig"))
    reference = json.loads((ROOT / "tools/atmos/pipenet-reference.json").read_text())
    expected = [e for c,e in zip(reference["input"], reference["observed"]) if "steps" not in c]
    assert len(actual["cases"]) == len(expected) == len(cases)
    failures = []
    for case, group, source in zip(cases, actual["cases"], expected):
        assert len(group) == len(source), "Incomplete mixture output"
        for packet, dm in zip(group, source):
            count = int(packet[3]); gases = {int(packet[4 + i*2]): packet[5 + i*2] for i in range(count)}
            observed = dict(temperature=packet[0], moles=[gases.get(g,0) for g in range(20)], keys=list(gases))
            wanted = dict(temperature=float(dm["temperature"]), moles=list(map(float,dm["moles"])), keys=dm["keys"])
            diff = transport.difference(wanted, observed)
            if diff: failures.append(dict(case=case["label"], difference=diff))
            archive = 4 + count * 2 + 26 * 2
            if packet[archive] != 123 or packet[archive + 3:archive + 6] != [1,3,123] or packet[-1] != 42:
                failures.append(dict(case=case["label"], difference="Archive/last_share changed"))
    failures += [dict(check=k) for k,v in actual["checks"].items() if v != 1]
    benchmarks = actual.get("benchmarks", [])
    for repeat in {b["repeat"] for b in benchmarks}:
        group = {b["mode"]:b for b in benchmarks if b["repeat"] == repeat}
        assert set(group) == {"dm", "packet", "direct", "owned"}
        assert group["direct"]["packets"] == 0 and group["direct"]["direct_groups"] == 500
        assert group["owned"]["packets"] == 0 and group["owned"]["owned_groups"] == 500
        assert group["owned"]["imports"] == 24 * 500 and group["direct"]["imports"] == 32 * 500
        assert group["packet"]["packets"] == 32 * 500
        assert all(b["reconcile_seconds"] > 0 for b in group.values())
        for mode in ("packet", "direct", "owned"):
            for dm, packet in zip(group["dm"]["states"], group[mode]["states"]):
                def state(p):
                    gases = {int(p[4+i*2]):p[5+i*2] for i in range(int(p[3]))}
                    return dict(temperature=p[0], moles=[gases.get(g,0) for g in range(20)], keys=list(gases))
                diff = transport.difference(state(dm), state(packet))
                if diff: failures.append(dict(benchmark=mode, repeat=repeat, difference=diff))
    report = dict(cases=len(cases), checks=actual["checks"], failures=failures, runtime=runtime,
                  benchmarks=benchmarks,
                  source_hashes=source_hashes,
                  dll_sha256=hashlib.sha256((output / "bandastation_atmos.dll").read_bytes()).hexdigest())
    (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"Pipe world: {len(cases)} cases, {len(actual['checks'])} checks; failures: {failures}")
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
