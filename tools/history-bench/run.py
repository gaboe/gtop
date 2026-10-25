#!/usr/bin/env python3
"""Alternate fresh-process JSON and production SQLite trials on a copied fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import statistics
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("input", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--trials", type=int, default=5)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
binary = Path(__file__).resolve().parent / "target/release/gtop-history-bench"
fixture = json.loads(args.input.read_bytes())
anchor = max(bucket["minute"] for bucket in fixture["buckets"])

def run(mode, directory):
    command = [str(binary), mode, str(args.input.resolve()), str(directory.resolve()), str(anchor)]
    completed = subprocess.run(command, capture_output=True, text=True, check=True)
    records = [json.loads(line) for line in completed.stdout.splitlines()]
    assert records and (records[-1].get("verified") or records[-1].get("prepared")), records
    return records

seed = args.output / "seed"
assert not seed.exists(), "Use a fresh output directory."
prepared = run("prepare", seed)
measurements = []
for trial in range(args.trials):
    modes = ["json", "sqlite"] if trial % 2 == 0 else ["sqlite", "json"]
    for mode in modes:
        directory = args.output / f"{trial}-{mode}"
        directory.mkdir()
        if mode == "sqlite":
            with sqlite3.connect(seed / "history.sqlite") as source:
                with sqlite3.connect(directory / "history.sqlite") as target:
                    source.backup(target)
        records = run(mode, directory)
        measurements.extend(dict(record, mode=mode, trial=trial) for record in records)
        (args.output / "measurements.json").write_text(json.dumps(measurements, indent=2) + "\n")
        print(f"trial {trial + 1} {mode} verified", flush=True)
summary = []
for mode in ["json", "sqlite"]:
    for operation in ["startup", "append_minute", "view_1h", "view_24h"]:
        rows = [r for r in measurements if r["mode"] == mode and r.get("operation") == operation]
        assert len(rows) == args.trials
        record = dict(mode=mode, operation=operation, n=len(rows))
        for metric in ["cpu_ms", "wall_ms"]:
            values = [r[metric] for r in rows]
            record[metric] = dict(median=statistics.median(values), min=min(values), max=max(values))
        summary.append(record)
report = dict(input=str(args.input.resolve()), sha256=hashlib.sha256(args.input.read_bytes()).hexdigest(), anchor=anchor, prepared=prepared, summary=summary, errors=0, load=os.getloadavg(), cores=os.cpu_count())
(args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
