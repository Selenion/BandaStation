"""Inventory gas-mixture entry points and candidate direct writes in checked-out DM.

Candidate writes are deliberately conservative: DM aliases and dynamic calls require
review of the enclosing proc, rather than a claim of complete static type analysis.
"""
import json
import re
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GROUPS = {
    "lifecycle": "New Destroy banda_touch banda_expose_pipe_port parse_string_immutable",
    "read": "heat_capacity heat_capacity_archive total_moles has_gas return_pressure return_temperature return_volume return_visuals thermal_energy compare get_breath_partial_pressure gas_pressure_minimum_transfer gas_pressure_calculate gas_pressure_quadratic gas_pressure_approximate to_string check_gases banda_pipe_pressure copy",
    "ensure": "assert_gas assert_gases",
    "reset_keys": "add_gas add_gases",
    "collect": "garbage_collect",
    "archive": "archive",
    "merge": "merge",
    "set": "set_gas set_temperature",
    "adjust": "adjust_gas adjust_multiple_gases",
    "convert": "convert_gas",
    "take": "remove remove_ratio remove_specific remove_specific_ratio",
    "replace": "copy_from copy_from_ratio",
    "exchange": "equalize share temperature_share",
    "react": "react electrolyze",
    "transfer": "pump_gas_to release_gas_to",
}
CATEGORIES = {name: group for group, names in GROUPS.items() for name in names.split()}
HEADER = re.compile(r"^(/datum/gas_mixture[^\n(]*)\(", re.M)
WRITE = re.compile(
    r"\b(?:moles(?:_archive)?|temperature(?:_archived)?|reaction_results|volume|last_share)"
    r"(?:\[[^\n]*?\])?\s*(?:[+*/-]?=(?!=)|\.Cut\()"
)
ACCESS = re.compile(r"\b(?:return_air|get_readonly_air|get_mutable_air|return_analyzable_air|remove_air|assume_air)\s*\(")
DECLARATION = re.compile(r"\b(?:var/)?datum/gas_mixture(?:/\w+)*")
AIR_FIELD = re.compile(r"\.(?:air|airs|air_contents|moles|moles_archive|reaction_results)\b")
MIXTURE_CALL = re.compile(r"\.(?:" + "|".join(re.escape(name) for name, group in CATEGORIES.items() if group != "lifecycle") + r")\s*\(")
VALUE_ACCESS = re.compile(r"\breturn_air_values\s*\(")


def inventory():
    methods, candidates, unclassified, consumers = [], [], [], []
    for directory in ("code", "modular_bandastation"):
        for path in sorted((ROOT / directory).rglob("*.dm")):
            name = path.relative_to(ROOT).as_posix()
            code = path.read_text(encoding="utf-8-sig")
            for match in HEADER.finditer(code):
                symbol = match[1]
                method = symbol.rsplit("/", 1)[-1]
                row = dict(path=name, line=code.count("\n", 0, match.start()) + 1,
                           symbol=symbol, category=CATEGORIES.get(method))
                methods.append(row)
                if row["category"] is None:
                    unclassified.append(row)
            proc = None
            for line_number, line in enumerate(code.splitlines(), 1):
                if line.startswith("/"):
                    proc = line.strip()
                if not line.lstrip().startswith("//") and WRITE.search(line):
                    candidates.append(dict(path=name, line=line_number, proc=proc, code=line.strip()))
                if line.lstrip().startswith("//"):
                    continue
                reasons = [label for label, pattern in (("air_api", ACCESS), ("typed_mixture", DECLARATION), ("air_field", AIR_FIELD), ("mixture_method_candidate", MIXTURE_CALL), ("value_api", VALUE_ACCESS)) if pattern.search(line)]
                if reasons:
                    domain = "tests" if "unit_tests" in path.parts else ("adapter" if name.startswith("modular_bandastation/atmos_rust/") else ("mixture_implementation" if proc and proc.startswith("/datum/gas_mixture") else "game"))
                    consumers.append(dict(path=name, line=line_number, proc=proc, code=line.strip(), reasons=reasons, domain=domain, review="unreviewed"))
    return dict(methods=methods, direct_write_candidates=candidates,
                unclassified=unclassified,
                categories=dict(Counter(row["category"] for row in methods)), consumer_candidates=consumers,
                consumer_domains=dict(Counter(row["domain"] for row in consumers)),
                coverage="Conservative textual inventory: includes unrelated air fields and same-named methods; dynamic aliases and every candidate require manual review.")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = inventory()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(dict(methods=len(result["methods"]),
                          direct_write_candidates=len(result["direct_write_candidates"]),
                          unclassified=result["unclassified"], categories=result["categories"],
                          consumer_candidates=len(result["consumer_candidates"]), consumer_domains=result["consumer_domains"]),
                     ensure_ascii=False))
    raise SystemExit(bool(result["unclassified"]))
