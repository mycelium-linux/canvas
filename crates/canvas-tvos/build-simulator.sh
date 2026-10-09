#!/bin/sh
set -eu
# Build only. No runtime download, boot, install, capture replacement or upload.
cd "$(dirname "$0")"
task_target="${CARGO_TARGET_DIR:-../../target/canvas-tvos}"
task_sdk="$(xcrun --sdk appletvsimulator --show-sdk-path)"
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo build --offline --jobs 1 \
  --target aarch64-apple-tvos-sim --target-dir "$task_target"
mkdir -p build/CanvasTV.app
cp -f apple/Info.plist build/CanvasTV.app/Info.plist
xcrun --sdk appletvsimulator swiftc -sdk "$task_sdk" \
  -target arm64-apple-tvos18.0-simulator -parse-as-library \
  -import-objc-header apple/Canvas.h apple/Main.swift \
  -L "$task_target/aarch64-apple-tvos-sim/debug" -lcanvas_tvos \
  -framework UIKit -framework AVFoundation -framework Security \
  -o build/CanvasTV.app/CanvasTV
codesign --force --sign - build/CanvasTV.app
printf '%s\n' "Built: $(pwd)/build/CanvasTV.app"
