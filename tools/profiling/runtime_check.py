"""Keep reviewed baseline errors visible without rejecting unrelated profiling work."""

from collections import Counter
import json
import re


# Exact signatures, not file-wide exclusions. These failures were reproduced without Rust.
# Baseline revision a72025a0480566585ab7cc48a62873581fe3a6a5; the guard run only
# patches the separately approved missing excited-group timer check.
KNOWN_RUNTIMES = (
    dict(id="painting_zero_integrity", error="The painting frame taking damage while having <= 0 integrity",
         proc="/atom/proc/take_damage", source="code/game/atom/atom_defense.dm", context="",
         evidence="tmp/profiling/avx2-01-atmos-flood-dm/data/logs/local-profile/runtime.log.json"),
    dict(id="large_painting_zero_integrity", error="The large painting frame taking damage while having <= 0 integrity",
         proc="/atom/proc/take_damage", source="code/game/atom/atom_defense.dm", context="",
         evidence="tmp/profiling/avx2-01-atmos-flood-dm/data/logs/local-profile/runtime.log.json"),
    dict(id="grenade_zero_integrity", error="The frag grenade taking damage while having <= 0 integrity",
         proc="/atom/proc/take_damage", source="code/game/atom/atom_defense.dm", context="",
         evidence="tmp/profiling/compare-dm-sm-guard-01/data/logs/local-profile/runtime.log.json"),
    dict(id="sm_demoralize_missing_location", error="Cannot read null.z",
         proc="/proc/is_valid_z_level", source="code/__HELPERS/levels.dm",
         context="(/datum/sm_delam/explosive): effect demoralize(",
         evidence="tmp/profiling/publish2-dm-01-sm-plasma/data/logs/local-profile/runtime.log.json"),
    dict(id="holodeck_power_channels", error="list index out of bounds",
         proc="/area/proc/removeStaticPower", source="code/game/area/areas.dm",
         context="(/area/station/holodeck/rec_center)",
         evidence="tmp/profiling/publish2-dm-01-sm-plasma/data/logs/local-profile/runtime.log.json"),
    dict(id="corpse_organ_throw_after_delete", error="Qdeleted thing being thrown around.",
         proc="/atom/movable/proc/throw_at", source="code/game/atoms_movable.dm",
         context="(/datum/corpse_damage/post_mortem/organ_loss): apply to body(",
         evidence="tmp/profiling/final-03-dm-atmos-fire/data/logs/local-profile/runtime.log.json"),
    dict(id="tail_pin_preimpact_missing_target", error="Cannot read null._listen_lookup",
         proc="/atom/movable/proc/pre_impact", source="code/game/atoms_movable.dm",
         context="src: the tail pin (/obj/item/tail_pin)",
         evidence="tmp/profiling/final-03-dm-guard-sm-plasma/data/logs/local-profile/runtime.log.json"),
    dict(id="tail_pin_impact_missing_target", error="Cannot execute null.hitby().",
         proc="/atom/movable/proc/throw_impact", source="code/game/atoms_movable.dm",
         context="src: the tail pin (/obj/item/tail_pin)",
         evidence="tmp/profiling/final-03-dm-guard-sm-plasma/data/logs/local-profile/runtime.log.json"),
)

MARKER = re.compile(r"runtime error:|runtime in |infinite loop", re.IGNORECASE)


def runtime_blocks(text):
    matches = list(MARKER.finditer(text))
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        yield text[match.start():end]


def signature(block):
    message = block.splitlines()[0].strip()
    # DreamDaemon, the JSON logger and Logis use different prefixes for the same event.
    prefix = r"^[ \t]*(?:- )?[ \t]*"
    proc = re.search(prefix + r"proc name: [^\n]*\((/[^()\n]+)\)[ \t]*$", block, re.MULTILINE)
    source = re.search(prefix + r"source file: ([^,\n]+),\d+[ \t]*$", block, re.MULTILINE)
    native = re.fullmatch(r"Runtime in ([^,\n]+),\d+: (.+)", message)
    if native:
        message = "runtime error: " + native[2]
    source_path = native[1] if native else source[1] if source else None
    return message, proc[1] if proc else None, source_path.replace("\\", "/") if source_path else None


def classify(block):
    message, proc, source = signature(block)
    for rule in KNOWN_RUNTIMES:
        if (message == "runtime error: " + rule["error"] and proc == rule["proc"]
                and source == rule["source"] and rule["context"] in block):
            return rule
    return None


def validate_runtime_logs(folder):
    entries = {}
    for path in sorted(folder.rglob("*")):
        if not path.is_file() or not (path.suffix == ".log" or path.name.endswith(".log.json")):
            continue
        text = path.read_text(encoding="utf-8-sig", errors="replace")
        messages = [text]
        if path.name.endswith(".log.json"):
            messages = []
            for line in text.splitlines():
                try:
                    row = json.loads(line)
                    if isinstance(row, dict) and isinstance(row.get("msg"), str):
                        messages.append(row["msg"])
                        messages.append(json.dumps({k: v for k, v in row.items() if k != "msg"}))
                    else:
                        messages.append(line)
                except json.JSONDecodeError:
                    # A truncated runtime record must not become an accepted known error.
                    messages.append(line)
        counts = Counter()
        for message in messages:
            for block in runtime_blocks(message or ""):
                rule = classify(block)
                key = (rule["id"] if rule else None, *signature(block))
                counts[key] += 1
                if key not in entries:
                    entries[key] = dict(id=key[0], message=key[1], proc=key[2], source=key[3],
                                        evidence=rule["evidence"] if rule else None, logs={})
        for key, count in counts.items():
            entries[key]["logs"][path.relative_to(folder).as_posix()] = count
    known, unexpected = [], []
    for entry in entries.values():
        # BYOND mirrors events into several logs; summing them would inflate counts.
        entry["count"] = max(entry["logs"].values())
        (known if entry["id"] else unexpected).append(entry)
    return dict(status="failed" if unexpected else "accepted_with_known_runtimes" if known else "clean",
                count_method="Maximum occurrences per signature in any one log; mirrored logs are not added",
                known_count=sum(e["count"] for e in known), unexpected_count=sum(e["count"] for e in unexpected),
                known=known, unexpected=unexpected)
