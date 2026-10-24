#!/usr/bin/env python3
"""Run later with permission. Sample an existing frontmost gtop, without focusing it.

Launch gtop with NEOHTOP_PERF=1 and redirect stderr to a new log file first.
Pass its PID, that log, and an output JSON path. Exit nonzero on invalid samples.
CPU is ps averaged CPU. RSS is resident memory in KiB. Frame logs report CPU
Window::draw work, not GPU completion. FPS counts draws in each sampled second.
"""
import argparse
import json
import re
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("frame_log", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    offset = args.frame_log.stat().st_size
    rows = []
    started = time.monotonic()
    for index in range(60):
        time.sleep(max(0, started + index - time.monotonic()))
        front = subprocess.run(["osascript", "-e",
            'tell application "System Events" to get unix id of first process whose frontmost is true'],
            capture_output=True, text=True)
        sample = subprocess.run(["ps", "-p", str(args.pid), "-o", "pid=,%cpu=,rss="],
                                capture_output=True, text=True)
        fields = sample.stdout.split()
        valid = (front.returncode == 0 and front.stdout.strip() == str(args.pid)
                 and sample.returncode == 0 and len(fields) == 3)
        row = {"second": index, "time": time.time(), "frontmost": front.stdout.strip(),
               "valid": valid, "ps": sample.stdout.strip(),
               "errors": front.stderr + sample.stderr}
        if len(fields) == 3:
            row.update(cpu_ps_percent=float(fields[1]), rss_kib=int(fields[2]))
        rows.append(row)
    time.sleep(max(0, started + 60 - time.monotonic()))
    with args.frame_log.open("rb") as source:
        source.seek(offset)
        logs = source.read().decode(errors="replace")
    frames = [line for line in logs.splitlines() if line.startswith("perf frames ")]
    ticks = [line for line in logs.splitlines() if line.startswith("perf tick ")]
    drawn_frames = sum(int(value) for line in frames
                       for value in re.findall(r"(?:^| )draws=(\d+)(?: |$)", line))
    valid = (all(row["valid"] for row in rows) and len(frames) >= 55
             and drawn_frames > 0 and bool(ticks))
    args.output.write_text(json.dumps({"valid": valid, "samples": rows,
        "frame_samples": frames, "drawn_frames": drawn_frames, "tick_samples": ticks,
        "limitations": "Frontmost is checked per sample. No pixel exposure proof. CPU draw only; GPU completion unmeasured."}, indent=2) + "\n")
    raise SystemExit(0 if valid else "Invalid measurement. See saved samples.")


if __name__ == "__main__":
    main()
