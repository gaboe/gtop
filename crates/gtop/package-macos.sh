#!/bin/sh
set -eu

gtop_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$gtop_root"
cargo build --release -p gtop

gtop_target=${CARGO_TARGET_DIR:-"$gtop_root/target"}
gtop_bundle="$gtop_target/release/bundle/macos/gtop.app"
mkdir -p "$gtop_bundle/Contents/MacOS" "$gtop_bundle/Contents/Resources"
cp "$gtop_target/release/gtop" "$gtop_bundle/Contents/MacOS/gtop.new"
mv "$gtop_bundle/Contents/MacOS/gtop.new" "$gtop_bundle/Contents/MacOS/gtop"
cp crates/gtop/Info.plist "$gtop_bundle/Contents/Info.plist"
cp crates/gtop/assets/icon.icns "$gtop_bundle/Contents/Resources/icon.icns"
codesign --force --deep --sign - "$gtop_bundle"
codesign --verify --deep --strict "$gtop_bundle"
printf '%s\n' "$gtop_bundle"
