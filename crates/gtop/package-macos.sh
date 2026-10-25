#!/bin/sh
set -eu

gtop_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$gtop_root"
cargo_output=$(mktemp)
trap 'rm -f "$cargo_output"' EXIT HUP INT TERM
cargo build --release -p gtop --message-format=json-render-diagnostics >"$cargo_output"

gtop_target=${CARGO_TARGET_DIR:-"$gtop_root/target"}
gtop_executable=$(python3 - "$cargo_output" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as cargo_output:
    for line in cargo_output:
        message = json.loads(line)
        target = message.get("target", {})
        executable = message.get("executable")
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "gtop"
            and "bin" in target.get("kind", [])
            and executable
        ):
            print(executable)
            break
    else:
        raise SystemExit("Cargo did not report the gtop executable")
PY
)

gtop_bundle="$gtop_target/gtop-native/gtop.app"
rm -rf "$gtop_bundle"
mkdir -p "$gtop_bundle/Contents/MacOS" "$gtop_bundle/Contents/Resources/licenses"
cp "$gtop_executable" "$gtop_bundle/Contents/MacOS/gtop.new"
mv "$gtop_bundle/Contents/MacOS/gtop.new" "$gtop_bundle/Contents/MacOS/gtop"
cp crates/gtop/Info.plist "$gtop_bundle/Contents/Info.plist"
cp crates/gtop/assets/icon.icns "$gtop_bundle/Contents/Resources/icon.icns"
cp LICENSE "$gtop_bundle/Contents/Resources/licenses/gtop-LICENSE"

gtop_cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
gtop_registry=$(find "$gtop_cargo_home/registry/src" -type d -name 'gpui-pre-0.3.9' -print -quit)
gtop_assets_registry=$(find "$gtop_cargo_home/registry/src" -type d -name 'gpui-kit-assets-0.8.0' -print -quit)
if [ -z "$gtop_registry" ] || [ -z "$gtop_assets_registry" ]; then
  printf '%s\n' 'Could not find the pinned GPUI license files in the Cargo registry.' >&2
  exit 1
fi
cp "$gtop_registry/LICENSE-APACHE" "$gtop_bundle/Contents/Resources/licenses/GPUI-LICENSE-APACHE"
cp "$gtop_assets_registry/LICENSE-LUCIDE" "$gtop_bundle/Contents/Resources/licenses/Lucide-Feather-NOTICE"

cp crates/gtop/assets/fonts/*-OFL.txt "$gtop_bundle/Contents/Resources/licenses/"
codesign --force --deep --sign - "$gtop_bundle"
codesign --verify --deep --strict "$gtop_bundle"
printf '%s\n' "$gtop_bundle"
