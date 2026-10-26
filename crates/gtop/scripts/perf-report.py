#!/usr/bin/env python3
"""Report per-state gtop CPU use from a copy of perf.sqlite."""

import argparse
from dataclasses import dataclass, field
import math
from pathlib import Path
import statistics
import sqlite3
import tempfile
from urllib.parse import quote


STATES = (
    ("visible", "visible_seconds", "cpu_visible_seconds"),
    ("occluded", "occluded_seconds", "cpu_occluded_seconds"),
    ("hidden", "hidden_seconds", "cpu_hidden_seconds"),
    ("minimized", "minimized_seconds", "cpu_minimized_seconds"),
    ("unknown", "unknown_seconds", "cpu_unknown_seconds"),
)
LOAD_BUCKETS = ((8, "<8"), (20, "8-20"), (40, "20-40"))


@dataclass
class MinuteGroup:
    cpu_percent: list[float] = field(default_factory=list)


@dataclass
class WorkGroup:
    interval_seconds: float = 0.
    intervals: int = 0
    draw_count: int = 0
    draw_total_ms: float = 0.
    sampler_runs: int = 0
    sampler_total_ms: float = 0.
    system_cpu_busy_sum: float = 0.
    system_cpu_busy_count: int = 0


@dataclass(frozen=True)
class StateSummary:
    hour: str
    version: str
    state: str
    load_bucket: str
    minutes: int
    median_core_pct: float
    p90_core_pct: float
    p95_core_pct: float


@dataclass(frozen=True)
class WorkSummary:
    hour: str
    version: str
    load_bucket: str
    intervals: int
    draws_per_min: float
    mean_draw_ms: float | None
    sampler_runs_per_min: float
    mean_sampler_ms: float | None
    mean_system_cpu_busy_pct: float | None


def copy_database(source: Path, destination: Path) -> None:
    uri = f"file:{quote(str(source.resolve()))}?mode=ro"
    original = sqlite3.connect(uri, uri=True)
    copy = sqlite3.connect(destination)
    try:
        original.backup(copy)
    finally:
        original.close()
        copy.close()


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def load_bucket(value: float | None) -> str:
    if value is None:
        return "unknown"
    for limit, label in LOAD_BUCKETS:
        if value < limit:
            return label
    return ">40"


def summarize(path: Path) -> tuple[list[StateSummary], list[WorkSummary]]:
    grouped: dict[tuple[int, str, str, str], MinuteGroup] = {}
    work: dict[tuple[int, str, str], WorkGroup] = {}
    with sqlite3.connect(path) as db:
        db.row_factory = sqlite3.Row
        for row in db.execute("SELECT * FROM perf_minute ORDER BY wall_seconds"):
            columns = row.keys()
            # Assign the whole minute to its start hour, including boundary-ending minutes.
            start = (row["interval_start_seconds"] if "interval_start_seconds" in columns
                     else row["wall_seconds"] - sum(float(row[column]) for _, column, _ in STATES))
            hour = math.floor(start / 3600)
            version = row["app_version"]
            load = row["mean_load_1m"] if "mean_load_1m" in columns else None
            bucket = load_bucket(float(load) if load is not None else None)
            elapsed = float(row["wall_seconds"]) - float(start)
            if elapsed > 0:
                key = (hour, version, bucket)
                summary = work.setdefault(key, WorkGroup())
                summary.interval_seconds += elapsed
                summary.intervals += 1
                summary.draw_count += int(row["draw_count"])
                summary.draw_total_ms += float(row["draw_total_ms"])
                summary.sampler_runs += int(row["sampler_runs"])
                summary.sampler_total_ms += float(row["sampler_total_ms"])
                if "mean_system_cpu_busy_percent" in columns:
                    busy = row["mean_system_cpu_busy_percent"]
                    if busy is not None:
                        summary.system_cpu_busy_sum += float(busy)
                        summary.system_cpu_busy_count += 1
            for state, seconds_column, cpu_column in STATES:
                seconds = float(row[seconds_column])
                if seconds <= 0:
                    continue
                percent = float(row[cpu_column]) * 100 / seconds
                key = (hour, version, state, bucket)
                summary = grouped.setdefault(key, MinuteGroup())
                summary.cpu_percent.append(percent)
    states = [
        StateSummary(
            str(hour), version, state, bucket, len(summary.cpu_percent),
            statistics.median(summary.cpu_percent), percentile(summary.cpu_percent, 0.90),
            percentile(summary.cpu_percent, 0.95),
        )
        for (hour, version, state, bucket), summary in sorted(grouped.items())
    ]
    intervals = [
        WorkSummary(
            str(hour), version, bucket, summary.intervals,
            summary.draw_count * 60 / summary.interval_seconds,
            summary.draw_total_ms / summary.draw_count if summary.draw_count else None,
            summary.sampler_runs * 60 / summary.interval_seconds,
            summary.sampler_total_ms / summary.sampler_runs if summary.sampler_runs else None,
            summary.system_cpu_busy_sum / summary.system_cpu_busy_count
            if summary.system_cpu_busy_count else None,
        )
        for (hour, version, bucket), summary in sorted(work.items())
    ]
    return states, intervals


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path, help="path to perf.sqlite")
    args = parser.parse_args()
    if not args.database.is_file():
        parser.error(f"database does not exist: {args.database}")
    with tempfile.TemporaryDirectory(prefix="gtop-perf-report-") as scratch:
        copy = Path(scratch) / "perf.sqlite"
        copy_database(args.database, copy)
        states, work = summarize(copy)
    print("hour_utc\tversion\tstate\tload_bucket\tminutes\tmedian_core_pct\tp90_core_pct\tp95_core_pct")
    for row in states:
        values = [row.hour, row.version, row.state, row.load_bucket, str(row.minutes)]
        values.extend(f"{value:.3f}" for value in (
            row.median_core_pct, row.p90_core_pct, row.p95_core_pct,
        ))
        print("\t".join(values))
    print("\nhour_utc\tversion\tload_bucket\tintervals\tdraws_per_min\tmean_draw_ms\tsampler_runs_per_min\tmean_sampler_ms\tmean_system_cpu_busy_pct")
    for row in work:
        values = [row.hour, row.version, row.load_bucket, str(row.intervals)]
        values.append(f"{row.draws_per_min:.3f}")
        values.append("NA" if row.mean_draw_ms is None else f"{row.mean_draw_ms:.3f}")
        values.append(f"{row.sampler_runs_per_min:.3f}")
        values.append("NA" if row.mean_sampler_ms is None else f"{row.mean_sampler_ms:.3f}")
        values.append("NA" if row.mean_system_cpu_busy_pct is None else f"{row.mean_system_cpu_busy_pct:.3f}")
        print("\t".join(values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
