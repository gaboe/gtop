#!/usr/bin/env python3
"""Alternate release tick_cost binaries. No build or load process runs here."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import time


def percentile(values, quantile):
    return sorted(values)[max(0, math.ceil(len(values) * quantile) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--ticks", type=int, default=60)
    parser.add_argument("--interval", type=float, default=3.)
    args = parser.parse_args()
    if args.runs < 1 or args.ticks < 1 or not math.isfinite(args.interval) or args.interval <= 0:
        parser.error("runs, ticks and interval must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    report = {"cores": os.cpu_count(), "ticks": args.ticks,
              "interval_seconds": args.interval, "runs": [], "binaries": {}}
    for side in ("base", "candidate"):
        binary = getattr(args, side).resolve()
        report["binaries"][side] = {"path": str(binary),
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    for pair in range(args.runs):
        for side in ("base", "candidate"):
            binary = report["binaries"][side]["path"]
            load = os.getloadavg()
            started = time.time()
            result = subprocess.run([binary, str(args.ticks), str(args.interval)],
                                    capture_output=True, text=True)
            stem = f"{pair + 1:02}-{side}"
            (args.output / f"{stem}.stdout").write_text(result.stdout)
            (args.output / f"{stem}.stderr").write_text(result.stderr)
            rows = []
            parse_errors = 0
            for line in result.stdout.splitlines():
                try:
                    rows.append(json.loads(line))
                except (ValueError, TypeError):
                    parse_errors += 1
            errors = parse_errors + sum(row.get("errors", 1) for row in rows)
            errors += int(result.returncode != 0) + int(len(rows) != args.ticks)
            errors += sum(int(row.get("processes", 0) <= 0) for row in rows)
            times = [row["elapsed_us"] / 1000 for row in rows]
            run = {"pair": pair + 1, "side": side, "started": started,
                   "load_before": load, "load_after": os.getloadavg(),
                   "noisy": max(load[0], os.getloadavg()[0]) > report["cores"],
                   "returncode": result.returncode, "errors": errors,
                   "work_ticks": len(rows), "rows": rows}
            if times:
                run.update(median_ms=statistics.median(times), p90_ms=percentile(times, .9),
                           process_min=min(row["processes"] for row in rows),
                           process_max=max(row["processes"] for row in rows))
            report["runs"].append(run)
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({k: v for k, v in run.items() if k != "rows"}), flush=True)
            if errors:
                raise SystemExit("Invalid run. See saved output.")
    summary = {}
    for side in ("base", "candidate"):
        runs = [run for run in report["runs"] if run["side"] == side]
        medians = [run["median_ms"] for run in runs]
        summary[side] = {"median_ms": statistics.median(medians),
                         "range_ms": [min(medians), max(medians)],
                         "p90_ms": statistics.median([run["p90_ms"] for run in runs]),
                         "noisy_runs": sum(run["noisy"] for run in runs)}
    summary["pairs"] = []
    for pair in range(args.runs):
        base, candidate = report["runs"][2 * pair:2 * pair + 2]
        loads = [base["load_before"][0], base["load_after"][0],
                 candidate["load_before"][0], candidate["load_after"][0]]
        process_counts = [base["process_min"], base["process_max"],
                          candidate["process_min"], candidate["process_max"]]
        summary["pairs"].append({"pair": pair + 1,
            "similar_load": max(loads) - min(loads) <= max(1., min(loads) * .2),
            "similar_process_count": max(process_counts) <= min(process_counts) * 1.1,
            "noisy": base["noisy"] or candidate["noisy"],
            "base_ms": base["median_ms"], "candidate_ms": candidate["median_ms"]})
    summary["verdict"] = "diagnostic only; compare paired loads and variation before acceptance"
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
