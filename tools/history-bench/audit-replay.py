#!/usr/bin/env python3
"""Check retained work and hourly values after a production replay."""
import argparse
import json
from pathlib import Path
import sqlite3

parser = argparse.ArgumentParser()
parser.add_argument("mode", choices=["dense", "sparse"])
parser.add_argument("input", type=Path)
parser.add_argument("directory", type=Path)
parser.add_argument("days", type=int)
parser.add_argument("--seed", type=Path)
args = parser.parse_args()
source = json.loads(args.input.read_text())["buckets"]
first = source[0]["minute"]
anchor = 100800
seed = None
if args.seed:
    seed = sqlite3.connect(f"file:{args.seed.resolve()}?mode=ro", uri=True)
    anchor = seed.execute("SELECT MAX(minute) FROM system WHERE resolution=1").fetchone()[0] + 1
now = anchor + (args.days - 1) * 1440 + (1439 if args.mode == "dense" else source[-1]["minute"] - first)
cutoff = (now - 7 * 1440) // 10 * 10
expired = (now - 30 * 1440) // 10 * 10
weights = [(b["minute"]-first, b["seconds"], sum(p["cpu_seconds"] for p in b["processes"].values()), sum(p["gpu_seconds"] for p in b["processes"].values())) for b in source]
if args.mode == "dense":
    weights = [(offset, 60, weights[0][2], weights[0][3]) for offset in range(1440)]
expected = [0, 0., 0., 0.]
fine = 0
coarse = set()
if seed:
    system = seed.execute("SELECT COALESCE(SUM(recorded),0),COALESCE(SUM(seconds),0) FROM system WHERE resolution IN (1,10) AND minute>=?", [expired]).fetchone()
    work = seed.execute("SELECT TOTAL(cpu_seconds),TOTAL(gpu_seconds) FROM process WHERE resolution IN (1,10) AND minute>=?", [expired]).fetchone()
    expected = list(system) + list(work)
    fine = seed.execute("SELECT COUNT(*) FROM system WHERE resolution=1 AND minute>=?", [cutoff]).fetchone()[0]
    coarse.update(row[0] for row in seed.execute("SELECT DISTINCT minute/10*10 FROM system WHERE resolution IN (1,10) AND minute>=? AND minute<?", [expired,cutoff]))
for day in range(args.days):
    for offset, seconds, cpu, gpu in weights:
        minute = anchor + day * 1440 + offset
        if minute < expired:
            continue
        expected[0] += 1
        expected[1] += seconds
        expected[2] += cpu
        expected[3] += gpu
        if minute >= cutoff:
            fine += 1
        else:
            coarse.add(minute // 10 * 10)
path = args.directory / "history.sqlite"
c = sqlite3.connect(f"file:{path.resolve()}?mode=ro", uri=True)
assert c.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
actual = list(c.execute("SELECT SUM(recorded),SUM(seconds) FROM system WHERE resolution IN (1,10)").fetchone())
actual += list(c.execute("SELECT TOTAL(cpu_seconds),TOTAL(gpu_seconds) FROM process WHERE resolution IN (1,10)").fetchone())
for a, b in zip(actual, expected):
    assert abs(a-b) <= 1e-8 * max(abs(b), 1), (actual, expected)
assert c.execute("SELECT COUNT(*) FROM system WHERE resolution=1").fetchone()[0] == fine
assert c.execute("SELECT COUNT(*) FROM system WHERE resolution=10").fetchone()[0] == len(coarse)
hours = {row[:2]: row[2:] for row in c.execute("SELECT minute,name_id,cpu_seconds,gpu_seconds,peak_memory,peak_cpu FROM process WHERE resolution=60")}
groups = list(c.execute("SELECT minute/60*60,name_id,TOTAL(cpu_seconds),TOTAL(gpu_seconds),MAX(peak_memory),MAX(peak_cpu) FROM process WHERE resolution=1 GROUP BY minute/60,name_id"))
assert len(groups) == len(hours)
for minute, name, cpu, gpu, memory, peak in groups:
    a = hours[(minute,name)]
    assert abs(a[0]-cpu) <= 1e-8*max(abs(cpu),1)
    assert abs(a[1]-gpu) <= 1e-8*max(abs(gpu),1)
    assert a[2:] == (memory,peak)

def decode(blob):
    previous = 0
    delta = 0
    shift = 0
    ids = set()
    for byte in blob:
        delta |= (byte & 127) << shift
        if byte < 128:
            previous += delta
            assert previous not in ids
            ids.add(previous)
            delta = 0
            shift = 0
        else:
            shift += 7
    assert shift == 0
    return ids

presence = {row[:2]:decode(row[2]) for row in c.execute("SELECT resolution,minute,ids FROM presence")}
expected_hourly = {}
for (resolution, minute), ids in presence.items():
    if resolution == 1:
        expected_hourly.setdefault(minute//60*60,set()).update(ids)
for (minute,name) in hours:
    expected_hourly.setdefault(minute,set()).discard(name)
actual_hourly = {minute:ids for (resolution,minute),ids in presence.items() if resolution == 60 and ids}
assert {m:ids for m,ids in expected_hourly.items() if ids} == actual_hourly
name_ids = dict(c.execute("SELECT name,id FROM names"))
for minute, in c.execute("SELECT minute FROM system WHERE resolution=1 AND minute>=? ORDER BY minute LIMIT 3", [anchor]):
    original = source[0] if args.mode == "dense" else next(b for b in source if (b["minute"]-first) == (minute-anchor)%1440)
    rows = {name:(cpu,gpu,memory,peak) for name,cpu,gpu,memory,peak in c.execute("SELECT n.name,p.cpu_seconds,p.gpu_seconds,p.peak_memory,p.peak_cpu FROM process p JOIN names n ON n.id=p.name_id WHERE p.resolution=1 AND p.minute=?",[minute])}
    for name in original["processes"]:
        if name_ids[name] in presence.get((1,minute),set()):
            rows[name] = (0.,0.,0,0.)
    assert rows.keys() == original["processes"].keys()
    for name,p in original["processes"].items():
        values = (p["cpu_seconds"],p["gpu_seconds"],p["peak_memory"],p["peak_cpu"])
        assert all(abs(a-b) <= 1e-6*max(abs(b),1) for a,b in zip(rows[name],values))
report = dict(mode=args.mode,days=args.days,seed=str(args.seed) if args.seed else None,n=1,bytes=path.stat().st_size,wal_bytes=Path(str(path)+"-wal").stat().st_size if Path(str(path)+"-wal").exists() else 0,free_pages=c.execute("PRAGMA freelist_count").fetchone()[0],recorded_minutes=actual[0],seconds=actual[1],cpu_work=actual[2],gpu_work=actual[3],fine_minutes=fine,coarse_bins=len(coarse),hourly_nonzero_rows=len(hours),hourly_presence_bins=len(actual_hourly),errors=0,integrity="ok")
(args.directory / "audit.json").write_text(json.dumps(report,indent=2)+"\n")
print(json.dumps(report,indent=2))
