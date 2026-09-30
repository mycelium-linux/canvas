#!/bin/sh
set -eu

cd "$(dirname "$0")/.."

# Cargo owns where artifacts land: CARGO_TARGET_DIR, or build.target-dir in a cargo
# config, can point it away from ./target. Ask cargo rather than assume.
target=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
test "$(uname -s)" = Darwin || {
  printf '%s\n' 'Dock Canvas app packaging requires macOS' >&2
  exit 1
}

binary=${1:-$target/debug/canvas}
helper=${2:-$(dirname "$binary")/canvas-mac}
test -x "$binary" || {
  printf '%s\n' 'Build canvas before packaging or pass its binary path' >&2
  exit 1
}
test -x "$helper" || {
  printf '%s\n' 'Build canvas-mac before packaging or pass its binary path as argument 2' >&2
  exit 1
}

app=$target/macos/DockCanvas.app
mkdir -p "$app/Contents/MacOS"
cp -f "$binary" "$app/Contents/MacOS/dock-canvas"
cp -f "$helper" "$app/Contents/MacOS/canvas-resources"
cp -f crates/canvas/macos/Info.plist "$app/Contents/Info.plist"
codesign --force --sign - --identifier dev.fpl.dock.canvas "$app"
printf '%s\n' "$PWD/$app"
