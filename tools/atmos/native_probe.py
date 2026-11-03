"""Fail early if a staged native library lacks the BYOND feature/entry points."""
import json
import os
from pathlib import Path
import subprocess


def probe(byond, output):
    dme = output / "native-probe.dme"
    dme.write_bytes(Path(__file__).with_suffix(".dm.in").read_bytes())
    compiler = byond / ("dm.exe" if os.name == "nt" else "DreamMaker")
    daemon = byond / ("dd.exe" if os.name == "nt" else "DreamDaemon")
    with (output / "native-probe.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(compiler), str(dme)], stdout=log, stderr=subprocess.STDOUT, check=True, timeout=20)
        subprocess.run([str(daemon), "native-probe.dmb", "-trusted", "-close", "-invisible", "-ip", "127.0.0.1"],
                       cwd=output, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=20)
    result = output / "native-probe.json"
    if not result.exists():
        raise SystemExit(f"DLL failed BYOND probe; build with --features byond. Inspect {output / 'native-probe.log'}")
    data = json.loads(result.read_text(encoding="utf-8-sig"))
    assert len(data) == 4, "Invalid native ABI response"
    if data != [11, 20, 26, 6653311]:
        raise SystemExit(f"DLL protocol/catalog is stale: {data}; rebuild with --features byond")
