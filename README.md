# gtop

gtop is a native GPUI system monitor. It shows processes, CPU, memory, GPU, temperature, network, storage, and local ports.

![gtop screenshot](screenshot.png)

## Build the macOS app

Install stable Rust and the Xcode Command Line Tools. Then run:

```sh
sh crates/gtop/package-macos.sh
```

The script creates `target/gtop-native/gtop.app` and verifies its code signature. It does not install the app in `/Applications`.

## Run from source

```sh
cargo run --release -p gtop
```

Search processes by name, command, PID, or port. Use `:3000` or `port:443` to find a local port. A bare number matches a PID.

## Check the workspace

```sh
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
