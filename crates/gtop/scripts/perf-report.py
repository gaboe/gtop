#!/usr/bin/env python3
"""Report per-state gtop CPU use from a copy of perf.sqlite."""

import argparse
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


def summarize(path: Path) -> list[tuple[str, str, str, int, float, float, float]]:
    rows = []
    with sqlite3.connect(path) as db:
        db.row_factory = sqlite3.Row
        for row in db.execute("SELECT * FROM perf_minute ORDER BY wall_seconds"):
            # Assign the whole minute to its start hour, including boundary-ending minutes.
            start = (row["interval_start_seconds"] if "interval_start_seconds" in row.keys()
                     else row["wall_seconds"] - sum(float(row[column]) for _, column, _ in STATES))
            hour = math.floor(start / 3600)
            version = row["app_version"]
            for state, seconds_column, cpu_column in STATES:
                seconds = float(row[seconds_column])
                if seconds <= 0:
                    continue
                percent = float(row[cpu_column]) * 100 / seconds
                rows.append((hour, version, state, percent))
    grouped: dict[tuple[int, str, str], list[float]] = {}
    for hour, version, state, percent in rows:
        grouped.setdefault((hour, version, state), []).append(percent)
    return [
        (str(hour), version, state, len(values), statistics.median(values), percentile(values, 0.90), percentile(values, 0.95))
        for (hour, version, state), values in sorted(grouped.items())
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path, help="path to perf.sqlite")
    args = parser.parse_args()
    if not args.database.is_file():
        parser.error(f"database does not exist: {args.database}")
    with tempfile.TemporaryDirectory(prefix="gtop-perf-report-") as scratch:
        copy = Path(scratch) / "perf.sqlite"
        copy_database(args.database, copy)
        rows = summarize(copy)
    print("hour_utc\tversion\tstate\tn\tmedian_core_pct\tp90_core_pct\tp95_core_pct")
    for row in rows:
        print("\t".join([*map(str, row[:4]), *(f"{value:.3f}" for value in row[4:])]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
