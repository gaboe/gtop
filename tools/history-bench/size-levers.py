#!/usr/bin/env python3
"""Measure lossless storage candidates on copies of review workload databases."""
import argparse
import json
from pathlib import Path
import sqlite3

parser = argparse.ArgumentParser()
parser.add_argument("source", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)

class Presence:
    def __init__(self):
        self.ids = []
    def step(self, value):
        self.ids.append(value)
    def finalize(self):
        result = bytearray()
        previous = 0
        for value in sorted(self.ids):
            delta = value - previous
            previous = value
            while delta >= 128:
                result.append((delta & 127) | 128)
                delta >>= 7
            result.append(delta)
        return bytes(result)

variants = [
    ("numeric_affinity", False, False, False, 4096, False),
    ("no_name_index", False, False, True, 4096, False),
    ("zero_presence", True, False, True, 4096, False),
    ("integer_memory", True, True, True, 4096, False),
    ("page8192", True, True, True, 8192, True),
    ("page16384", True, True, True, 16384, True),
]
original_path = args.output / "original.sqlite"
with sqlite3.connect(f"file:{args.source.resolve()}?mode=ro", uri=True) as source:
    with sqlite3.connect(original_path) as target:
        source.backup(target)
with sqlite3.connect(original_path) as original:
    original.execute("VACUUM")
reports = [dict(variant="original", bytes=original_path.stat().st_size, source_bytes=args.source.stat().st_size, errors=0)]
for label, zeros, memory, no_index, page_size, incremental in variants:
    path = args.output / (label + ".sqlite")
    c = sqlite3.connect(path)
    c.execute(f"PRAGMA page_size={page_size}")
    c.execute(f"PRAGMA auto_vacuum={2 if incremental else 0}")
    c.execute("ATTACH DATABASE ? AS source", (str(args.source.resolve()),))
    c.create_aggregate("presence", 1, Presence)
    tables = c.execute("SELECT name,sql FROM source.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").fetchall()
    for name, sql in tables:
        c.execute(sql.replace(" REAL", " NUMERIC") if name == "process" else sql)
        if name == "process":
            condition = "NOT(cpu_seconds=0 AND gpu_seconds=0 AND peak_memory=0 AND peak_cpu=0)" if zeros else "1"
            mem = "CASE WHEN peak_memory%1024=0 THEN peak_memory/1024 ELSE -peak_memory END" if memory else "peak_memory"
            c.execute(f"INSERT INTO process SELECT resolution,minute,name_id,cpu_seconds,gpu_seconds,{mem},peak_cpu FROM source.process WHERE {condition}")
        else:
            c.execute(f'INSERT INTO "{name}" SELECT * FROM source."{name}"')
    if zeros:
        c.execute("CREATE TABLE presence(resolution INTEGER, minute INTEGER, ids BLOB, PRIMARY KEY(resolution,minute)) WITHOUT ROWID")
        c.execute("INSERT INTO presence SELECT resolution,minute,presence(name_id) FROM source.process WHERE cpu_seconds=0 AND gpu_seconds=0 AND peak_memory=0 AND peak_cpu=0 GROUP BY resolution,minute")
    if not no_index:
        c.execute("CREATE INDEX process_name_minute ON process(name_id,resolution,minute)")
    c.commit()
    c.execute("VACUUM")
    assert c.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    original = c.execute("SELECT COUNT(*),SUM(cpu_seconds),SUM(gpu_seconds),MAX(peak_memory),MAX(peak_cpu) FROM source.process").fetchone()
    actual = c.execute("SELECT COUNT(*),SUM(cpu_seconds),SUM(gpu_seconds),MAX(" + ("CASE WHEN peak_memory>=0 THEN peak_memory*1024 ELSE -peak_memory END" if memory else "peak_memory") + "),MAX(peak_cpu) FROM process").fetchone()
    presence_count = 0
    if zeros:
        for blob, in c.execute("SELECT ids FROM presence"):
            presence_count += sum(b < 128 for b in blob)
    decoded_memory = "CASE WHEN peak_memory>=0 THEN peak_memory*1024 ELSE -peak_memory END" if memory else "peak_memory"
    columns = "resolution,minute,name_id,cpu_seconds,gpu_seconds,peak_memory,peak_cpu"
    source_filter = "WHERE NOT(cpu_seconds=0 AND gpu_seconds=0 AND peak_memory=0 AND peak_cpu=0)" if zeros else ""
    expected_rows = f"SELECT {columns} FROM source.process {source_filter}"
    actual_rows = f"SELECT resolution,minute,name_id,cpu_seconds,gpu_seconds,{decoded_memory},peak_cpu FROM process"
    assert c.execute(f"SELECT COUNT(*) FROM ({expected_rows} EXCEPT {actual_rows})").fetchone()[0] == 0
    assert c.execute(f"SELECT COUNT(*) FROM ({actual_rows} EXCEPT {expected_rows})").fetchone()[0] == 0
    assert original[0] == actual[0] + presence_count
    assert all(abs(a-b) <= 1e-8*max(abs(a),1) for a,b in zip(original[1:], actual[1:]))
    c.close()
    report = dict(variant=label, bytes=path.stat().st_size, rows=actual[0], presence=presence_count, errors=0)
    reports.append(report)
    (args.output / "summary.json").write_text(json.dumps(reports, indent=2) + "\n")
    print(json.dumps(report), flush=True)
