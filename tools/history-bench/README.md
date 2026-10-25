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

The size probe repeats the copied day 30 times, including gaps and zero process entries. It runs production hourly maintenance and retention. It verifies work totals and database integrity. It reports file sizes before and after explicit vacuum. New stores use incremental auto-vacuum after retention.

```sh
rtk proxy python3 tools/history-bench/size.py copied-history.json /private/tmp/history-paired/seed /private/tmp/history-size
```

The size probe does not replay individual minute commits. It does not model new names, continuous uptime, or steady-state fragmentation. Its compact file size is one measured synthetic fixture, not a capacity guarantee.

Replay each minute through production persistence in fresh scratch directories.

```sh
rtk proxy tools/history-bench/target/release/gtop-history-bench replay-dense copied-history.json /private/tmp/history-dense 40
rtk proxy python3 tools/history-bench/audit-replay.py dense copied-history.json /private/tmp/history-dense 40
rtk proxy tools/history-bench/target/release/gtop-history-bench replay-sparse copied-history.json /private/tmp/history-sparse 60
rtk proxy python3 tools/history-bench/audit-replay.py sparse copied-history.json /private/tmp/history-sparse 60
```

The dense replay uses every process from the first source minute in every minute. It keeps source work and peaks and sets system coverage to 60 seconds. The sparse replay repeats all source minutes and gaps. Both start at minute 100800. These integer widths can produce smaller files than current timestamps. The auditor checks retained work, recorded time, hourly values, zero-row presence, and representative process rows. Each replay is one file-size observation, not a speed benchmark. An existing replay directory continues after its last stored minute. For a fresh directory, the auditor uses the stated total day count. For a continuation, pass `--seed <original-seed.sqlite>` and the number of advancing days. The seed must stay unchanged.

Compare storage candidates on copies of a completed workload database.

```sh
rtk proxy python3 tools/history-bench/size-levers.py /private/tmp/history-dense/history.sqlite /private/tmp/history-size-candidates
```

This command expects the old row-based schema. It compares the original vacuumed file, numeric affinity, index removal, packed zero presence, memory scaling, and page sizes. It checks every retained nonzero process row in both directions. The candidates use Python SQLite. Verify accepted choices with the bundled production engine and a production replay. Rebuilding rows also changes page layout, so this comparison does not measure steady-state fragmentation.
