"""Verify space heat capacity and the reported breach in a complete Banda DM build."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import catalog

ROOT = catalog.ROOT
sys.path.insert(0, str(ROOT / "tools/profiling"))
from run import stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--record-boundaries", action="store_true")
    parser.add_argument("--suite", choices=("boundary", "fire", "thermal", "integration"), default="boundary")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid capture name")
    output = ROOT / "tmp/atmos-space" / args.name
    if output.exists():
        parser.error("Use a new capture name")
    boundary_path = ROOT / "tools/atmos" / f"{args.suite}-world-reference.json"
    if args.record_boundaries and boundary_path.exists():
        parser.error("World reference already exists; never overwrite silently")
    reference = json.loads((ROOT / "tools/atmos/boundary-reference.json").read_text(encoding="utf-8"))
    if args.suite == "fire":
        import fire
        inputs = fire.cases()
    elif args.suite == "thermal":
        import thermal
        inputs = thermal.cases()
    elif args.suite == "integration":
        import fire
        import thermal
        ordinary = json.loads((ROOT / "tools/atmos/network-reference.json").read_text(encoding="utf-8"))
        inputs = [c["input"] for c in ordinary["cases"] if c["input"]["active"] == list(range(len(c["input"]["nodes"])))]
        inputs += [c["input"] for c in reference["cases"] if all(n.get("kind",0) != 2 for n in c["input"]["nodes"])]
        inputs += fire.cases() + thermal.cases()
        inputs = [dict(c,hotspots=True) for c in inputs]
    else:
        inputs = [c["input"] for c in reference["cases"]]
    fixture_sha256={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in ("tools/atmos/world_network.dm.in","tools/atmos/space_world.dm.in")}
    output.mkdir(parents=True)
    stage(output)
    shutil.copy2(ROOT / "_maps/map_jsons/metastation.json", output / "data/next_map.json")
    (output / "world-network-cases.json").write_text(json.dumps(inputs), encoding="utf-8")
    dme = ROOT / "local-atmos-space.test.dme"
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig") + '\n#include "tools/atmos/world_network.dm.in"\n#include "tools/atmos/space_world.dm.in"\n', encoding="utf-8", newline="\n")
    print("Compiling complete Banda space audit", flush=True)
    subprocess.run([str(args.byond / ("dm.exe" if os.name == "nt" else "DreamMaker")), "-DCBT", str(dme)], cwd=ROOT, check=True, timeout=300)
    for ext in ("dmb", "rsc"):
        shutil.copy2(dme.with_suffix("." + ext), output / ("tgstation." + ext))
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(args.byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")), "tgstation.dmb", "-trusted", "-close", "-invisible",
                        "-ip", "127.0.0.1", "-logself", "-params", "log-directory=local-atmos-space"],
                       cwd=output, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
    logs = "\n".join(p.read_text(encoding="utf-8-sig", errors="replace") for p in output.rglob("*.log"))
    if "runtime error:" in logs.lower() or "runtime in " in logs.lower():
        raise SystemExit(f"Runtime in space audit; inspect {output}")
    observed = json.loads((output / "space-output.json").read_text(encoding="utf-8-sig"))
    case = next(c for c in reference["cases"] if c["input"]["label"] == "breach_floor_runs_first")
    expected = [dict(temperature=float(s["cells"][0]["temperature"]), moles=sum(map(float, s["cells"][0]["moles"]))) for s in case["expected"][:2]]
    checks = dict(space_heat_capacity=observed["space_heat_capacity"] == 7000,
                  space_heat_capacity_archive=observed["space_heat_capacity_archive"] == 0,
                  standalone_breach_matches_full_build=observed["breach_floor_runs_first"] == expected)
    assert len(observed["networks"]) == len(inputs), "Incomplete world graph capture"
    sources = [catalog.MIXTURE, "code/modules/atmospherics/gasmixtures/immutable_mixtures.dm", "code/__DEFINES/atmospherics/atmos_core.dm",
               "code/modules/atmospherics/environmental/LINDA_turf_tile.dm", "code/modules/atmospherics/environmental/LINDA_fire.dm", "code/controllers/subsystem/air.dm"]
    if args.suite != "boundary": sources = list(dict.fromkeys([*catalog.extract()["sources"], *sources]))
    report = dict(observed=observed, checks=checks,
                  source_revision=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                  source_sha256={p: hashlib.sha256(catalog.read(p).encode()).hexdigest() for p in sources},
                  fixture_sha256=fixture_sha256,
                  binary_sha256=hashlib.sha256((output / "tgstation.dmb").read_bytes()).hexdigest())
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    snapshot = dict(schema=1, byond=observed["byond"], input=inputs, observed=observed["networks"], source_sha256=report["source_sha256"])
    if args.record_boundaries:
        boundary_path.write_text(json.dumps(snapshot, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    elif boundary_path.exists():
        saved = json.loads(boundary_path.read_text(encoding="utf-8"))
        assert saved == snapshot, "World boundary behavior changed; review explicitly"
    print(json.dumps(dict(observed={k:v for k,v in observed.items() if k != "networks"}, checks=checks, networks=len(inputs)), ensure_ascii=False))
    raise SystemExit(not all(checks.values()))


if __name__ == "__main__":
    main()
