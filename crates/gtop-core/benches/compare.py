#!/usr/bin/env python3
"""Alternate release tick_cost binaries. No build or load process runs here."""
from concurrent.futures import ThreadPoolExecutor
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import select
import subprocess
import time


def percentile(values, quantile):
    return sorted(values)[max(0, math.ceil(len(values) * quantile) - 1)]


def measure(binary, ticks, interval):
    load = os.getloadavg()
    started = time.time()
    result = subprocess.run([binary, str(ticks), str(interval)],
                            capture_output=True, text=True,
                            timeout=max(60, (ticks + 2) * interval * 2))
    return result, load, started, os.getloadavg(), time.time() - started


def measure_pair(binaries, ticks, interval):
    processes = {}
    try:
        for side in ("base", "candidate"):
            processes[side] = subprocess.Popen(
                [binaries[side]["path"], str(ticks), str(interval), "--barrier"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        for process in processes.values():
            if not select.select([process.stdout], [], [], 30)[0]:
                raise TimeoutError("collector did not reach the start barrier")
            if process.stdout.readline().strip() != "ready":
                raise ValueError("collector failed before the start barrier")
        started = time.time()
        load = os.getloadavg()
        deadline_us = int((started + .2) * 1e6)
        for index, process in enumerate(processes.values()):
            process.stdin.write(str(deadline_us + int(index * interval * .5e6)) + "\n")
            process.stdin.flush()

        def finish(process):
            stdout, stderr = process.communicate(timeout=max(60, (ticks + 2) * interval * 2))
            result = subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)
            return result, load, started, os.getloadavg(), time.time() - started

        with ThreadPoolExecutor(max_workers=2) as executor:
            pending = {side: executor.submit(finish, process) for side, process in processes.items()}
            return {side: future.result() for side, future in pending.items()}
    finally:
        for process in processes.values():
            if process.poll() is None:
                process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--interleave", action="store_true",
                        help="offset two default-cadence collectors by half an interval")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--ticks", type=int, default=60)
    parser.add_argument("--interval", type=float, default=3.)
    args = parser.parse_args()
    if args.runs < 1 or args.ticks < 1 or not math.isfinite(args.interval) or args.interval <= 0:
        parser.error("runs, ticks and interval must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    report = {"cores": os.cpu_count(), "ticks": args.ticks,
              "interval_seconds": args.interval, "interleaved": args.interleave,
              "runs": [], "binaries": {}}
    for side in ("base", "candidate"):
        binary = getattr(args, side).resolve()
        report["binaries"][side] = {"path": str(binary),
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    for pair in range(args.runs):
        if args.interleave:
            measured = measure_pair(report["binaries"], args.ticks, args.interval)
        for side in ("base", "candidate"):
            if args.interleave:
                result, load, started, load_after, duration = measured[side]
            else:
                result, load, started, load_after, duration = measure(
                    report["binaries"][side]["path"], args.ticks, args.interval)
            stem = f"{pair + 1:02}-{side}"
            (args.output / f"{stem}.stdout").write_text(result.stdout)
            (args.output / f"{stem}.stderr").write_text(result.stderr)
            rows = []
            parse_errors = 0
            for line in result.stdout.splitlines():
                try:
                    row = json.loads(line)
                    if not isinstance(row, dict):
                        raise ValueError("tick is not an object")
                    for key in ("elapsed_us", "cpu_us", "processes"):
                        value = row.get(key)
                        if (isinstance(value, bool) or not isinstance(value, (int, float))
                                or not math.isfinite(value) or value <= 0):
                            raise ValueError(f"invalid {key}")
                    if args.interleave:
                        gap = row.get("gap_us", 0)
                        if not args.interval * 0.5e6 <= gap <= args.interval * 2e6:
                            raise ValueError("cadence gap")
                        if row.get("started_unix_us", 0) <= 0:
                            raise ValueError("missing sample timestamp")
                    if row.get("tick") != len(rows) or row.get("errors") != 0:
                        raise ValueError("invalid tick sequence or failed snapshot")
                    rows.append(row)
                except (ValueError, TypeError):
                    parse_errors += 1
            errors = parse_errors + sum(row.get("errors", 1) for row in rows)
            errors += int(result.returncode != 0) + int(len(rows) != args.ticks)
            errors += sum(int(row.get("processes", 0) <= 0) for row in rows)
            errors += int(duration < 0 or duration > (args.ticks + 2) * args.interval * 1.5 + 10)
            times = [row["elapsed_us"] / 1000 for row in rows]
            cpu_times = [row["cpu_us"] / 1000 for row in rows]
            run = {"pair": pair + 1, "side": side, "started": started,
                   "load_before": load, "load_after": load_after, "duration_seconds": duration,
                   "noisy": max(load[0], load_after[0]) > report["cores"],
                   "returncode": result.returncode, "errors": errors,
                   "work_ticks": len(rows), "rows": rows}
            if times:
                run.update(cpu_median_ms=statistics.median(cpu_times),
                           cpu_p90_ms=percentile(cpu_times, .9),
                           median_ms=statistics.median(times), p90_ms=percentile(times, .9),
                           process_min=min(row["processes"] for row in rows),
                           process_max=max(row["processes"] for row in rows))
            report["runs"].append(run)
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({k: v for k, v in run.items() if k != "rows"}), flush=True)
            if errors:
                raise SystemExit("Invalid run. See saved output.")
    for side, binary in report["binaries"].items():
        if hashlib.sha256(Path(binary["path"]).read_bytes()).hexdigest() != binary["sha256"]:
            raise SystemExit(f"{side} binary changed during measurement")
    summary = {}
    for side in ("base", "candidate"):
        runs = [run for run in report["runs"] if run["side"] == side]
        medians = [run["median_ms"] for run in runs]
        cpu_medians = [run["cpu_median_ms"] for run in runs]
        summary[side] = {"cpu_median_ms": statistics.median(cpu_medians),
                         "cpu_range_ms": [min(cpu_medians), max(cpu_medians)],
                         "median_ms": statistics.median(medians),
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
        alternate = None
        paired_counts_match = None
        paired_count_difference = None
        no_overlap = None
        if args.interleave:
            paired_rows = list(zip(base["rows"], candidate["rows"]))
            alternate = all(0 < b["started_unix_us"] - a["started_unix_us"] < args.interval * 1e6
                            for a, b in paired_rows)
            paired_count_difference = max(max(a["processes"], b["processes"]) /
                                          min(a["processes"], b["processes"]) - 1
                                          for a, b in paired_rows)
            paired_counts_match = paired_count_difference <= .1
            no_overlap = all(a["started_unix_us"] + a["elapsed_us"] <= b["started_unix_us"]
                             for a, b in paired_rows)
            no_overlap = no_overlap and all(
                b["started_unix_us"] + b["elapsed_us"] <= a["started_unix_us"]
                for a, b in zip(base["rows"][1:], candidate["rows"][:-1]))
        summary["pairs"].append({"pair": pair + 1, "ticks_alternate": alternate,
            "paired_counts_match": paired_counts_match,
            "maximum_paired_count_difference": paired_count_difference,
            "snapshots_do_not_overlap": no_overlap,
            "similar_load": max(loads) - min(loads) <= max(1., min(loads) * .2),
            "similar_process_count": max(process_counts) <= min(process_counts) * 1.1,
            "noisy": base["noisy"] or candidate["noisy"],
            "cpu_delta_percent": (candidate["cpu_median_ms"] / base["cpu_median_ms"] - 1) * 100,
            "base_cpu_ms": base["cpu_median_ms"],
            "candidate_cpu_ms": candidate["cpu_median_ms"],
            "base_ms": base["median_ms"], "candidate_ms": candidate["median_ms"]})
    summary["verdict"] = "compare control spread, cadence, paired work counts and variation before acceptance"
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
