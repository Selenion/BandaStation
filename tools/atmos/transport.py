"""Compare Rust gas/heat exchange with original Banda DM; preserve explicit reference fixtures."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re

import catalog
import compare

ROOT = catalog.ROOT
WORK = ROOT / "tmp/atmos-transport"
FIXTURE = ROOT / "tools/atmos/transport-reference.json"


def cases():
    def air(temperature, gases, turf=False, volume=2500):
        return dict(temperature=temperature, gases=list(gases.items()), turf=turf, volume=volume)

    def case(label, a, b, ca=0.2, cb=0.2, ticks=1, repeats=1, mode=0):
        return dict(label=label, a=a, b=b, ca=ca, cb=cb, ticks=ticks, repeats=repeats, mode=mode)

    reference = [
        case("equal_air", air(293.15, {0: 100, 1: 300}), air(293.15, {0: 100, 1: 300})),
        case("vacuum_release", air(293.15, {0: 100, 1: 300}, True), air(2.7, {}, True)),
        case("vacuum_receiver", air(2.7, {}, True), air(293.15, {0: 100, 1: 300}, True)),
        case("opposing_gases", air(293.15, {0: 10}), air(293.15, {3: 10})),
        case("unequal_neighbour_counts", air(600, {0: 80, 1: 320}), air(250, {0: 5, 1: 20}), ca=0.5, cb=1/7),
        case("archived_multiple_shares", air(800, {0: 100, 3: 30}), air(200, {0: 20, 4: 2}), repeats=3),
        case("all_gases", air(293.15, {i: i + 1 for i in range(20)}), air(600, {i: 20-i for i in reversed(range(20))})),
        case("trace_gc", air(293.15, {0: 0.0001, 1: 1}), air(293.15, {3: 0})),
        case("zero_keys", air(293.15, {3: 0, 0: 10}), air(293.15, {1: 0, 0: 10})),
        case("negative_quantization", air(293.15, {0: 0.00015}), air(293.15, {0: 0.0002})),
        case("positive_quantization", air(293.15, {0: 0.0002}), air(293.15, {0: 0.00015})),
        case("temperature_boundary", air(293.65, {0: 10}), air(293.15, {0: 10}), mode=1),
        case("vacuum_conduction", air(700, {}, True), air(2.7, {}, True), mode=1, ca=0.4),
        case("no_capacity_conduction", air(700, {}), air(293.15, {0: 10}), mode=1, ca=0.4),
        case("twenty_exchange_ticks", air(1600, {3: 20, 0: 100}), air(2.7, {1: 400}, True), ticks=20),
        case("twenty_conduction_ticks", air(800, {0: 50}), air(200, {3: 100}), ticks=20, mode=1, ca=0.4),
        case("complete_space_share", air(600, {0: 100, 3: 20}, True), air(2.7, {}, True), ca=1, cb=1),
    ]
    generated = []
    rng = random.Random(220)
    for index in range(240):
        mixtures = []
        for _ in range(2):
            gases = {g: 10 ** rng.uniform(-5, 4) for g in rng.sample(range(20), rng.randrange(1, 21))}
            mixtures.append(air(rng.uniform(2.7, 10000), gases, index % 3 == 0))
        generated.append(case(f"random:{index}", *mixtures, ca=1/rng.randrange(2, 8), cb=1/rng.randrange(2, 8),
                              mode=index % 2, ticks=20 if index % 5 == 0 else 1))
    return reference, generated


def difference(expected, actual, path="", numeric_strings=True):
    if expected is None or actual is None:
        return None if expected is actual else f"{path}: {expected} != {actual}"
    if isinstance(expected, str) or isinstance(actual, str):
        if not numeric_strings:
            return None if expected == actual else f"{path}: {expected!r} != {actual!r}"
        try:
            float(expected), float(actual)
        except (TypeError, ValueError):
            return None if expected == actual else f"{path}: {expected!r} != {actual!r}"
    if isinstance(expected, dict):
        if set(expected) != set(actual):
            return f"{path}: different fields"
        for key in expected:
            diff = difference(expected[key], actual[key], path + "." + key, numeric_strings)
            if diff:
                return diff
        return None
    if isinstance(expected, list):
        if len(expected) != len(actual):
            return f"{path}: length {len(expected)} != {len(actual)}"
        for index, (a, b) in enumerate(zip(expected, actual)):
            diff = difference(a, b, f"{path}[{index}]", numeric_strings)
            if diff:
                return diff
        return None
    if ".keys" in path or ".archived_keys" in path:
        return None if expected == actual else f"{path}: {expected} != {actual}"
    return None if math.isclose(float(expected), float(actual), rel_tol=2e-5, abs_tol=2e-5) else f"{path}: {expected} != {actual}"


def harness(data):
    compare.WORK = WORK
    compare.harness(data)
    source = catalog.read("tools/atmos/oracle.dm.in").split("world\n", 1)[0]
    compare.write("oracle-header.dm", source)
    blocks = catalog.blocks(catalog.read(catalog.MIXTURE))
    names = ("proc/archive", "proc/heat_capacity_archive", "turf/heat_capacity_archive", "proc/share", "proc/temperature_share")
    compare.write("transport-impl.dm", "\n".join(blocks["/datum/gas_mixture/" + name] for name in names))
    macro = re.search(r"#define CALCULATE_CONDUCTION_ENERGY[^\n]+\\\n[^\n]+", catalog.read("code/__DEFINES/atmospherics/atmos_helpers.dm"))
    assert macro, "Conduction macro changed; review extraction"
    compare.write("conduction.dm", macro[0] + "\n")
    compare.write("transport.dm", catalog.read("tools/atmos/transport.dm.in"))
    files = ("definitions.dm", "conduction.dm", "oracle-header.dm", "setup.dm", "mixture.dm", "reactions.dm", "electrolyzer.dm", "transport-impl.dm", "transport.dm")
    compare.write("transport.dme", "\n".join(f'#include "{p}"' for p in files) + "\n")


def run_reference(byond, inputs):
    compare.write("cases.json", json.dumps(inputs))
    print(compare.command([byond / ("dm.exe" if os.name == "nt" else "DreamMaker"), "transport.dme"], cwd=WORK))
    output = WORK / "dm-output.json"
    if output.exists():
        output.unlink()
    compare.write("transport.log", "")
    log = compare.command([byond / ("dd.exe" if os.name == "nt" else "DreamDaemon"), "transport.dmb", "-safe", "-close", "-invisible", "-logself"], cwd=WORK)
    log += (WORK / "transport.log").read_text(encoding="utf-8-sig", errors="replace")
    if "runtime error:" in log.lower() or "ORACLE_FAILURE" in log:
        raise RuntimeError(log[:5000])
    result = json.loads(output.read_text(encoding="utf-8-sig"))
    assert len(result["cases"]) == len(inputs), "Incomplete reference capture"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--rust", type=Path, default=ROOT / "rust/atmos/target/debug/transport.exe")
    parser.add_argument("--record", action="store_true", help="Create the DM reference once; refuses to overwrite")
    args = parser.parse_args()
    if args.record and FIXTURE.exists():
        parser.error("Reference already exists; review changes explicitly instead of overwriting")
    data = catalog.extract()
    harness(data)
    reference, generated = cases()
    inputs = reference + generated
    expected = run_reference(args.byond, inputs)
    sources = dict(data["sources"])
    helper = "code/__DEFINES/atmospherics/atmos_helpers.dm"
    sources[helper] = hashlib.sha256(catalog.read(helper).encode()).hexdigest()
    snapshot = dict(schema=1, byond=expected["version"], source_sha256=sources,
                    cases=[dict(input=c, expected=v) for c, v in zip(reference, expected["cases"])])
    snapshot = json.loads(json.dumps(snapshot))
    if args.record:
        FIXTURE.write_text(json.dumps(snapshot, ensure_ascii=False, indent="\t") + "\n", encoding="utf-8")
        print(f"Recorded {len(reference)} Banda DM reference scenarios")
    else:
        saved = json.loads(FIXTURE.read_text(encoding="utf-8"))
        assert saved == snapshot, "Banda behavior/source/version differs from pinned reference; do not refresh silently"
    lines = []
    for case in inputs:
        words = [case[k] for k in ("mode", "ticks", "repeats", "ca", "cb")]
        for key in ("a", "b"):
            sample = case[key]
            words += [sample["temperature"], sample["volume"], int(sample["turf"]), len(sample["gases"])]
            words += [n for pair in sample["gases"] for n in pair]
        lines.append(" ".join(map(str, words)))
    actual = [json.loads(line) for line in compare.command([args.rust], input_text="\n".join(lines) + "\n").splitlines()]
    assert len(actual) == len(inputs), "Incomplete Rust run"
    failures = [dict(label=c["label"], difference=diff, input=c, expected=e, actual=a)
                for c, e, a in zip(inputs, expected["cases"], actual) if (diff := difference(e, a))]
    compare.write("failures.json", json.dumps(failures, ensure_ascii=False, indent="\t"))
    print(f"Transport: {len(inputs)-len(failures)}/{len(inputs)} match; BYOND {expected['version']}")
    for failure in failures[:8]:
        print(f"  {failure['label']}: {failure['difference']}")
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
