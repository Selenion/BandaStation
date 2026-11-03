"""Capture and check real Banda world effects in an isolated local game process."""
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

SOURCES = [catalog.GASES, catalog.REACTIONS, catalog.MIXTURE,
           "code/modules/atmospherics/environmental/LINDA_fire.dm", "code/game/turfs/open/_open.dm",
           "code/datums/elements/frozen.dm", "code/datums/components/wet_floor.dm", "code/datums/status_effects/gas.dm",
           *catalog.DEFINES]


def cases():
    result = []
    for temperature in (190, 200, 200.01, 283, 303.15, 303.16):
        result.append(dict(id=f"water:{temperature}", temperature=temperature, gases=[["water_vapor", 1]], operation="react"))
    for resistant, temperature in ((False, 40), (True, 40), (False, 51)):
        result.append(dict(id=f"freeze:body={temperature},resistant={resistant}", temperature=190,
                           gases=[["water_vapor", 1]], operation="react", person=True,
                           bodytemperature=temperature, cold_resistant=resistant))
    result.append(dict(id="existing_permafrost", temperature=190, gases=[["water_vapor", 1]], operation="react", permafrost=True))
    for oxygen in (0.4999, 0.5, 0.51):
        result.append(dict(id=f"hotspot:oxygen={oxygen}", temperature=1644,
                           gases=[["plasma", 1], ["o2", oxygen], ["hydrogen", 1]], operation="hotspot"))
    result.append(dict(id="reaction_intermediate_fire", temperature=1644,
                       gases=[["plasma", 1], ["o2", 0.51], ["hydrogen", 1]], operation="react"))
    return result


def feedback_cases():
    return [dict(id=f"coal_feedback:deferred={deferred}", temperature=500,
                 gases=[["hydrogen", 1], ["plasma", 10], ["o2", 10]], operation="react",
                 coal=50, defer_fire=deferred) for deferred in (False, True)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True, help="Unique capture directory name")
    parser.add_argument("--record", action="store_true", help="Create reference once; never overwrite")
    parser.add_argument("--suite", choices=("base", "feedback", "tick"), default="base")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    reference_path = ROOT / "tools/atmos" / ("world-reference.json" if args.suite == "base" else f"world-{args.suite}-reference.json")
    if args.record and reference_path.exists():
        parser.error("Reference already exists; do not overwrite behavior silently")
    output = ROOT / "tmp/atmos-world" / args.name
    if output.exists():
        parser.error("Use a new name to preserve the earlier capture")
    output.mkdir(parents=True)
    stage(output)
    # This harness observes effects; automatic profiling is unnecessary here.
    config = output / "config/config.txt"
    config.write_text(config.read_text(encoding="utf-8-sig").replace("\nAUTO_PROFILE\n", "\n# AUTO_PROFILE\n"), encoding="utf-8")
    shutil.copy2(ROOT / "_maps/map_jsons/metastation.json", output / "data/next_map.json")
    inputs = cases() if args.suite == "base" else feedback_cases()
    if args.suite == "tick":
        inputs = [dict(case, tick=True) for case in cases() if case["operation"] == "react"]
        inputs += [dict(feedback_cases()[1], tick=True)]
    (output / "world-cases.json").write_text(json.dumps(inputs), encoding="utf-8")
    dme = ROOT / "local-atmos-behavior.test.dme"
    dme.write_text((ROOT / "tgstation.dme").read_text(encoding="utf-8-sig")
                   + '\n#include "tools/atmos/world_network.dm.in"\n#include "tools/atmos/world_behavior.dm.in"\n', encoding="utf-8", newline="\n")
    compiler = args.byond / ("dm.exe" if os.name == "nt" else "DreamMaker")
    print("Compiling isolated world behavior harness", flush=True)
    subprocess.run([str(compiler), "-DCBT", str(dme)], cwd=ROOT, check=True, timeout=300)
    for extension in ("dmb", "rsc"):
        shutil.copy2(dme.with_suffix("." + extension), output / ("tgstation." + extension))
    daemon = args.byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")
    with (output / "launcher.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(daemon), "tgstation.dmb", "-trusted", "-close", "-invisible", "-ip", "127.0.0.1",
                        "-logself", "-params", "log-directory=local-atmos-behavior"], cwd=output,
                       stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
    logs = "\n".join(path.read_text(encoding="utf-8-sig", errors="replace") for path in output.rglob("*.log"))
    if "runtime error:" in logs.lower() or "runtime in " in logs.lower():
        raise SystemExit(f"Runtime in world harness; inspect {output}")
    observed = json.loads((output / "world-output.json").read_text(encoding="utf-8-sig"))
    assert [case["id"] for case in observed["cases"]] == [case["id"] for case in inputs], "Incomplete capture"
    sources = SOURCES + (["code/game/objects/items/stacks/sheets/mineral.dm"] if args.suite != "base" else [])
    snapshot = dict(schema=1, source_revision=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    harness_sha256=hashlib.sha256(catalog.read("tools/atmos/world_behavior.dm.in").encode()).hexdigest(),
                    source_sha256={path: hashlib.sha256(catalog.read(path).encode()).hexdigest() for path in sources},
                    input=inputs, observed=observed)
    (output / "manifest.json").write_text(json.dumps(snapshot, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
    if args.record:
        reference_path.write_text(json.dumps(snapshot, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
        print(f"Recorded {len(inputs)} real world scenarios in {reference_path}")
    else:
        saved = json.loads(reference_path.read_text(encoding="utf-8"))
        # Source changes are reported separately: the behavior comparison remains useful after a port.
        changed = [path for path, digest in saved["source_sha256"].items() if snapshot["source_sha256"][path] != digest]
        if changed:
            print("Changed source files:", ", ".join(changed))
        assert saved["input"] == inputs, "Reference scenarios changed"
        assert saved["observed"] == observed, f"Banda world behavior changed; inspect {output}"
        print(f"World behavior: {len(inputs)}/{len(inputs)} match")


if __name__ == "__main__":
    main()
