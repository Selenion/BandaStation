"""Materialize untouched HEAD for a DM baseline without changing the working tree or Git metadata."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--ref", default="HEAD")
    args = parser.parse_args()
    if not args.name.replace("-", "").replace("_", "").isalnum():
        parser.error("Invalid name")
    target = ROOT / "tmp/profiling-sources" / args.name
    if target.exists():
        parser.error("Use a new name to preserve the source snapshot")
    revision = subprocess.check_output(["git", "rev-parse", "--verify", args.ref + "^{commit}"], cwd=ROOT, text=True).strip()
    changed = set(subprocess.check_output(["git", "diff", "--name-only", "-z", revision], cwd=ROOT).split(b"\0"))
    entries = subprocess.check_output(["git", "ls-tree", "-r", "-z", revision], cwd=ROOT).split(b"\0")
    count = 0
    for entry in filter(None, entries):
        metadata, raw_path = entry.split(b"\t", 1)
        mode, kind, blob = metadata.split()
        if kind != b"blob" or mode not in (b"100644", b"100755"):
            raise ValueError(f"Unsupported Git entry: {metadata!r}")
        relative = Path(raw_path.decode("utf-8"))
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("Unsafe Git path")
        source, dest = ROOT / relative, target / relative
        dest.parent.mkdir(parents=True, exist_ok=True)
        if raw_path in changed or not source.is_file():
            dest.write_bytes(subprocess.check_output(["git", "cat-file", "blob", blob.decode("ascii")], cwd=ROOT))
        elif relative.suffix.lower() in (".dm", ".dme", ".md", ".py", ".json", ".txt", ".toml", ".yml", ".yaml"):
            shutil.copy2(source, dest)
        else:
            # Assets are shared read-only; mutable source and configs have independent copies.
            os.link(source, dest)
        count += 1
    (target / "baseline-source.json").write_text(json.dumps(dict(revision=revision, files=count), indent=2) + "\n", encoding="utf-8")
    print(f"Prepared untouched {revision}: {count} files in {target}")


if __name__ == "__main__":
    main()
