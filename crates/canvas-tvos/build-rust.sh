#!/bin/sh
set -eu
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
case "${PLATFORM_NAME:?Xcode PLATFORM_NAME required}" in
  appletvos) task_arch=aarch64-apple-tvos ;;
  appletvsimulator) task_arch=aarch64-apple-tvos-sim ;;
  *) printf '%s\n' 'Unsupported Apple platform' >&2; exit 1 ;;
esac
task_target="${CANVAS_RUST_TARGET_DIR:-$(pwd)/../../target/canvas-tvos}"
if ! rustup target list --installed | awk -v expected="$task_arch" '$0 == expected { found=1 } END { exit !found }'; then
  printf 'Install Rust target explicitly: rustup target add %s\n' "$task_arch" >&2
  exit 1
fi
require_space() {
  task_probe="$1"
  while [ ! -d "$task_probe" ]; do task_probe="$(dirname "$task_probe")"; done
  task_free_kib="$(df -k "$task_probe" | awk 'NR==2 {print $4}')"
  if [ "$task_free_kib" -lt 1048576 ]; then
    printf 'At least 1 GiB free required on build volume: %s. No automatic cleanup.\n' "$task_probe" >&2
    exit 1
  fi
}
require_space "$task_target"
require_space "${BUILT_PRODUCTS_DIR:?Xcode products directory required}"
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo build --locked --offline \
  --jobs 1 --target "$task_arch" --target-dir "$task_target"
mkdir -p "$BUILT_PRODUCTS_DIR"
cp -f "$task_target/$task_arch/debug/libcanvas_tvos.a" "$BUILT_PRODUCTS_DIR/libcanvas_tvos.a"
xcrun swift apple/GenerateArtwork.swift apple/Assets.xcassets
