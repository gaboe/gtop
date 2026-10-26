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

Click a table header to sort. Click it again to reverse the order. Search accepts names, commands, PID substrings, and comma-separated alternatives. Use `:3000` or `port:3000` to find a local bound port. Port collection runs only while the search contains a valid port term.

Select a process to show its command, root directory, resources, parent, and children. Load environment variables on demand. The kill action requires a second confirmation. It checks the process start time before sending the signal.

Choose a refresh interval from 1, 2, 3, 5, or 10 seconds. The default is 3 seconds. Pause stops sampling. GPUI visibility events also stop sampling when the window is minimized, hidden, fully covered, or on a sleeping display. One snapshot that already started can finish after the window becomes hidden.

The SSD panel shows system-volume storage in decimal GB. Used space is total capacity minus available capacity. On macOS, available capacity includes space the system can purge, such as caches, through `NSURLVolumeAvailableCapacityForImportantUsageKey`. The APFS volumes share the container capacity. Storage refreshes every 60 seconds while sampling runs.

To log sampling and visible UI ticks, run:

```sh
NEOHTOP_PERF=1 cargo run --release -p gtop
```

The app pins `gpui-pre` and its platform crate to `0.3.9`. It pins `gpui-component` to `0.8.0`, which requires that exact GPUI version. The component table virtualizes rows and columns. Search and sorting run when input or a snapshot changes. They do not run during row rendering.

On macOS, `font-kit` enables native text. The app compiles Metal shaders at startup through GPUI's `runtime_shaders` feature. This avoids a build dependency on Apple's optional Metal Toolchain. The first window can take longer to appear.

Run the focused checks from the repository root:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```
