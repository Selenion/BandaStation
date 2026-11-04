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
    start_count = metadata.get("atmos_start", {}).get("native_completed_ticks")
    end_count = metadata["atmos"].get("native_completed_ticks")
    completed = end_count - start_count if start_count is not None and end_count is not None else None
    count = completed if completed is not None else air["times_fired"]
    wall = metadata["sample_wall_seconds"]
    normalized = dict(completed_ticks=count, completed_ticks_per_real_second=count / wall,
                      real_ms_per_completed_tick=1000 * wall / count if count else None,
                      scope="Whole-server CPU divided by completed atmosphere ticks, not isolated atmosphere CPU or latency",
                      estimate="Average CPU between in-window OS samples scaled to the full metadata window; boundary samples are missing")
    if process:
        for label, key in (("process", "cpu_one_core_percent"), ("main_thread", "main_thread_cpu_one_core_percent"),
                           ("other_threads", "other_threads_cpu_one_core_percent")):
            if key in process:
                seconds = process[key] / 100 * wall
                normalized[f"estimated_{label}_cpu_seconds"] = seconds
                normalized[f"estimated_{label}_cpu_ms_per_completed_tick"] = 1000 * seconds / count if count else None
    worker = {}
    start = metadata.get("atmos_start", {})
    end = metadata["atmos"]
    for label, key in (("cpu_seconds", "native_worker_cpu_seconds"), ("jobs", "native_worker_jobs"), ("mcycles", "native_worker_mcycles")):
        if label == "cpu_seconds" and not end.get("native_worker_clock_available"):
            continue
        if key in start and key in end and start[key] >= 0 and end[key] >= 0:
            worker[label] = end[key] - start[key]
    if "cpu_seconds" in worker:
        worker["cpu_one_core_percent"] = 100 * worker["cpu_seconds"] / wall
        worker["cpu_ms_per_job"] = 1000 * worker["cpu_seconds"] / worker["jobs"] if worker.get("jobs") else None
        worker["scope"] = "Completed numerical jobs through staging, before Byond_Return; jobs may cross window bounds; Windows thread-time counters can be coarse"
    worker["simd_available"] = end.get("native_simd_available")
    return dict(process=process, native_completed_ticks=completed, sample_game_seconds=metadata["sample_game_seconds"],
                sample_wall_seconds=metadata["sample_wall_seconds"], ssair=air,
                atmos_start=metadata.get("atmos_start"), atmos_end=metadata["atmos"], normalized=normalized, worker=worker)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pair", nargs=2, action="append", type=Path, required=True, metavar=("DM", "RUST"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.output.with_suffix(".json").exists():
        parser.error("Use a new output path to preserve the previous comparison")
    rows = []
    lines = ["# Парные локальные профили DM/Rust", "", "CPU — процент одного ядра; память — private, не сумма с working set.", "",
             "| Сценарий / режим Rust | CPU DM / Rust, % | Главный поток DM / Rust, % | Private DM / Rust, MiB, максимум | SSair DM / Rust, ms* | Проходы MC DM / Rust | Завершённые native-расчёты | Реальное окно DM / Rust, s |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for dm_folder, rust_folder in args.pair:
        dm, rust = read(dm_folder), read(rust_folder)
        assert not dm["rust_enabled"] and rust["rust_enabled"], "Wrong backend labels"
        assert not dm.get("rust_threaded", False), "DM cannot run a Rust worker"
        for key in ("scenario", "byond", "map", "startup_seed", "seeded_turfs"):
            assert dm[key] == rust[key], f"Different {key}"
        manifests = [json.loads((folder / "manifest.json").read_text(encoding="utf-8-sig")) for folder in (dm_folder, rust_folder)]
        for folder, manifest, metadata in zip((dm_folder, rust_folder), manifests, (dm, rust)):
            assert not any(key in manifest for key in ("diagnostic_only_reason", "diagnostic_instrumentation")), f"Diagnostic capture cannot establish performance: {folder}"
            # run.py adds these only after the complete runtime log passes validation.
            assert manifest.get("metadata") == metadata and manifest.get("binary_sha256"), f"Capture was not accepted: {folder}"
            assert manifest.get("rust_threaded", False) == metadata.get("rust_threaded", False), f"Wrong worker mode: {folder}"
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
        if rust.get("rust_threaded", False):
            assert b["native_completed_ticks"] is not None, "Worker requires a completed-calculation counter"
        mode = "worker" if rust.get("rust_threaded", False) else "sync"
        row = dict(scenario=dm["scenario"], rust_mode=mode, byond=dm["byond"], map=dm["map"], startup_seed=dm["startup_seed"],
                   dm_folder=str(dm_folder), rust_folder=str(rust_folder), dm=a, rust=b)
        rows.append(row)
        main_cpu = [f'{m["process"]["main_thread_cpu_one_core_percent"]:.2f}'
                    if "main_thread_cpu_one_core_percent" in m["process"] else "—" for m in (a, b)]
        lines.append(f'| {dm["scenario"]} / {mode} | {a["process"]["cpu_one_core_percent"]:.2f} / {b["process"]["cpu_one_core_percent"]:.2f} | '
                     f'{main_cpu[0]} / {main_cpu[1]} | '
                     f'{a["process"]["private_mib"][1]:.1f} / {b["process"]["private_mib"][1]:.1f} | '
                     f'{a["ssair"]["cost_ms_rolling"]:.2f} / {b["ssair"]["cost_ms_rolling"]:.2f} | '
                     f'{a["ssair"]["times_fired"]} / {b["ssair"]["times_fired"]} | '
                     f'{b["native_completed_ticks"] if b["native_completed_ticks"] is not None else "—"} | '
                     f'{a["sample_wall_seconds"]:.2f} / {b["sample_wall_seconds"]:.2f} |')
    lines += ["", "* SSair cost — сглаженная оценка в конце окна, не сумма CPU за окно.", "",
              "## CPU за одинаковое игровое время и на завершённый расчёт", "",
              "CPU-секунда — занятость одного логического ядра в течение секунды. Общий CPU равен сумме главного и всех остальных потоков.",
              "CPU-секунды полного окна оценены из средней нагрузки внутри него: CPU% × реальная длительность / 100. Отсчёты ОС раз в секунду не совпадают с границами окна.",
              "CPU на расчёт — затраты всего сервера / число завершённых тиков атмоса; это не чистая стоимость или длительность атмоса. Остальные потоки включают worker, но не измеряют его отдельно.", "",
              "| Сценарий / режим | Расчётов за реальную секунду | Оценка CPU-секунд за игровое окно: всего / главный / остальные | Оценка CPU-ms всего сервера на расчёт |",
              "| --- | ---: | ---: | ---: |"]
    for row in rows:
        for label in ("dm", "rust"):
            data = row[label]["normalized"]
            cpu = [f'{data[f"estimated_{part}_cpu_seconds"]:.2f}' if f"estimated_{part}_cpu_seconds" in data else "—"
                   for part in ("process", "main_thread", "other_threads")]
            unit = data["estimated_process_cpu_ms_per_completed_tick"]
            unit_text = f"{unit:.1f}" if unit is not None else "—"
            mode = "DM" if label == "dm" else row["rust_mode"]
            lines.append(f'| {row["scenario"]} / {mode} | {data["completed_ticks_per_real_second"]:.3f} | '
                         f'{" / ".join(cpu)} | {unit_text} |')
    lines += ["",
              "По одному прогону; без клиентов, фоновые события/RNG могут различаться. Это предварительное сравнение, не модель 50+ игроков.",
              "Результат нельзя считать улучшением, если одновременно упало число выполненных расчётов. У worker fire MC и завершённый native-расчёт — разные события."]
    args.output.with_suffix(".json").write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    args.output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Compared {len(rows)} pairs: {args.output}")


if __name__ == "__main__":
    main()
