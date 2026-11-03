"""Differential checks against the checked-out DM source (requires BYOND 516)."""

import argparse
import json
import math
import os
from pathlib import Path
import random
import struct
import subprocess

import catalog

ROOT = catalog.ROOT
WORK = ROOT / "tmp/atmos-oracle"


def write(name, text):
    (WORK / name).write_text(text, encoding="utf-8", newline="\n")


def harness(data):
    """Extract original implementations, rather than maintain a second DM model."""
    WORK.mkdir(parents=True, exist_ok=True)
    expressions, _ = catalog.constants()
    definitions = [f"#define {key} {expressions[key]}" for key in data["constants"]]
    definitions.append(f'#define ELECTROLYSIS_ARGUMENT_SUPERMATTER_POWER {expressions["ELECTROLYSIS_ARGUMENT_SUPERMATTER_POWER"]}')
    write("definitions.dm", "\n".join(definitions) + "\n")
    gas_paths = ", ".join(g["path"] for g in data["gases"])
    setup = ["/datum/gas", "\tvar/rarity = 0", "\tvar/id", "\tvar/specific_heat"]
    for gas in data["gases"]:
        setup += [gas["path"], f'\tid = "{gas["id"]}"', f'\trarity = {gas["rarity"]}', f'\tspecific_heat = {gas["specific_heat"]}']
    setup += ["/proc/oracle_gases()", f"\treturn list({gas_paths})", "/proc/oracle_reaction_ids()",
              "\treturn list(" + ", ".join(json.dumps(r["id"]) for r in data["reactions"]) + ")",
              "/proc/oracle_setup()", "\tvar/list/ids = list()", "\tvar/list/heats = list()"]
    for gas in data["gases"]:
        setup += [f'\tids[{gas["path"]}] = "{gas["id"]}"', f'\theats[{gas["path"]}] = {gas["specific_heat"]}']
    setup += ["\tGLOB.meta_gas_info = new /list(META_GAS_LENGTH)", "\tGLOB.meta_gas_info[META_GAS_ID] = ids",
              "\t/datum/gas_mixture::gas_meta = new /list(META_GAS_LENGTH)",
              "\t/datum/gas_mixture::gas_meta[META_GAS_SPECIFIC_HEAT] = heats", "\tSSair.gas_reactions = init_gas_reactions()"]
    write("setup.dm", "\n".join(setup) + "\n")
    mixture_blocks = catalog.blocks(catalog.read(catalog.MIXTURE))
    names = ["New", "proc/assert_gas", "proc/assert_gases", "proc/garbage_collect", "proc/heat_capacity",
             "turf/heat_capacity", "proc/total_moles", "proc/return_pressure", "proc/return_volume",
             "proc/adjust_gas", "proc/react"]
    write("mixture.dm", "\n".join(mixture_blocks["/datum/gas_mixture/" + name] for name in names))
    write("reactions.dm", catalog.read(catalog.REACTIONS))
    write("electrolyzer.dm", catalog.read(catalog.ELECTROLYZER))
    write("oracle.dm", catalog.read("tools/atmos/oracle.dm.in"))
    includes = ["definitions.dm", "oracle.dm", "setup.dm", "mixture.dm", "reactions.dm", "electrolyzer.dm"]
    write("oracle.dme", "\n".join(f'#include "{p}"' for p in includes) + "\n")


