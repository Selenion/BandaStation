"""Validate experimental AoS/AVX2/SoA gas exchange against unmodified DM procedures."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess

import catalog
import transport

ROOT = catalog.ROOT
PRESSURE_ABSOLUTE_TOLERANCE_KPA = 0.1  # One millibar; only the share() return value may differ.


def exact_difference(a, b, path=""):
    if a is None or b is None:
        return None if a is b else f"{path}: {a} != {b}"
    if isinstance(a, dict):
        if set(a) != set(b):
            return f"{path}: fields differ"
        for key in a:
            difference = exact_difference(a[key], b[key], path + "." + key)
            if difference:
                return difference
    elif isinstance(a, list):
        if len(a) != len(b):
            return f"{path}: lengths differ"
        for index, (left, right) in enumerate(zip(a, b)):
            difference = exact_difference(left, right, f"{path}[{index}]")
            if difference:
                return difference
    elif struct.pack("<f", float(a)) != struct.pack("<f", float(b)):
        return f"{path}: {a} != {b}"
    return None


def checksum(value):
    # Hash ordered fields and f32 bits, not decimal JSON formatting.
    digest = hashlib.sha256()

    def visit(item):
        if item is None:
            digest.update(b"null")
        elif isinstance(item, dict):
            for key in sorted(item):
                digest.update(key.encode())
                visit(item[key])
        elif isinstance(item, list):
            digest.update(struct.pack("<I", len(item)))
            for child in item:
                visit(child)
        else:
            digest.update(struct.pack("<f", float(item)))
    visit(value)
    return digest.hexdigest()


def dm_difference(case, expected, actual):
    # The user accepts insignificant BYOND reduction rounding, not changes to gas/heat state.
    difference = exact_difference(expected["states"], actual["states"], "states")
    if difference:
        return difference
    if len(expected["values"]) != len(actual["values"]):
        return "Incomplete return values"
    for step, (dm, native) in enumerate(zip(expected["values"], actual["values"])):
        if case["mode"] != 0 or dm is None or native is None:
            difference = exact_difference(dm, native, f"values[{step}]")
            if difference:
                return difference
            continue
        dm = float(dm)
        if (dm > 0) != (native > 0) or (dm < 0) != (native < 0):
            return f"values[{step}]: pressure direction changed"
        if not math.isclose(dm, native, rel_tol=0, abs_tol=PRESSURE_ABSOLUTE_TOLERANCE_KPA):
            return f"values[{step}]: pressure error {abs(dm-native)} kPa"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--binary", type=Path, required=True, help="Compiled atmos_bench executable")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid capture name")
    out = ROOT / "tmp/atmos-simd" / args.name
    if out.exists():
        parser.error("Use a new name to preserve results")
    out.mkdir(parents=True)
    transport.WORK = out / "dm"
    transport.harness(catalog.extract())
    fixture = transport.WORK / "transport.dm"
    source = fixture.read_text(encoding="utf-8")
    marker = '\t\t\ta.archive()'
    assert source.count(marker) == 1
    source = source.replace(marker, '''\t\t\tif(case["drive"] && ((tick - 1) % 32 == 0))
\t\t\t\ta.moles[/datum/gas/oxygen] = 100 + ((tick - 1) % 31)
\t\t\t\tb.moles[/datum/gas/plasma] = 20 + ((tick - 1) % 13)
\t\t\t\ta.temperature = case["a"]["temperature"]
\t\t\t\tb.temperature = case["b"]["temperature"]
''' + marker)
    fixture.write_text(source, encoding="utf-8")
    named, generated = transport.cases()
    long = dict(next(case for case in named if case["label"] == "all_gases"))
    long.update(label="driven_10000_ticks", ticks=10_000, drive=True)
    inputs = named + generated + [long]
    expected = transport.run_reference(args.byond.resolve(), inputs)
    logs = "\n".join(path.read_text(encoding="utf-8-sig", errors="replace") for path in transport.WORK.glob("*.log"))
    if any(marker in logs.lower() for marker in ("runtime in ", "runtime error:", "infinite loop")):
        raise RuntimeError("BYOND oracle runtime; capture is not valid")
    input_path, output_path = out / "input.json", out / "rust-output.json"
    input_path.write_text(json.dumps(inputs), encoding="utf-8")
    subprocess.run([str(args.binary.resolve()), "--verify", str(input_path), str(output_path)], check=True, cwd=ROOT, timeout=300)
    actual = json.loads(output_path.read_text(encoding="utf-8"))
    failures = []
    checksums = {}
    pressure_rounding = dict(different_returns=0, max_absolute_kpa=0.0)
    for mode in ("aos", "simd", "soa", "production"):
        observed = actual[mode]
        if observed is None:
            failures.append(dict(mode=mode, difference="AVX2 unavailable; SIMD was not validated"))
            continue
        assert len(observed) == len(inputs), "Incomplete variant output"
        checksums[mode] = checksum(observed)
        for case, dm, native in zip(inputs, expected["cases"], observed):
            if mode == "aos" and case["mode"] == 0:
                for step, (left, right) in enumerate(zip(dm["values"], native["values"])):
                    if left is None or right is None:
                        continue
                    error = abs(float(left) - right)
                    if struct.pack("<f", float(left)) != struct.pack("<f", right):
                        pressure_rounding["different_returns"] += 1
                    if error > pressure_rounding["max_absolute_kpa"]:
                        pressure_rounding.update(max_absolute_kpa=error, case=case["label"], step=step, dm=left, rust=right)
            difference = dm_difference(case, dm, native)
            if difference:
                failures.append(dict(mode=mode, label=case["label"], difference=difference))
        if mode != "aos":
            difference = exact_difference(actual["aos"], observed)
            if difference:
                failures.append(dict(mode=mode, difference=difference, contract="Rust variants must match bitwise"))
    layout = json.loads(subprocess.check_output([str(args.binary.resolve()), "--layout"], text=True, cwd=ROOT))
    sources = (ROOT / "rust/atmos/benches").rglob("*.rs")
    report = dict(byond=expected["version"], cases=len(inputs), driven_ticks=10_000,
                  dm_policy="Exact f32 gas/temperature/archive state and gas key order; only share() pressure return allows abs<=0.1 kPa (1 mbar), with identical sign and nullability; user accepts insignificant rounding",
                  rust_policy="Exact f32 bits, ordered gas keys and nullable results", checksums=checksums, layout=layout,
                  pressure_rounding=pressure_rounding,
                  production_simd_compiled=actual["production_simd_compiled"],
                  binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  source_sha256={path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest() for path in sources},
                  failures=failures)
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"SIMD layouts: {len(inputs)} DM scenarios, 10000 driven ticks; {len(failures)} failures")
    for failure in failures[:12]:
        print(failure)
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
