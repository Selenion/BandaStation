"""Check native cell calculations/commands against the pinned real Banda world reference."""
import argparse
import json
from pathlib import Path

import catalog
import compare
import transport


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", type=Path, default=catalog.ROOT / "rust/atmos/target/debug/cell.exe")
    args = parser.parse_args()
    reference = json.loads((catalog.ROOT / "tools/atmos/world-reference.json").read_text(encoding="utf-8"))
    metadata = catalog.extract()
    gas_indexes = {g["id"]: i for i, g in enumerate(metadata["gases"])}
    reaction_indexes = {r["path"]: i for i, r in enumerate(metadata["reactions"])}
    inputs = reference["input"]
    lines = []
    for case in inputs:
        words = [int(case["operation"] == "hotspot"), case["temperature"], len(case["gases"])]
        words += [number for gas, amount in case["gases"] for number in (gas_indexes[gas], amount)]
        lines.append(" ".join(map(str, words)))
    outputs = [json.loads(line) for line in compare.command([args.rust], cwd=catalog.ROOT,
               input_text="\n".join(lines) + "\n").splitlines()]
    assert len(outputs) == len(inputs), "Incomplete native capture"
    failures = []
    for case, expected, actual in zip(inputs, reference["observed"]["cases"], outputs):
        assert expected["id"] == case["id"]
        projected = dict(flags=expected["flags"], temperature=expected["temperature"],
                         moles=[0] * len(gas_indexes), keys=[], results=[None] * len(reaction_indexes),
                         hotspot=bool(expected["hotspot"]), hotspot_temperature=expected["hotspot_temperature"],
                         hotspot_volume=expected["hotspot_volume"])
        for gas, amount in expected["gases"]:
            projected["moles"][gas_indexes[gas]] = amount
            projected["keys"].append(gas_indexes[gas])
        for reaction, result in expected["results"]:
            projected["results"][reaction_indexes[reaction]] = result
        numerical = {k: v for k, v in actual.items() if k != "actions"}
        diff = transport.difference(projected, numerical)
        actions = actual["actions"]
        exposure = [a for a in actions if a[0] == 9]
        if not diff and len(exposure) != expected["fire_calls"]:
            diff = "Different fire_act call count"
        if not diff:
            for command, observation in zip(exposure, expected["fire_observations"]):
                diff = transport.difference([observation["oxygen"], observation["temperature"]], command[3:])
                if diff:
                    break
        if not diff and expected["item_frozen"] and not any(a[0] == 1 for a in actions):
            diff = "Missing Freeze command for the observed frozen item"
        if not diff and expected["wet_strength"] == 1 and not any(a[0] == 2 for a in actions):
            diff = "Missing Wet command for the observed wet floor"
        if diff:
            failures.append(dict(id=case["id"], difference=diff, expected=expected, actual=actual))
    output = catalog.ROOT / "tmp/atmos-cell"
    output.mkdir(parents=True, exist_ok=True)
    (output / "failures.json").write_text(json.dumps(failures, ensure_ascii=False, indent="\t"), encoding="utf-8")
    print(f"Cell physics/commands: {len(inputs)-len(failures)}/{len(inputs)} match real DM captures")
    for failure in failures:
        print(f"  {failure['id']}: {failure['difference']}")
    print("Actual execution of the commands by a Rust-connected DM server is a separate integration check.")
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