def cases(data):
    rng = random.Random(220)
    gas_index = {g["id"]: i for i, g in enumerate(data["gases"])}
    output = []

    def add(label, reaction, temperature, gases, **kwargs):
        case = dict(label=label, reaction=reaction, temperature=temperature, volume=2500,
                    turf_heat_capacity=False, gases=list(gases.items()), holder=0, bits=0,
                    working_power=1, supermatter_power=7000, seed_results=False, ticks=1)
        case.update(kwargs)
        output.append(case)

    def adjacent(number, step):
        # Next representable BYOND number, not Python's much smaller double step.
        bits = struct.unpack("I", struct.pack("f", number))[0]
        return struct.unpack("f", struct.pack("I", bits + step))[0]

    for reaction in data["reactions"]:
        rid = reaction["id"]
        reqs = reaction["requirements"]
        lower, upper = reqs.get("MIN_TEMP", 2.7), reqs.get("MAX_TEMP", 10000)
        temperature = (lower + upper) / 2 if "MAX_TEMP" in reqs else max(293.15, lower + 200)
        base = {gas_index[k]: max(v * 10, 30) for k, v in reqs.items() if k in gas_index}
        add(rid + ":baseline", rid, temperature, base)
        add(rid + ":twenty_ticks", rid, temperature, base, holder=3, bits=255, ticks=20)
        for holder in range(6):
            for bits in (0, 1, 2, 3, 4, 8, 16, 32, 64, 128, 255):
                add(f"{rid}:holder={holder},bits={bits}", rid, temperature, base, holder=holder, bits=bits)
        for threshold in (reqs.get("MIN_TEMP"), reqs.get("MAX_TEMP")):
            if threshold is not None:
                for t in (adjacent(threshold, -1), threshold, adjacent(threshold, 1)):
                    add(f"{rid}:temperature={t}", rid, t, base, holder=3, bits=255)
        for key, minimum in reqs.items():
            if key not in gas_index:
                continue
            for n in (0, adjacent(minimum, -1), minimum, adjacent(minimum, 1)):
                gases = {**base, gas_index[key]: n}
                add(f"{rid}:threshold={key}:{n}", rid, temperature, gases)
            missing = {k: v for k, v in base.items() if k != gas_index[key]}
            add(f"{rid}:missing={key}", rid, temperature, missing)
        for volume in (1, 100, 100000):
            add(f"{rid}:volume={volume}", rid, temperature, base, volume=volume, holder=3, bits=255)
        for power in (0, 0.1, 1, 4):
            for sm in (4999, 5000, 5000.1, 9000, 10000):
                add(f"{rid}:power={power},sm={sm}", rid, temperature, base, working_power=power, supermatter_power=sm)
        for i in range(40):
            t = rng.uniform(lower, upper)
            gases = {g: n * 10 ** rng.uniform(-0.8, 2) for g, n in base.items()}
            for _ in range(3):
                gases.setdefault(rng.randrange(len(gas_index)), rng.uniform(0, 100))
            pairs = list(gases.items())
            rng.shuffle(pairs)
            add(f"{rid}:random={i}", rid, t, dict(pairs), volume=10 ** rng.uniform(1, 5),
                holder=rng.randrange(6), bits=rng.randrange(256), turf_heat_capacity=i % 2 == 0)

    for t in (2.7, 15, 20, 20.01, 50, 200, 270, 300, 500, 1600, 6000, 60000):
        for i in range(25):
            gases = {g: rng.uniform(0.01, 500) for g in rng.sample(range(20), rng.randrange(1, 21))}
            if i % 3:
                gases.pop(gas_index["hypernoblium"], None)
            add(f"batch:{t}:{i}", "batch", t, gases, holder=i % 6, bits=255, turf_heat_capacity=i % 2 == 0,
                seed_results=True)
            add(f"batch:{t}:{i}:twenty_ticks", "batch", t, gases, holder=i % 6, bits=255,
                turf_heat_capacity=i % 2 == 0, seed_results=True, ticks=20)
    for ratio in (9.999, 10, 10.001, 95.999, 96, 96.001):
        for t in (373.15, 374, 1643.15, 1644):
            add(f"plasma:ratio={ratio},t={t}", "plasmafire", t, {3: 1, 0: ratio}, holder=3)
    for t in (20, 59.999, 60, 120, 120.001, 140, 159.999, 160, 200, 200.001, 283, 303.15):
        add(f"water:internal_boundary={t}", "vapor", t, {4: 1}, holder=3, bits=255)
        add(f"freon:internal_boundary={t}", "freonfire", t, {12: 1, 0: 10}, holder=3, bits=255)
    for bz in (0, 1, 1000, 1000000):
        add(f"nobformation:catalyst={bz}", "nobformation", 10, {1: 100, 8: 50, 9: bz})
    for label, gases, t in [
        ("empty", {}, 300), ("inert", {1: 10}, 300), ("zero_keys", {19: 0, 5: 0, 8: 0}, 300),
        ("suppression", {19: 1, 5: 5, 8: 10}, 300), ("suppression_boundary", {19: 1, 5: 5}, 20),
        ("anti_cleanup", {19: 1, 1: 0.005}, 300), ("vacuum_cleanup", {19: 0.25}, 300),
        ("turf_vacuum", {4: 0.25}, 190), ("gc_boundary", {19: 0.25, 1: 0.0001}, 300),
    ]:
        add("batch:" + label, "batch", t, gases, seed_results=True, turf_heat_capacity=True, holder=3, bits=255)
    return output


def command(args, cwd=WORK, input_text=None):
    result = subprocess.run([str(a) for a in args], cwd=cwd, input=input_text, text=True,
                            encoding="utf-8", errors="replace", capture_output=True, timeout=60)
    if result.returncode or "runtime error:" in result.stdout.lower() or "ORACLE_FAILURE" in result.stdout:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
    return result.stdout


