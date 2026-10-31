# Run gtop with a native GPUI window

From the repository root, run:

```sh
cargo run --release -p gtop
```

The app uses `gtop-core` for system monitoring. It does not start a webview.

To create an ad hoc signed macOS bundle, run:

```sh
sh crates/gtop/package-macos.sh
```

The bundle is `target/gtop-native/gtop.app`. Packaging does not install it in `/Applications`.

Click a table header to sort. Click it again to reverse the order. Search accepts names, commands, PID substrings, and comma-separated alternatives. Use `:3000` or `port:3000` to find a local bound port. While the window is visible and not paused, gtop refreshes its port cache every 10 seconds. A valid port search also collects ports on each sample.

Select a process to show its command, root directory, resources, parent, and children. Load environment variables on demand. The kill action requires a second confirmation. It checks the process start time before sending the signal.

Choose a refresh interval from 1, 2, 3, 5, or 10 seconds. The default is 3 seconds. Pause stops sampling. Sampling continues while the window is hidden or covered. Closing the window hides it and quits gtop after ten minutes.

The SSD panel shows system-volume storage in decimal GB. On macOS, the app reads free space on every sample and refreshes total capacity and the paired purgeable-space estimate every 60 seconds while sampling runs. Purgeable space is the difference between `NSURLVolumeAvailableCapacityForImportantUsageKey` and free space from the same refresh. APFS volumes share the container capacity. On other platforms, gtop uses sysinfo's available-space value, which updates during the 60-second disk refresh. It reports no purgeable space.

## Current metrics file

While gtop runs, it writes `~/Library/Application Support/gtop/now.json` once per sample, at the selected refresh interval. The file contains the latest system sample. Its `time` is the sample time in Unix seconds, and `interval_s` is the active refresh interval. Readers can treat data as stale when its age exceeds three times `interval_s`. The file remains after gtop exits.

Version 1 has `cpu.percent`; `memory.used_bytes`, `memory.total_bytes`, and `memory.percent`; `ssd.free_bytes`, `ssd.total_bytes`, `ssd.read_bytes_per_s`, `ssd.write_bytes_per_s`, and `ssd.temperature_c`; and `net.rx_bytes_per_s`, `net.tx_bytes_per_s`, `net.latency_ms`, `net.loss_percent`, and `net.probe`. Percentages use the same values as the CPU and memory panels. The SSD byte counts use the same system-volume values as the SSD panel. Read and write rates are `null` until two valid cumulative samples are available. They return to `null` after a counter reset or a sampling gap longer than three intervals. The temperature is `null` when no SSD sensor is available.

The network probe opens a TCP connection to `1.1.1.1:443` every 15 seconds and closes it after the handshake. `latency_ms` is the median of the last four successful probes from the last five minutes. `loss_percent` covers up to 20 probes from the last five minutes. Both values are `null` when no matching probe results remain.

To log sampling and visible UI ticks, run:

```sh
GTOP_PERF=1 cargo run --release -p gtop
```

The app pins `gpui-pre` and its platform crate to `0.3.9`. It pins `gpui-component` to `0.8.0`, which requires that exact GPUI version. The process table uses custom GPUI elements. It renders the first ten rows directly and virtualizes the remaining rows with `uniform_list`. Columns are not virtualized. Search and sorting run when input or a snapshot changes. They do not run during row rendering.

On macOS, `font-kit` enables native text. The app compiles Metal shaders at startup through GPUI's `runtime_shaders` feature. This avoids a build dependency on Apple's optional Metal Toolchain. The first window can take longer to appear.

Run the focused checks from the repository root:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

## README screenshots

From the repository root on macOS, run:

```sh
cargo test -p gtop --bin gtop readme_screenshots -- --ignored --nocapture
```

This writes `docs/screenshots/live.png`, `detail.png`, and `history24h.png` using a fixed clock and 40 fake processes. It uses GPUI TestPlatform with Apple text and the headless Metal renderer. It opens no native window and reads no live monitoring data or saved settings. The disk screenshot is omitted because the existing disk fixture starts a real filesystem scan.
