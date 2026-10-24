# Measure tick and draw costs

Build each revision with `cargo build --release -p gtop-core --example tick_cost`.
Copy each resulting executable from `target/release/examples/tick_cost` to a distinct path before the next build.
Use the same lockfile, build profile, host, and workload for both sides.
Do not build or profile during the comparison.

Run the alternating comparison from the repository root.

```sh
rtk proxy python3 crates/gtop-core/benches/compare.py /absolute/base /absolute/candidate /absolute/results
```

The defaults are five alternating pairs, 60 measured ticks per run, and the native app's default three-second interval.
Each run discards two warmup ticks.
The timer covers the complete `AppState::snapshot(false, false)` call.
It includes refresh, GPU collection, temperatures, and process output construction.
The native app collects ports in a separate worker and loads environment variables on demand.
Those paths are excluded from this tick metric on both sides.

The example checks that each snapshot contains its own PID and CPU and memory data.
The runner saves stdout, stderr, binary hashes, errors, work counts, process counts, median, p90, and loads before and after each run.
It marks load above the logical core count as noisy.
The summary marks a pair as similar only when all boundary loads differ by at most 20 percent or one load unit, and process counts differ by at most 10 percent.
These flags help inspection. They do not prove that contention stayed constant between boundaries.
Do not use mismatched pairs for a speed claim.
A gap smaller than run variation does not establish a win.

The `--runs`, `--ticks`, and `--interval` options support diagnostic runs.
A short run or a different interval does not satisfy the production measurement gate.
A release profile with stripped symbols is suitable for timing.
Use a separate run for profiling.

## Measure the visible window later

Do this only after the operator permits the visible measurement.
Start the release app with `NEOHTOP_PERF=1` and redirect stderr to a new log file.
Keep its window frontmost and exposed for the full measurement.
Do not interact with it during the 60 seconds.

```sh
rtk proxy python3 crates/gtop-core/benches/visible_idle.py GTOP_PID /absolute/new-frame.log /absolute/visible.json
```

The script checks the frontmost PID before each of 60 `ps` samples.
It fails if the PID differs, sampling fails, frame telemetry is absent, no drawn frame is recorded, or tick telemetry is absent.
The host may require Automation permission for the frontmost check.
The script never changes focus.
A frontmost check does not prove that every pixel was exposed.
Check window exposure independently before interpreting the result.

CPU is the average reported by macOS `ps`, not a calculated one-second CPU delta.
RSS is resident memory in KiB.
The frame telemetry uses GPUI's completed `Window::draw` histogram.
It reports mean CPU draw time and the draw count divided by the actual sampling interval.
The interval is approximately one second.
No frame is requested to obtain a measurement.
The UI retains the mean from the last interval that contained a draw.
An idle window can report zero FPS.
Neither GPU execution nor display presentation completion is measured.
The GPUI profiler feature is enabled, but trace retention is not enabled.
Its runtime overhead still needs a visible-window comparison.
