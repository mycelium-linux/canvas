#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
test "$(uname -s)" = Darwin || {
  printf '%s\n' 'Dock Canvas app packaging requires macOS' >&2
  exit 1
}

binary=${1:-target/debug/dock-canvas}
test -x "$binary" || {
  printf '%s\n' 'Build dock-canvas before packaging or pass its binary path' >&2
  exit 1
}

app=target/macos/DockCanvas.app
mkdir -p "$app/Contents/MacOS"
cp -f "$binary" "$app/Contents/MacOS/dock-canvas"
cp -f crates/dock-canvas/macos/Info.plist "$app/Contents/Info.plist"
codesign --force --sign - --identifier dev.fpl.dock.canvas "$app"
printf '%s\n' "$PWD/$app"
