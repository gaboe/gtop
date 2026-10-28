# gtop scripts

## Performance telemetry report

Run the performance telemetry report with a copy of the app database.

```sh
rtk proxy python3 crates/gtop/scripts/perf-report.py "$HOME/Library/Application Support/gtop/perf.sqlite"
rtk proxy python3 -m unittest discover -s crates/gtop/scripts -p 'test_perf_report.py' -v
```

The report opens the source database read-only, copies it to a temporary SQLite file, and reads the copy. It prints median, p90, and p95 CPU use of one core for each UTC hour, app version, window state, and mean-load bucket. It also prints draws per minute, mean draw time, sampler runs per minute, mean sampler time, and mean whole-machine CPU busy percent. Rows without load data use the `unknown` bucket.

## Full Monitor draw benchmark

Run on macOS from the repository root. Fetch pinned dependencies before the first run.

```sh
rtk proxy cargo fetch --locked
rtk proxy python3 crates/gtop/scripts/draw-benchmark.py --mode validate
rtk proxy python3 crates/gtop/scripts/draw-benchmark.py --mode clean
rtk proxy python3 crates/gtop/scripts/draw-benchmark.py --mode clean --interactions
```

No native window opens. The release profile stays at opt-level 3, fat LTO, one codegen unit, and no incremental compilation. Each state gets five warmup draws and 31 measured draws. Use `--samples 5` for a short run. Use `--debug` only to check the tool.

The fixture has 1000 processes, 14 cores, a 1480x960 viewport, populated history and process detail. It adds 1,000,000 to each synthetic PID and rejects any match to the test process. This keeps the gtop cost readout constant across runs. Sixteen processes share the supplied per-core CPU total. The remaining rows are idle. History rows use production sorting and leader rules. The fixture retains the single-bank topology and unavailable-GPU branch from test data. It does not cover every hardware layout.

Select the realistic smaller control with these variables.

```sh
rtk proxy env GTOP_DRAW_PROCESSES=100 GTOP_DRAW_CORES=4 GTOP_DRAW_WIDTH=1200 GTOP_DRAW_HEIGHT=720 \
  python3 crates/gtop/scripts/draw-benchmark.py --samples 5 --mode clean
```

Process counts must be 1 through 100000. Core counts must be 1 through 256. Viewport dimensions must be 1 through 16384. Samples must be 5 through 100000. Zero, malformed input and narrowing overflow fail before fixture construction. A viewport below 1200x720 is a synthetic sanity check only. Set `GTOP_DRAW_VIEWS=live` for live-only runs. Other names are `history1h`, `history24h`, and `detail`.

The timer measures Darwin thread CPU around `Window::draw`. The test forces a full refresh and asserts one Monitor render inside each timed draw. Arena cleanup, scene observation, fixture construction, presentation and GPU execution stay outside timing. AppleTextSystem and bundled fonts provide production text shaping. Collectors and settings writers stay inactive.

Clean mode disables AnyElement increments and per-frame scene formatting. It retains a thread-local enabled check per AnyElement allocation, one counter reset per draw, two timer calls, a disabled interaction-timer check, and one Monitor render-counter increment. The copied GPUI records debug selector bounds in release builds so interactions can target the rendered controls. Both modes disable the existing unit-glyph test bounds recorder. Validation mode records work counts and scene digests after each measured draw. Use clean mode for performance claims. Alternate clean and validation runs to measure observer effects. Changing runtime modes does not rebuild dependencies.

Interaction mode uses the same fixture at 1480x960 and the test platform's scale factor of 2. It visits Live, Detail, 24-hour history, and the palette panel. It sends stationary moves, row and header moves, button enter and leave, wheel scroll, row clicks, and text selection drags. The `events` field counts interaction steps. A wheel step sends a move and a wheel event. A row-click step sends a move, press, and release. Views that do not support an action skip it.

The `notify` action sends one Monitor notification per sample without forcing a full window refresh. It measures a parent-only notification. Assertions require one draw and one Monitor render per notification, with the same scene before and after. This separates regular redraws from the full-refresh test, which deliberately disables GPUI view caches.

The interaction timer runs inside `Window::draw`. It counts draws that GPUI's test context performs while it flushes event effects. `draws` counts actual frames. `renders` counts Monitor renders. `draw_cpu_ms` excludes arena cleanup. `event_cpu_ms` measures the remaining thread CPU for the event step, including effects and arena cleanup. Scene hashes run once before and after each action, outside the timed steps. Assertions require scroll to change the scene, row clicks to open Detail, and a drag to select text. No native window opens.

The script copies pinned GPUI into ignored `target/draw-benchmark`. It checks all upstream bytes, its own bytes and all generated copy bytes. A damaged copy is reconstructed. Registry files stay unchanged. A script-private cfg enables access to copied instrumentation methods. Stock Cargo features do not depend on those methods.

Script invocations serialize before preparation and lockfile snapshots. Keep exclusive Cargo use in this checkout. Other Cargo tools do not acquire the script lock. The script restores Cargo.lock only when its bytes equal the snapshot or the exact expected removal of the pinned GPUI registry source and checksum. An unexpected write remains on disk and fails the command. Abrupt process termination can leave the temporary lock patch. Run stock Cargo to restore registry resolution before another benchmark run.

Scene equality covers every primitive vector in draw order. Pinned scene structs derive Debug for all fields. Background has a custom incomplete Debug, so the digest also includes its explicit tag, color space, solid color, gradient angle or pattern height, and both color stops. Nested Hsla, LinearColorStop, vertices, masks, transforms, edges, corners and atlas tiles use derived Debug. Size Debug includes both dimensions. ScaledPixels and DevicePixels Debug include their full scalar values. Padding has no rendering meaning. Surface content is unsupported and causes an assertion failure.

The copied HeadlessAtlasBackend hashes raster bytes at insertion. Scene digests include the content digest of each used monochrome, subpixel and polychrome tile. The validation pass tests same-size zero versus 255 atlas bytes and a gradient color-space mutation. Both must change the scene digest. This proves equality of scene construction and raster inputs for this pinned source, not GPU pixel equality. An equivalent rendering with a changed primitive representation can have a different digest. The digest uses Rust DefaultHasher and is scoped to this compiler and pinned dependency set.

Save complete validation output as a reference before optimization. Compare `scene` lines across independent runs at both scales. Save raw CPU samples and successful draw counts. Alternate at least five runs per side for sensitivity and observer calibration. Report median and range. A gap inside run variation is inconclusive. A busy machine can affect thread CPU through frequency and cache contention.

Run isolated state probes with this command.

```sh
rtk proxy python3 -m unittest discover -s crates/gtop/scripts -p 'test_draw_benchmark.py' -v
```

For a separate symbolized profile, use `--symbols`. This adds debuginfo and retains symbols through `RUSTFLAGS`, including dependency symbols. All optimization settings stay unchanged. Sample only the owned test process. Discard profiled timings.

`notify` only notifies Monitor. `live_sample` accepts a fixed snapshot through
`Monitor::accept`, reorders Processes, and notifies both entities. It asserts one
Processes render per sample. Only `live_sample` represents an accepted live sample.

The ignored `warm_dot_cache_transitions` test uses the same instrumented binary.
It compares warm scenes with a fresh dot-view cache and `window.refresh()` after
colour edits, palette switches, resize, and scale changes.
