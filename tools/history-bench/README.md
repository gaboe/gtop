# History storage checks

This standalone crate compiles the production `history.rs` and `analysis.rs`. It excludes GPUI rendering. The JSON adapter preserves load, save, prune, and view behavior from commit `809dd7082817356537b73eccf42dee2bc46dd2ec`. It uses the shared unchanged view types.

Build the unprofiled release binary from the repository root.

```sh
rtk proxy cargo build --release --manifest-path tools/history-bench/Cargo.toml --locked
```

Run five alternating fresh-process trials on a copied JSON fixture. The output directory must be new.

```sh
rtk proxy python3 tools/history-bench/run.py copied-history.json /private/tmp/history-paired
```

The runner verifies row totals, columns, gaps, and leaders before timing. Startup includes open or typed JSON load plus all three views. Later view queries use that warmed history. Append stages the largest source minute before timing. Its timer covers production persistence, including hourly maintenance, or the complete JSON save. Readback verifies the new minute. The output reports process CPU, wall time, trial count, ranges, and errors.

Profile separately. This build enables statement counters and symbols. Never use it for paired timing.

```sh
rtk proxy cargo rustc --release --manifest-path tools/history-bench/Cargo.toml --locked --features history-profile -- -C strip=none -C debuginfo=1
rtk proxy python3 tools/history-bench/profile.py copied-history.json /private/tmp/history-paired/seed /private/tmp/history-profile sqlite
rtk proxy python3 tools/history-bench/profile.py copied-history.json /private/tmp/history-paired/seed /private/tmp/history-profile json
```

The profile script samples only its own child. It requires macOS `sample`. Rebuild without `history-profile` before timing again.

The size probe repeats the copied day 30 times, including gaps and zero process entries. It runs production hourly maintenance and retention. It verifies work totals and database integrity. It reports file sizes before and after explicit vacuum. Production does not vacuum automatically.

```sh
rtk proxy python3 tools/history-bench/size.py copied-history.json /private/tmp/history-paired/seed /private/tmp/history-size
```

The size probe does not replay individual minute commits. It does not model new names, continuous uptime, or steady-state fragmentation. Its compact file size is one measured synthetic fixture, not a capacity guarantee.
