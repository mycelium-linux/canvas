# Home Pi render profile — 2026-10-05

The TV is GPU-bound, with a major cost in the full-screen **iced theme gradient**.
This is not primarily CPU load, the native compositor, or the new clock alone.
No production rendering defaults were changed by this investigation.

## Live measurements

Raspberry Pi 4 Model B, V3D 4.2, Vulkan/Mesa V3DV, physical output
1920×1080 at 60 Hz. Release Canvas, configured clock cadence 25 Hz.
Each kernel trace is approximately ten seconds, using isolated named trace
instances; complete reports were analyzed on the Pi, not truncated tool output.

| UI condition | Display flips/sec | p95 flip interval | Canvas render-job p95 | Render queue-wait p95 |
| --- | ---: | ---: | ---: | ---: |
| Empty desktop, theme gradient | 11.26 | 200.01 ms | 91.42 ms | 159.73 ms |
| GPU clock, theme gradient | 8.66 | 283.34 ms | 128.66 ms | 234.54 ms |
| Empty desktop, solid background | 27.69 | 50.01 ms | 13.40 ms | 21.46 ms |
| GPU clock, solid background | 18.66 | 100.01 ms | 37.97 ms | 53.61 ms |
| GPU clock, gradient restored (repeat) | 8.48 | 283.33 ms | 128.68 ms | 234.10 ms |
| Empty desktop, no-antialiasing diagnostic build | 11.40 | 200.02 ms | 91.52 ms | 160.73 ms |
| GPU clock, no-antialiasing diagnostic build | 8.40 | 283.34 ms | 128.74 ms | 236.54 ms |
| Legacy clock, theme gradient | 7.08 | 283.35 ms | 116.08 ms | 260.31 ms |

These are **DRM display flips, not unique app FPS**: duplicate content can be
presented. GPU-job execution is kernel scheduler run→done elapsed time, not a
GPU timestamp query, individual shader-pass duration, or whole-frame latency.
The table uses p95 across render jobs (including small upload jobs), not a
per-frame histogram. Do not add job percentiles together.

The compositor's own render jobs were roughly 5–6 ms; its queue waits reached
249 ms with the clock/gradient. Canvas was holding up the shared GPU queue.
The GPU clock was observed at 500 MHz and temperature at about 73.5°C.
`vcgencmd` could not open `/dev/vcio`, so this does **not** certify absence of
throttling. No power, frequency, driver, or compositor settings were changed.

Disabling the UI's mandatory antialiasing did not meaningfully improve the
live result. Reverting to the legacy clock did not cure the bottleneck either.
The solid-background comparison used the existing revision-bound Canvas patch
API on the same production binary; the theme backgrounds and `qemu` workspace
were restored afterward.

## Isolated gfx measurements

The standalone benchmark uses the canonical `fpl-gfx::AnalogClock` scene,
flat material tokens, a BGRA8 sRGB render target, and waits for each submission
to complete. Canvas was temporarily hidden to reduce contention. The compositor
remained running. Each condition has five warmup iterations and 25 samples.

| Scenario | 1080p, 1 sample: p50 / p95 | 720p, 1 sample: p50 / p95 | 1080p, 4 samples: p50 / p95 |
| --- | ---: | ---: | ---: |
| Clear only | 5.05 / 7.93 ms | 2.58 / 3.48 ms | 12.18 / 14.49 ms |
| Clock face | 13.12 / 15.84 ms | 6.78 / 7.90 ms | 18.79 / 20.49 ms |
| 60 ticks | 5.62 / 6.89 ms | 3.80 / 5.07 ms | 12.96 / 14.79 ms |
| Four hands/center primitives | 7.82 / 9.40 ms | 4.41 / 5.27 ms | 14.54 / 16.06 ms |
| Complete 65-primitive clock | 18.97 / 21.03 ms | 10.18 / 13.81 ms | 24.09 / 26.45 ms |

These are CPU-encoding + submission + GPU-completion elapsed times, **not pure
GPU time or presentation latency**. No readback or MSAA resolve is measured.
The standalone viewport/material setup is not an exact reproduction of the
live widget's size, framing, and surrounding UI. It isolates components rather
than predicting app FPS. CPU encode p50 was approximately 0.3–0.6 ms.

## Next implementation slice

Tracked as `dock-o5b`, rather than changing defaults during profiling:

1. Retain/cache the exact theme gradient instead of rerasterizing it every tick.
   Derive invalidation from theme colors/stops, viewport, format, and scale.
   Keep Coherence/gfx as the theme source; do not duplicate palette ownership.
2. Add actual frame/submission/presentation timing and bound in-flight GPU work
   so slow frames cannot accumulate into hundreds of milliseconds of latency.
3. Reprofile the live clock and surrounding UI. Its remaining 38 ms heavy jobs
   on solid backgrounds mean this investigation does not establish 25 or 60 FPS.

The analog clock appearance should be preserved. A permanently flat background,
blind clock-renderer rollback, or global antialiasing removal is not the proposed
fix.

## Evidence and reproduction

Checked-in JSON summaries: `docs/perf/home-pi-20261005/`.
Raw traces and diagnostic binaries: Home Pi
`/home/ajm/.cache/canvas-perf-20261005/`.
Build tools/sources: Agora
`/home/ajmwagar/canvas-coherence-tv.7KZV9d/perf/`.

Compile the dependency-free trace analyzer on Agora:

```sh
rustc --edition 2024 --test tools/gpu-trace.rs -o /tmp/gpu-trace-tests
/tmp/gpu-trace-tests
rustc --edition 2024 -O --target aarch64-unknown-linux-gnu \
  -C linker=aarch64-linux-gnu-gcc tools/gpu-trace.rs -o /tmp/gpu-trace
```

