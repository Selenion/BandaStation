"""Compare completed DM/Rust captures without adding inclusive profiler times."""
import argparse
import json
from pathlib import Path

from summarize import process_summary


def read(folder):
    return json.loads((folder / "profile-metadata.json").read_text(encoding="utf-8-sig"))


def metrics(folder, metadata):
    process = process_summary(folder, metadata)
    air = next(s for s in metadata["subsystems"] if s["type"] == "/datum/controller/subsystem/air")
    return dict(process=process, sample_game_seconds=metadata["sample_game_seconds"],
                sample_wall_seconds=metadata["sample_wall_seconds"], ssair=air,
                atmos_start=metadata.get("atmos_start"), atmos_end=metadata["atmos"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pair", nargs=2, action="append", type=Path, required=True, metavar=("DM", "RUST"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.output.with_suffix(".json").exists():
        parser.error("Use a new output path to preserve the previous comparison")
    rows = []
    lines = ["# Парные локальные профили DM/Rust", "", "CPU — процент одного ядра; память — private, не сумма с working set.", "",
             "| Сценарий | CPU DM / Rust, % | Private DM / Rust, MiB, максимум | SSair DM / Rust, ms* | Проходы DM / Rust | Реальное окно DM / Rust, s |",
             "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for dm_folder, rust_folder in args.pair:
        dm, rust = read(dm_folder), read(rust_folder)
        assert not dm["rust_enabled"] and rust["rust_enabled"], "Wrong backend labels"
        for key in ("scenario", "byond", "map", "startup_seed", "seeded_turfs"):
            assert dm[key] == rust[key], f"Different {key}"
        manifests = [json.loads((folder / "manifest.json").read_text(encoding="utf-8-sig")) for folder in (dm_folder, rust_folder)]
        for folder, manifest, metadata in zip((dm_folder, rust_folder), manifests, (dm, rust)):
            assert not any(key in manifest for key in ("diagnostic_only_reason", "diagnostic_instrumentation")), f"Diagnostic capture cannot establish performance: {folder}"
            # run.py adds these only after the complete runtime log passes validation.
            assert manifest.get("metadata") == metadata and manifest.get("binary_sha256"), f"Capture was not accepted: {folder}"
        for key in ("map", "warmup_seconds", "sample_seconds", "harness_sha256", "platform", "logical_cpus"):
            assert manifests[0][key] == manifests[1][key], f"Different capture setting: {key}"
        shared_libraries = manifests[0]["dll_sha256"].keys() & manifests[1]["dll_sha256"].keys()
        for library in shared_libraries - {"bandastation_atmos.dll"}:
            assert manifests[0]["dll_sha256"][library] == manifests[1]["dll_sha256"][library], f"Different runtime library: {library}"
        if dm["scenario"] == "sm-plasma":
            for key in ("seeded_turfs", "center", "plasma_moles_per_turf", "temperature", "internal_energy", "gas_heat_power_generation", "strategy", "explosion_radius"):
                assert dm["incident"][key] == rust["incident"][key], f"Different SM input: {key}"
            assert dm["incident"]["sm_deleted"] and rust["incident"]["sm_deleted"], "Delamination did not complete"
        a, b = metrics(dm_folder, dm), metrics(rust_folder, rust)
        assert a["process"] and b["process"], "Missing process samples"
        row = dict(scenario=dm["scenario"], byond=dm["byond"], map=dm["map"], startup_seed=dm["startup_seed"],
                   dm_folder=str(dm_folder), rust_folder=str(rust_folder), dm=a, rust=b)
        rows.append(row)
        lines.append(f'| {dm["scenario"]} | {a["process"]["cpu_one_core_percent"]:.2f} / {b["process"]["cpu_one_core_percent"]:.2f} | '
                     f'{a["process"]["private_mib"][1]:.1f} / {b["process"]["private_mib"][1]:.1f} | '
                     f'{a["ssair"]["cost_ms_rolling"]:.2f} / {b["ssair"]["cost_ms_rolling"]:.2f} | '
                     f'{a["ssair"]["times_fired"]} / {b["ssair"]["times_fired"]} | '
                     f'{a["sample_wall_seconds"]:.2f} / {b["sample_wall_seconds"]:.2f} |')
    lines += ["", "* SSair cost — сглаженная оценка в конце окна, не сумма CPU за окно.", "",
              "По одному прогону; без клиентов, фоновые события/RNG могут различаться. Это предварительное сравнение, не модель 50+ игроков.",
              "Результат нельзя считать улучшением, если одновременно упало число выполненных проходов."]
    args.output.with_suffix(".json").write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    args.output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Compared {len(rows)} pairs: {args.output}")


if __name__ == "__main__":
    main()
