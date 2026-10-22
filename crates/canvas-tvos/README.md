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
separate acceptance gates tracked in Beads `dock-dusv` (SDK spike `dock-cys1`).

## Device archive and TestFlight export

The shared `CanvasTV` Xcode scheme now targets real Apple TV devices as well as
simulators. Device identity is **dev.fpl.canvas.tvos**, separate from the manual
simulator spike and unrelated iOS apps. Apple team is **8B3GZ9UEGE**; the
registered App Store Connect tvOS app is **6820884143**, named **fungOS Canvas**.
These are public routing identifiers, not signing credentials.

```sh
rustup target add aarch64-apple-tvos
cargo fetch --locked --manifest-path crates/canvas-tvos/Cargo.toml
sh crates/canvas-tvos/archive-device.sh --unsigned  # compile/archive validation
sh crates/canvas-tvos/archive-device.sh             # configured signing required
sh crates/canvas-tvos/export-testflight.sh          # local IPA export, not upload
```

Unsigned archives are not installable or distributable. Do not export or upload
one as a successful release. `build/CanvasTV.xcarchive` contains the archive;
`build/testflight` is the local export directory. Xcode uses the explicit device
target `aarch64-apple-tvos`; simulator builds use `aarch64-apple-tvos-sim`. Rust
static libraries are copied into that build's products directory, avoiding
device/simulator ABI mixing. Build number can be overridden with
`CURRENT_PROJECT_VERSION=NUMBER` on the archive command. A signed distribution
archive/profile and current App Store Connect permissions are required to export.
Scripts do not silently create profiles, download runtimes, or upload binaries.
An active **fungOS Canvas tvOS App Store** provisioning profile was registered
for this app using the existing distribution certificate. Its credentials have
not been downloaded/imported, and no signed archive or export is verified.

The Xcode pre-build phase uses one Cargo job, debug information disabled, and
checks for at least 1 GiB free before compilation. It requires installed Rust
targets and offline dependencies. Set `CANVAS_RUST_TARGET_DIR` to an absolute
build-cache path when using another volume. No automatic cleanup is performed.
Space checks resolve the existing parent of the Rust cache and Xcode products
locations, including external volumes. The spike intentionally links debug-profile
Rust with debug information disabled even for the Release Xcode archive; Swift
is optimized. Rust release optimization and end-to-end frame performance remain
unqualified, not implied by the Xcode configuration name.
Original vector icon artwork is generated deterministically using Apple's
native graphics API: layered small/large Apple TV icons and a top-shelf image.
The generated catalog artwork is excluded from git; its generator is source.

Device archive attempt on Neo was blocked **before compilation** by incomplete
Xcode platform support: the generic tvOS destination reports “tvOS 26.5 is not
installed,” despite the SDK being present. Asset compilation also requires an
available Apple TV simulator runtime. Xcode project/scheme and plist/script
parsing passed; generated artwork exists. No tvOS runtime/platform download was
started on the disk-constrained host. Install the tvOS platform component on a
Mac with adequate space, then rerun the unsigned archive gate before signing.
This packaging slice does not claim a successful device archive or TestFlight
availability yet.