Record on the Pi using a new, task-owned trace instance and validated artifact
directory. Never reset another user's instance or the global tracer:

```sh
sudo trace-cmd record -B canvas-perf-example \
  -o /home/ajm/.cache/canvas-perf-20261005/example.dat \
  -e v3d -e gpu_scheduler \
  -e drm:drm_vblank_event_queued -e drm:drm_vblank_event_delivered sleep 10
sudo trace-cmd report -i /home/ajm/.cache/canvas-perf-20261005/example.dat \
  | /home/ajm/.cache/canvas-perf-20261005/gpu-trace
sudo trace-cmd reset -B canvas-perf-example -d
```

Map scheduler `client_id` to the running Canvas/compositor's DRM fdinfo;
IDs change when Canvas restarts. Do not reuse this run's client IDs blindly.
The analyzer rejects explicit lost/truncated reports and incomplete jobs are
excluded. Its percentiles use sorted index `n/2`, `n*95/100`, and `n*99/100`,
bounded to `n-1`.

For gfx, the independent locked crate does not change the production workspace:

```sh
cd tools/gfx-profile
cargo check --locked
cargo test --locked
cargo clippy --locked -- -D warnings
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --locked --release --target aarch64-unknown-linux-gnu
# On the Pi; hide/show Canvas with a cleanup trap around the samples:
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/broadcom_icd.json \
  ./canvas-gfx-profile 1920 1080 1
```

Trace analyzer: three tests passed. Gfx benchmark: check, one test, strict
Clippy, and ARM release build passed on Agora. The gfx additive read-only scene
iterator used by the benchmark passed the existing 34 gfx library tests.

## Separate workspace compatibility finding

The older TV compatibility build rejects newer `pages`/`active_page` fields in
the original saved workspace store. The original 24-workspace file is intact at
`/home/ajm/.local/state/dock/canvas/workspaces.json.before-coherence-20261005`.
The current compatible store has `dock`, `coherence-preview`, and `qemu`.
Tracked separately as `dock-0jc`: restore with schema-compatible code, preserving
subsequent edits. Do not strip unknown fields, overwrite the backup, or claim
that all original workspaces were restored by this performance investigation.

## Retained theme cache rollout

The theme ground now rasterizes to a GPU texture on a cache miss and samples
that texture inside iced's existing compositor pass thereafter. This applies
to `Background::Theme` and the theme fallback for unsupported background kinds;
explicit user gradients and animated shader backgrounds retain their existing
behavior. The analog clock shader and Coherence palette mapping are unchanged.

Cache keys include iced's canonical packed gradient (including its color-space,
f16 packing, and angle geometry), physical extent, and display-scale bits.
Resources belong to the renderer/device and each window's retained widget.
Disposed widgets age out; cache capacity is 16 window entries. The grace period
accounts for iced trimming shared storage after each window's presentation,
so four live windows do not evict one another every frame.

At the same 1920×1080 size, warm ten-second traces measured:

| View | Before: flips/sec / render-job p95 | Cached: flips/sec / render-job p95 | Cached flip-interval p95 |
| --- | ---: | ---: | ---: |
| Clock (restart-to-restart baseline) | 8.48 / 128.65 ms | 17.90 / 42.71 ms | 116.66 ms |
| Empty desktop | 11.26 / 91.42 ms | 28.50 / 17.61 ms | 50.00 ms |

Final evidence uses `cached-*-layout-summary.json`, with the clock baseline in
`gradient-restarted-summary.json`. An initial capture differed in content size
from the pre-restart UI. To avoid any sizing-base change, the final version uses
`Stack::push_under` beneath the existing outer container. A fresh restart of the
original binary confirmed the same clock size/framing as the final cached build,
isolating startup layout state from the cache comparison. The earlier
`cached-*-summary.json` files remain preliminary evidence, not used above.

The log showed one 1920×1080 cache miss over the measured run, not one per tick.
These remain display flips and scheduler job timings, **not unique app FPS or
per-frame histograms**. The clock still does not establish a 25-FPS presentation
budget. Bounded in-flight rendering and true frame telemetry remain in
`dock-o5b`; this completed cache slice is `dock-o5b.1`.

Validation on Agora: host/ARM checks, 85 widget tests, and the explicitly run
Vulkan GPU test passed. The GPU test compares actual cached output against
iced's original gradient, within one 8-bit channel value, for dark, light,
and saturated colors at scales 1, 2 and 1.25. It verifies allocation reuse,
four-window stability, and resource retirement. The actual TV capture was also
inspected. Strict Clippy still reports the five pre-existing diagnostics noted
in the rollout report; rerunning with only those legacy lint categories allowed
passed. No unrelated legacy widgets were rewritten.

The cache was cross-built in the isolated compatibility snapshot described in
`coherence-home-pi-rollout.md`; current main's unrelated transport changes were
not deployed. The pre-cache UI is retained at
`/usr/local/lib/canvas/rollback/gradient-cache-20261005/canvas`, and the compatible
workspace state was copied to
`/home/ajm/.cache/canvas-perf-20261005/workspaces.before-gradient-cache.json`.
The original 24-workspace backup remains untouched. Only the UI binary changed;
the compositor, capture geometry, service configuration, and media transport
were preserved. QEMU was restored after profiling.

Installed UI SHA-256:
`e80da2e65313a226e9f44644dc40780426be92bb96a5c7ab5ac9efd3cd3cf9b9`.
