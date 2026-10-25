# Full Monitor draw benchmark

Run on macOS from the repository root. Fetch pinned dependencies before the first run.

```sh
rtk proxy cargo fetch --locked
rtk proxy python3 crates/gtop/scripts/draw-benchmark.py --mode validate
rtk proxy python3 crates/gtop/scripts/draw-benchmark.py --mode clean
```

No native window opens. The release profile stays at opt-level 3, fat LTO, one codegen unit, and no incremental compilation. Each state gets five warmup draws and 31 measured draws. Use `--samples 5` for a short run. Use `--debug` only to check the tool.

The fixture has 1000 processes, 14 cores, a 1480x960 viewport, populated history and process detail. Sixteen processes share the supplied per-core CPU total. The remaining rows are idle. History rows use production sorting and leader rules. The fixture retains the single-bank topology and unavailable-GPU branch from test data. It does not cover every hardware layout.

Select the realistic smaller control with these variables.

```sh
rtk proxy env GTOP_DRAW_PROCESSES=100 GTOP_DRAW_CORES=4 GTOP_DRAW_WIDTH=1200 GTOP_DRAW_HEIGHT=720 \
  python3 crates/gtop/scripts/draw-benchmark.py --samples 5 --mode clean
```

Process counts must be 1 through 100000. Core counts must be 1 through 256. Viewport dimensions must be 1 through 16384. Samples must be 5 through 100000. Zero, malformed input and narrowing overflow fail before fixture construction. A viewport below 1200x720 is a synthetic sanity check only. Set `GTOP_DRAW_VIEWS=live` for live-only runs. Other names are `history1h`, `history24h`, and `detail`.

The timer measures Darwin thread CPU around `Window::draw`. The test forces a full refresh and asserts one Monitor render inside each timed draw. Arena cleanup, scene observation, fixture construction, presentation and GPU execution stay outside timing. AppleTextSystem and bundled fonts provide production text shaping. Collectors and settings writers stay inactive.

Clean mode disables AnyElement increments and per-frame scene formatting. It retains a thread-local enabled check per AnyElement allocation, one counter reset per draw, two timer calls and one Monitor render-counter increment. Both modes disable the existing unit-glyph test bounds recorder. Validation mode records work counts and scene digests after each measured draw. Use clean mode for performance claims. Alternate clean and validation runs to measure observer effects. Changing runtime modes does not rebuild dependencies.

The script copies pinned GPUI into ignored `target/draw-benchmark`. It checks all upstream bytes, its own bytes and all generated copy bytes. A damaged copy is reconstructed. Registry files stay unchanged. A script-private cfg enables access to copied instrumentation methods. Stock Cargo features do not depend on those methods.

Script invocations serialize before preparation and lockfile snapshots. Keep exclusive Cargo use in this checkout. Other Cargo tools do not acquire the script lock. The script restores Cargo.lock only when its bytes equal the snapshot or the exact expected removal of the pinned GPUI registry source and checksum. An unexpected write remains on disk and fails the command. Abrupt process termination can leave the temporary lock patch. Run stock Cargo to restore registry resolution before another benchmark run.

Scene equality covers every primitive vector in draw order. Pinned scene structs derive Debug for all fields. Background has a custom incomplete Debug, so the digest also includes its explicit tag, color space, solid color, gradient angle or pattern height, and both color stops. Nested Hsla, LinearColorStop, vertices, masks, transforms, edges, corners and atlas tiles use derived Debug. Size Debug includes both dimensions. ScaledPixels and DevicePixels Debug include their full scalar values. Padding has no rendering meaning. Surface content is unsupported and causes an assertion failure.

The copied HeadlessAtlasBackend hashes raster bytes at insertion. Scene digests include the content digest of each used monochrome, subpixel and polychrome tile. The validation pass tests same-size zero versus 255 atlas bytes and a gradient color-space mutation. Both must change the scene digest. This proves equality of scene construction and raster inputs for this pinned source, not GPU pixel equality. An equivalent rendering with a changed primitive representation can have a different digest. The digest uses Rust DefaultHasher and is scoped to this compiler and pinned dependency set.

Save complete validation output as a reference before optimization. Compare `scene` lines across independent runs at both scales. Save raw CPU samples and successful draw counts. Alternate at least five runs per side for sensitivity and observer calibration. Report median and range. A gap inside run variation is inconclusive. A busy machine can affect thread CPU through frequency and cache contention.

Run isolated state probes with this command.

```sh
rtk proxy python3 -m unittest discover -s crates/gtop/scripts -p 'test_draw_benchmark.py' -v
```

For a separate symbolized profile, use `--symbols`. This adds debuginfo and retains symbols for the final test executable. Dependency line tables may be absent. All optimization settings stay unchanged. Sample only the owned test process. Discard profiled timings.