def difference(expected, actual, path=""):
    if expected is None or actual is None:
        return None if expected is actual else f"{path}: {expected} != {actual}"
    if isinstance(expected, dict):
        for key in ("initial_heat", "flags", "keys", "temperature", "moles", "results", "effects"):
            diff = difference(expected[key], actual[key], key)
            if diff:
                return diff
        return None
    if isinstance(expected, list):
        if len(expected) != len(actual):
            return f"{path}: length {len(expected)} != {len(actual)}"
        for i, (a, b) in enumerate(zip(expected, actual)):
            diff = difference(a, b, f"{path}[{i}]")
            if diff:
                return diff
        return None
    if path == "flags" or path.startswith("keys") or (path.startswith("effects") and path.endswith("[0]")):
        return None if expected == actual else f"{path}: {expected} != {actual}"
    if isinstance(expected, str):
        expected = float(expected)
    if path == "initial_heat":
        return None if struct.pack("f", expected) == struct.pack("f", actual) else f"{path}: {expected} != {actual}"
    # The oracle exports nine significant digits, enough to round-trip BYOND f32.
    if math.isclose(expected, actual, rel_tol=2e-5, abs_tol=2e-5):
        return None
    return f"{path}: {expected} != {actual}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True, help="Directory containing dm and DreamDaemon")
    parser.add_argument("--rust", type=Path, default=ROOT / "rust/atmos/target/debug/chemistry.exe")
    args = parser.parse_args()
    command([os.sys.executable, ROOT / "tools/atmos/catalog.py", "--check"], cwd=ROOT)
    data = catalog.extract()
    harness(data)
    inputs = cases(data)
    write("cases.json", json.dumps(inputs, ensure_ascii=False))
    suffix = ".exe" if os.name == "nt" else ""
    print(command([args.byond / ("dm" + suffix), "oracle.dme"]))
    output_path = WORK / "dm-output.json"
    if output_path.exists():
        output_path.unlink()
    daemon = "dd.exe" if os.name == "nt" else "DreamDaemon"
    write("oracle.log", "")
    log = command([args.byond / daemon, "oracle.dmb", "-safe", "-close", "-invisible", "-logself"])
    log += (WORK / "oracle.log").read_text(encoding="utf-8-sig", errors="replace")
    if "runtime error:" in log.lower() or "ORACLE_FAILURE" in log:
        raise RuntimeError(f"DM runtime failure:\n{log[:5000]}")
    if not output_path.exists():
        raise RuntimeError(f"DM produced no results:\n{log}")
    reference = json.loads(output_path.read_text(encoding="utf-8-sig"))
    print("BYOND " + reference["version"])
    assert reference["version"].startswith("516."), "These gas_mixture helpers require BYOND 516"
    assert reference["order"] == [r["id"] for r in data["reactions"]], "DM subtype ordering changed"
    assert len(reference["cases"]) == len(inputs), "Incomplete DM run"
    indexes = {r["id"]: i for i, r in enumerate(data["reactions"])}
    lines = []
    for case in inputs:
        words = [indexes.get(case["reaction"], -1), case["ticks"], case["temperature"], case["volume"], int(case["turf_heat_capacity"]),
                 case["holder"], case["bits"], case["working_power"], case["supermatter_power"], int(case["seed_results"]), len(case["gases"])]
        words += [n for pair in case["gases"] for n in pair]
        lines.append(" ".join(str(n) for n in words))
    rust = [json.loads(line) for line in command([args.rust.resolve()], input_text="\n".join(lines) + "\n").splitlines()]
    write("rust-output.json", json.dumps(rust))
    assert len(rust) == len(inputs), "Incomplete Rust run"
    failures = []
    for case, dm, rs in zip(inputs, reference["cases"], rust):
        diff = difference(dm, rs)
        if diff:
            failures.append(dict(label=case["label"], difference=diff, input=case, dm=dm, rust=rs))
    write("failures.json", json.dumps(failures, ensure_ascii=False, indent=2))
    coverage = {r["id"]: 0 for r in data["reactions"]}
    for case, dm in zip(inputs, reference["cases"]):
        if case["reaction"] in coverage and (any(v is not None for v in dm["results"]) or dm["moles"] != [dict(case["gases"]).get(i, 0) for i in range(20)]):
            coverage[case["reaction"]] += 1
    assert all(coverage.values()), f"Missing positive reaction coverage: {coverage}"
    report = dict(cases=len(inputs), passed=len(inputs) - len(failures), failed=len(failures), byond=reference["version"],
                  positive_coverage=coverage, source_hashes=data["sources"])
    write("report.json", json.dumps(report, indent=2))
    print(f"{report['passed']}/{len(inputs)} matched; {len(failures)} differences")
    for failure in failures[:12]:
        print(f"  {failure['label']}: {failure['difference']}")
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
