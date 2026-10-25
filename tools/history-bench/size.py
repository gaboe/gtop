#!/usr/bin/env python3
"""Repeat the copied day 30 times, then run production rollups and retention."""
import argparse
import json
from pathlib import Path
import sqlite3
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("input", type=Path)
parser.add_argument("seed", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
path = args.output / "history.sqlite"
assert not path.exists(), "Use a fresh output directory."
with sqlite3.connect(args.seed / "history.sqlite") as source:
    with sqlite3.connect(path) as target:
        source.backup(target)
with sqlite3.connect(path) as db:
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA synchronous=FULL")
    db.execute("CREATE TEMP TABLE seed_system AS SELECT * FROM system WHERE resolution=1")
    db.execute("CREATE TEMP TABLE seed_process AS SELECT * FROM process WHERE resolution=1")
    original_system = db.execute("SELECT COUNT(*),SUM(seconds) FROM seed_system").fetchone()
    original_process = db.execute("SELECT COUNT(*),SUM(cpu_seconds),SUM(gpu_seconds) FROM seed_process").fetchone()
    db.execute("DELETE FROM process WHERE resolution<>1")
    db.execute("DELETE FROM system WHERE resolution<>1")
    db.execute("DELETE FROM rollup_meta")
    db.execute("UPDATE meta SET compacted_before=0")
    for day in range(1, 30):
        shift = day * 1440
        db.execute("INSERT INTO system SELECT resolution,minute-?,recorded,seconds,cpu,memory,gpu,gpu_seconds,temperature,temp_seconds,peak_cpu,peak_memory,peak_gpu,peak_temp FROM seed_system", [shift])
        db.execute("INSERT INTO process SELECT resolution,minute-?,name_id,cpu_seconds,gpu_seconds,peak_memory,peak_cpu FROM seed_process", [shift])
        db.commit()
        print(f"synthesized day {day + 1}", flush=True)
    anchor = db.execute("SELECT MAX(minute) FROM system").fetchone()[0]
binary = Path(__file__).resolve().parent / "target/release/gtop-history-bench"
completed = subprocess.run([str(binary), "retain", str(args.input.resolve()), str(args.output.resolve()), str(anchor)], capture_output=True, text=True, check=True)
assert json.loads(completed.stdout)["retained"]
with sqlite3.connect(path) as db:
    counts = {str(resolution): dict(system=db.execute("SELECT COUNT(*),COALESCE(SUM(recorded),0),COALESCE(SUM(seconds),0) FROM system WHERE resolution=?", [resolution]).fetchone(), process=db.execute("SELECT COUNT(*),COALESCE(SUM(cpu_seconds),0),COALESCE(SUM(gpu_seconds),0) FROM process WHERE resolution=?", [resolution]).fetchone()) for resolution in [1, 10, 60]}
    actual_minutes = counts["1"]["system"][1] + counts["10"]["system"][1]
    assert actual_minutes == original_system[0] * 30
    for index in [1, 2]:
        expected = original_process[index] * 30
        actual = counts["1"]["process"][index] + counts["10"]["process"][index]
        assert abs(actual - expected) <= abs(expected) * 1e-8
    assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = dict(bytes=path.stat().st_size, allocated_bytes=path.stat().st_blocks * 512, free_pages=db.execute("PRAGMA freelist_count").fetchone()[0])
    db.execute("VACUUM")
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    after = dict(bytes=path.stat().st_size, allocated_bytes=path.stat().st_blocks * 512, free_pages=db.execute("PRAGMA freelist_count").fetchone()[0])
sidecars = {suffix: Path(str(path) + suffix).stat().st_size if Path(str(path) + suffix).exists() else 0 for suffix in ["-wal", "-shm"]}
report = dict(days=30, n=1, copied_fixture=str(args.input.resolve()), repeated_day=True, continuous_uptime=False, replays_individual_minute_commits=False, counts=counts, before_vacuum=before, after_vacuum=after, sidecars=sidecars, verified=True)
(args.output / "size.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
