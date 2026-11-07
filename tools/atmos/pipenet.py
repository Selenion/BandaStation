"""Capture Banda pipenet reconciliation from the original DM procedure."""
import argparse
import hashlib
import json
import random
import re
from pathlib import Path

import catalog
import compare

ROOT = catalog.ROOT
SOURCE = "code/modules/atmospherics/machinery/datum_pipeline.dm"


def original_pipeline():
    # Ignore only the additive dispatch hook; the captured original math must remain unchanged.
    return re.sub(r'\t// BANDASTATION ADDITION START - Native pipe reconciliation preserves DM mixture order and aliases\.\n.*?\t// BANDASTATION ADDITION END - Native pipe reconciliation preserves DM mixture order and aliases\.\n',
                  '', catalog.read(SOURCE), flags=re.S)


def cases():
    def mix(volume, gases, temperature=300):
        return dict(volume=volume, gases=list(gases.items()), temperature=temperature)

    def case(label, mixtures, order=None):
        return dict(label=label, mixtures=mixtures, order=order or list(range(len(mixtures))))

    result = [
        case("volume_split", [mix(200, {0: 10}), mix(800, {1: 90}, 600)]),
        case("zero_volume_port", [mix(0, {0: 10}), mix(1000, {1: 90})]),
        case("zero_total_volume", [mix(0, {0: 10}), mix(0, {1: 90})]),
        case("empty_network", [mix(200, {}, 700), mix(800, {}, 200)]),
        case("trace_keys", [mix(1, {3: 0.0001, 1: 0}), mix(100000, {0: 1})]),
        case("duplicate_port", [mix(200, {0: 10}), mix(800, {1: 90}, 600)], [0, 0, 1]),
        case("duplicate_main", [mix(200, {0: 10}), mix(800, {1: 90}, 600)], [0, 1, 1]),
    ]
    chain = case("ten_thousand_steps", [mix(200, {0: 10}), mix(800, {1: 90}, 600)])
    chain["steps"] = 10000
    result.append(chain)
    rng = random.Random(220)
    for index in range(64):
        mixtures = [mix(rng.choice([0, 200, 1000, 2500]),
                        {g: 10 ** rng.uniform(-6, 4) for g in rng.sample(range(20), rng.randrange(1, 21))},
                        rng.uniform(2.7, 10000)) for _ in range(rng.randrange(2, 9))]
        result.append(case(f"random:{index}", mixtures))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path)
    parser.add_argument("--name")
    parser.add_argument("--check", action="store_true", help="Check frozen source hashes and input cases without running BYOND")
    args = parser.parse_args()
    if args.check:
        reference = json.loads((ROOT / "tools/atmos/pipenet-reference.json").read_text(encoding="utf-8"))
        assert reference["input"] == json.loads(json.dumps(cases())), "Pipenet input contract changed; review and capture a new reference"
        assert len(reference["input"]) == len(reference["observed"]), "Incomplete frozen reference"
        for path, digest in reference["source_hashes"].items():
            content = (ROOT / path).read_bytes()
            if path == SOURCE:
                # The pinned source had LF line endings, like catalog.read().
                content = original_pipeline().encode("utf-8")
            assert hashlib.sha256(content).hexdigest() == digest, f"Original source changed: {path}"
        print(f"Checked {len(reference['input'])} frozen cases and {len(reference['source_hashes'])} source hashes")
        return
    if not args.byond or not args.name:
        parser.error("Capture requires --byond and --name")
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    work = ROOT / "tmp/atmos-pipenet" / args.name
    if work.exists():
        parser.error("Use a new name to preserve observations")
    compare.WORK = work
    data = catalog.extract()
    compare.harness(data)
    compare.write("oracle-header.dm", '#define WRAP_UID(value) ((value) % 16777215)\n' +
                  catalog.read("tools/atmos/oracle.dm.in").split("world\n", 1)[0])
    mixture = catalog.blocks(catalog.read(catalog.MIXTURE))
    compare.write("pipenet-mixture.dm", mixture["/datum/gas_mixture/proc/copy_from_ratio"])
    original = catalog.blocks(original_pipeline())["/datum/pipeline/proc/reconcile_air"]
    compare.write("reconcile.dm", original)
    compare.write("pipenet.dm", catalog.read("tools/atmos/pipenet.dm.in"))
    files = ("definitions.dm", "oracle-header.dm", "setup.dm", "mixture.dm", "reactions.dm",
             "electrolyzer.dm", "pipenet-mixture.dm", "reconcile.dm", "pipenet.dm")
    compare.write("pipenet.dme", "\n".join(f'#include "{p}"' for p in files) + "\n")
    inputs = cases()
    compare.write("cases.json", json.dumps(inputs))
    print(compare.command([args.byond / "dm.exe", "pipenet.dme"], cwd=work))
    print(compare.command([args.byond / "dd.exe", "pipenet.dmb", "-safe", "-close", "-invisible", "-logself"], cwd=work))
    log = (work / "pipenet.log").read_text(encoding="utf-8-sig", errors="replace")
    if "runtime error:" in log.lower() or "ORACLE_FAILURE" in log:
        raise RuntimeError(log[:5000])
    observed = json.loads((work / "dm-output.json").read_text(encoding="utf-8-sig"))
    assert len(observed["cases"]) == len(inputs), "Incomplete pipenet capture"
    sources = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
               for p in (SOURCE, catalog.MIXTURE, "tools/atmos/pipenet.dm.in", "tools/atmos/compare.py")}
    sources.update(data["sources"])
    report = dict(byond=observed["version"], input=inputs, observed=observed["cases"], source_hashes=sources)
    report["elapsed_seconds"] = observed["elapsed_seconds"]
    report["generated_source_hashes"] = {p: hashlib.sha256((work / p).read_bytes()).hexdigest() for p in files}
    compare.write("reference.json", json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"Captured {len(inputs)} original DM pipenet cases: {work}")


if __name__ == "__main__":
    main()
