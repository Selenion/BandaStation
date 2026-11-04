"""Rank native BYOND proc costs without summing inclusive parent/child times."""

import argparse
from datetime import datetime
import json
from pathlib import Path


def process_summary(folder, metadata):
    samples_path = folder / "process-samples.json"
    if not samples_path.exists():
        return None
    samples = json.loads(samples_path.read_text(encoding="utf-8-sig"))
    lifetime_peaks = {
        name: max(sample[key] for sample in samples if key in sample) / 2**20
        for name, key in (("observed_lifetime_peak_working_set_mib", "os_peak_working_set_bytes"),
                          ("observed_lifetime_peak_commit_mib", "os_peak_commit_bytes"))
        if any(key in sample for sample in samples)
    }
    start, end = metadata.get("sample_start_unix"), metadata.get("sample_end_unix")
    if start is None or end is None:
        # Compatibility with the first capture, which recorded bounds in its log.
        rows = [json.loads(line) for line in (folder / "data/logs/local-profile/runtime.log.json").read_text(encoding="utf-8-sig").splitlines()]
        marks = [row for row in rows if "LOCAL_PROFILE_SAMPLE_" in row.get("msg", "")]
        start, end = [row["w-state"]["timestamp"] for row in marks]
    samples = [s for s in samples if float(start) <= datetime.fromisoformat(s["utc"].replace("Z", "+00:00")).timestamp() <= float(end)]
    if len(samples) < 2:
        return None
    elapsed = (datetime.fromisoformat(samples[-1]["utc"]) - datetime.fromisoformat(samples[0]["utc"])).total_seconds()
    cpu_seconds = samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]
    result = dict(samples=len(samples), sampled_wall_seconds=elapsed,
                cpu_seconds=cpu_seconds, cpu_one_core_percent=100 * cpu_seconds / elapsed,
                working_set_mib=[min(s["working_set_bytes"] for s in samples) / 2**20, max(s["working_set_bytes"] for s in samples) / 2**20],
                private_mib=[min(s["private_bytes"] for s in samples) / 2**20, max(s["private_bytes"] for s in samples) / 2**20])
    # OS peak counters retain startup peaks between polls; the final sub-second tail may be missed.
    result.update(lifetime_peaks)
    if all("main_thread_cpu_seconds" in s for s in samples) and len({s["main_thread_id"] for s in samples}) == 1:
        result["main_thread_id"] = samples[0]["main_thread_id"]
        result["main_thread_cpu_seconds"] = samples[-1]["main_thread_cpu_seconds"] - samples[0]["main_thread_cpu_seconds"]
        result["main_thread_cpu_one_core_percent"] = 100 * result["main_thread_cpu_seconds"] / elapsed
        # This residual includes every other thread, not just the Rust worker.
        result["other_threads_cpu_seconds"] = cpu_seconds - result["main_thread_cpu_seconds"]
        result["other_threads_cpu_one_core_percent"] = 100 * result["other_threads_cpu_seconds"] / elapsed
    return result


def summarize(folder):
    metadata = json.loads((folder / "profile-metadata.json").read_text(encoding="utf-8-sig"))
    procs = json.loads((folder / "profile-procs.json").read_text(encoding="utf-8-sig"))
    total_self = sum(row["self"] for row in procs)
    process = process_summary(folder, metadata)
    lines = ["# Исходный профиль DM", "",
             f'Сценарий: {metadata["scenario"]}; карта: {metadata["map"]}; BYOND {metadata["byond"]}; клиентов: {metadata["connected_clients"]}.',
             f'Окно: {metadata["sample_game_seconds"]} игровых / {metadata["sample_wall_seconds"]} реальных секунд.', "",
             "Доля self считается среди профилируемых DM-процедур, это не доля CPU всего процесса.",
             "Включённое время parent/child не суммируется. Внутренний native-код, сеть и SendMaps могут быть вне этого знаменателя.",
             "", "## Процедуры по собственному времени", "", "| Процедура | Self, % | Вызовов |", "| --- | ---: | ---: |"]
    for row in sorted(procs, key=lambda row: row["self"], reverse=True)[:25]:
        share = row["self"] / total_self * 100 if total_self else 0
        lines.append(f'| `{row["name"]}` | {share:.2f} | {row["calls"]} |')
    lines += ["", "## Подсистемы: сглаженная стоимость одного выполнения", "",
              "Это состояние MC в конце окна, а не суммарное время за весь замер.", "",
              "| Подсистема | Cost, ms | Завершённых выполнений в окне |", "| --- | ---: | ---: |"]
    for ss in sorted(metadata["subsystems"], key=lambda row: row["cost_ms_rolling"], reverse=True)[:20]:
        lines.append(f'| {ss["name"]} | {ss["cost_ms_rolling"]:.3f} | {ss["times_fired"]} |')
    lines += ["", "## Атмосферика", "", "```json", json.dumps(metadata["atmos"], indent="\t"), "```", "",
              "Ограничения: станция без игроков; нагрузка 50+ игроков и SendMaps не воспроизведена.",
              "Это исходная точка сравнения постоянных расходов; для выбора оптимизации нужны нагрузочные сценарии и профиль живого раунда.", ""]
    if process:
        lines += ["## Процесс во время окна сбора", "",
                  f'CPU: {process["cpu_one_core_percent"]:.2f}% одного логического ядра, по {process["samples"]} отсчётам.',
                  f'За интервал между этими отсчётами: {process["cpu_seconds"]:.3f} CPU-секунд за {process["sampled_wall_seconds"]:.3f} реальных секунд.',
                  f'Working set: {process["working_set_mib"][0]:.1f}–{process["working_set_mib"][1]:.1f} MiB.',
                  f'Private memory: {process["private_mib"][0]:.1f}–{process["private_mib"][1]:.1f} MiB.', "",
                  "Working set и private memory не складываются. Инициализация и выгрузка мира исключены.", ""]
        if "observed_lifetime_peak_commit_mib" in process:
            lines += [f'Наблюдаемый OS peak commit с загрузкой: {process["observed_lifetime_peak_commit_mib"]:.1f} MiB.',
                      f'Наблюдаемый OS peak working set с загрузкой: {process["observed_lifetime_peak_working_set_mib"]:.1f} MiB.',
                      "Пики ОС сохраняют всплески между опросами. Последний участок перед выходом процесса может быть пропущен.", ""]
        if "main_thread_cpu_one_core_percent" in process:
            lines += [f'CPU главного потока: {process["main_thread_cpu_one_core_percent"]:.2f}% ядра (TID {process["main_thread_id"]}).',
                      f'CPU всех остальных потоков вместе: {process["other_threads_cpu_one_core_percent"]:.2f}% ядра; это не отдельный счётчик Rust-worker.', ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    args = parser.parse_args()
    report = summarize(args.capture)
    (args.capture / "SUMMARY.md").write_text(report, encoding="utf-8", newline="\n")
    print(report)


if __name__ == "__main__":
    main()
