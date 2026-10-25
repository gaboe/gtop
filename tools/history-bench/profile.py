#!/usr/bin/env python3
"""Sample only the child started here. Run separately from paired timing trials."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("input", type=Path)
parser.add_argument("store", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("mode", choices=["json", "sqlite"])
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
anchor = max(b["minute"] for b in json.loads(args.input.read_bytes())["buckets"])
binary = Path(__file__).resolve().parent / "target/release/gtop-history-bench"
with (args.output / f"{args.mode}-counters.txt").open("w") as counters:
    child = subprocess.Popen([str(binary), f"profile-{args.mode}", str(args.input.resolve()), str(args.store.resolve()), str(anchor)], stdout=subprocess.PIPE, stderr=counters, text=True)
    try:
        ready = json.loads(child.stdout.readline())
        assert ready["profile_ready"] and ready["pid"] == child.pid
        sampled = subprocess.run(["sample", str(child.pid), "3", "1", "-file", str((args.output / f"{args.mode}-sample.txt").resolve())], capture_output=True, text=True, check=True)
        output, _ = child.communicate(timeout=20)
        assert child.returncode == 0
        result = json.loads(output)
        assert result["profile_complete"] and result["views"] > 0
        (args.output / f"{args.mode}-usage.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
    finally:
        if child.poll() is None:
            child.terminate()
            child.wait()
