# Canvas Apple TV build spike

Independent tvOS packaging with a small approved native host. Rust owns the
existing `canvas-workspace::endpoint::Endpoint` state machine, revision checks,
catalog admission and workspace switching. The analog clock and top-bar geometry
are the canonical `fpl-gfx` retained primitives, using its shared Undergrowth
palette. UIKit owns remote focus, lifecycle and a CoreGraphics raster adapter.
AVPlayer owns the optional public Apple HLS sample and TV-local audio.

This is not a Metal/wGPU qualification, network-control integration, hardware
test or TestFlight release. The bundled Home/Studio catalog is an explicit local
fixture; it is not a discovered or synchronized fleet catalog. Both selections
currently render the clock. Injected themes and full workspace surface rendering
are not wired. No background service, downloaded executable plugin, private
Apple API or subprocess is used.

## Build and validation

Requires macOS, Xcode's tvOS SDK and Rust's ARM64 simulator target:

```sh
rustup target add aarch64-apple-tvos-sim
cargo check --offline --manifest-path crates/canvas-tvos/Cargo.toml \
  --target-dir target/canvas-tvos --jobs 1
cargo test --offline --manifest-path crates/canvas-tvos/Cargo.toml \
  --target-dir target/canvas-tvos --jobs 1
sh crates/canvas-tvos/build-simulator.sh
```

The script builds `crates/canvas-tvos/build/CanvasTV.app` with SDK tools, not an
Xcode account or provisioning profile. Ad-hoc signing is for simulator use only.
It does not download runtimes, boot a simulator, install to a TV, upload to Apple,
or replace any existing Canvas/capture configuration. The independent Cargo lock
pins this packaging slice without editing the desktop lockfile.

## Launch when a tvOS runtime is already installed

```sh
xcrun simctl list devices available
xcrun simctl boot APPLE_TV_SIMULATOR_UUID
xcrun simctl bootstatus APPLE_TV_SIMULATOR_UUID -b
xcrun simctl install APPLE_TV_SIMULATOR_UUID crates/canvas-tvos/build/CanvasTV.app
xcrun simctl launch APPLE_TV_SIMULATOR_UUID dev.fpl.canvas.tvos.spike
open -a Simulator
```

Choose a tvOS device, not an iPhone simulator. When no tvOS runtime is installed,
SDK compilation can still succeed but this app cannot launch. Runtime downloads
can be several GiB; do not automatically install one on a disk-constrained host.
To show it on a physical TV, use the existing approved per-window simulator
capture pipeline after explicitly selecting this simulator window. No TV stream
is created by this spike.

Initial build evidence on Neo: native Rust check and four tests passed; the SDK
build produced a 27 MiB ARM64 Mach-O app with `TVOSSIMULATOR` platform,
tvOS 18.0 minimum, SDK 26.5, and a verified ad-hoc signature. Only the iOS 26.5
runtime is currently installed, so no simulator launch, on-screen capture, video
playback or physical Apple TV behavior has been verified.

Home/Studio buttons inspect the shared endpoint revision before switching.
Video starts a public Apple HLS sample only when selected; returning to a
workspace pauses it. Network playback errors appear in the bottom status line.
Rendering timers stop and video pauses when the app leaves the foreground.

## ABI and distribution boundary

`apple/Canvas.h` defines ABI version 1 and a 64 KiB request/response limit.
Callers supply readable input and writable, nonoverlapping output; Rust never
retains pointers or transfers allocation ownership. Return values -1/-2/-3 mean
invalid pointer/bounds, insufficient capacity and caught panic respectively.
Use the full 64 KiB output buffer: execution occurs before response copying, so a
capacity failure is not permission to retry a mutating command blindly. Shared
endpoint revision checks reject stale mutations. Transport authentication is
not provided by this in-process host ABI.

TestFlight requires a real tvOS Xcode archive/app record, Apple Developer team
and distribution signing, icons/metadata, privacy/export compliance answers and
applicable beta review. No team/account is assumed from an unrelated iOS app.
Beta builds expire after 90 days. Physical Apple TV model, provisioning access,
runtime availability and authenticated outbound Unibus integration remain
separate acceptance gates tracked in Beads `dock-cys1`.
