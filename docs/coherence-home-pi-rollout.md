# Home Pi Coherence rollout — 2026-10-05

The TV UI was built on Agora One for `aarch64-unknown-linux-gnu` with
`--release --features lua-scenes`. Only `/usr/local/bin/canvas` was replaced.
The native `canvas-linux` compositor, service configuration, capture dimensions,
transport implementation, and saved workspaces were not replaced.

## Why a narrow backport

The installed Canvas SHA-256 matched Agora's existing build exactly:
`952965174df16e151610c6250a2a6a66650b8675c018850cad8b2d271d3fcbef`.
Its source lives at `/home/ajmwagar/src/dock`, an exported older worktree whose
Git metadata points to a Mac path. It is not a checkout of current Canvas HEAD.
Replacing it wholesale would also change its working retained-video path.

The rollout snapshot at `/home/ajmwagar/canvas-coherence-tv.7KZV9d/dock` adds:

- The checked-in `config/deploy/home-pi/coherence-ui-backport.patch` for the
  older manifests and clock entry point.
- The **unchanged** `gfx_clock.rs` and `gfx_theme.rs` from Canvas `93a8446`.
- The checksummed Coherence native/token artifacts from the same commit.

`main.rs` and `retained_video.rs` remain byte-identical to the previous build
source. This is a compatibility deployment, not a second theme implementation.
Do not apply this patch to current HEAD; current HEAD already owns the adapter.

## Reproduce

1. Copy the older source to an isolated build directory. Preserve its
   `config/artifacts` inputs and sibling Unibus dependency resolution.
2. Verify the baseline source hashes in the deployment receipt, then apply
   `patch -p1 --dry-run < coherence-ui-backport.patch` followed by the actual
   patch. Copy the canonical modules and both vendored artifacts listed above.
3. Check and test the candidate before producing its release binary:

```sh
cargo check -p canvas --bin canvas --features lua-scenes
cargo test -p canvas-widgets --features live,iced/x11 --lib
cargo test -p canvas-core --lib
```

4. Use Agora's installed ARM compiler and multiarch libraries:

```sh
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
PKG_CONFIG_ALLOW_CROSS=1 \
PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig \
cargo build --release --target aarch64-unknown-linux-gnu \
  -p canvas --bin canvas --features lua-scenes
```

5. Copy the artifact through Mycelium with the Pi's explicit legacy `--user
   ajm`, verify SHA-256 and `ldd` on the Pi, and retain the installed binary and
   workspace file before installing the candidate through a temporary filename
   and atomic rename. Restart **only** `canvas-surface.service`.
6. Verify installed/running binary hashes, control socket responses, service
   health, and an actual screenshot with an analog clock. Restore the previous
   active workspace afterward. The media edge has a `Requires` dependency on
   the UI and restarts automatically with it; verify its reattachment too.

The candidate passed host/ARM checks, 84 widget tests and 95 core tests. Its
release binary and source archive are retained under the rollout directory's
`artifacts/`; hashes and live verification evidence are recorded in
`config/deploy/home-pi/coherence-ui-receipt.json`.

Strict widget Clippy is blocked by five diagnostics in unchanged legacy code
(`large_enum_variant`, `collapsible_if`, `manual_is_multiple_of`). This rollout
does not claim a clean strict-lint gate or silently rewrite those unrelated
widgets.

The live clock is configured for 25 updates/sec (40 ms), not 60 FPS. A
10-second `pidstat` sample measured Canvas at 7.3% of one CPU core and the
compositor at 6.6%; Canvas RSS stayed at 80,612 KiB. A separate DRM render-engine
counter sample measured 8.133 GPU-active seconds over 10.043 wall seconds
(about 81%). This is engine active time, not frequency-normalized utilization,
per-frame latency, or proof of actual presented FPS. The older TV build exposes
no frame-time histogram or presented-FPS counter. Do not claim a speedup or a
60-FPS budget from these samples.

## Rollback

The previous UI binary remains at
`/usr/local/lib/canvas/rollback/coherence-20261005/canvas`. On the Pi:

```sh
sudo install -m 755 /usr/local/lib/canvas/rollback/coherence-20261005/canvas \
  /usr/local/bin/canvas.rollback-next
sudo mv -f /usr/local/bin/canvas.rollback-next /usr/local/bin/canvas
sudo systemctl restart canvas-surface.service
```

The pre-change workspace file remains at
`/home/ajm/.local/state/dock/canvas/workspaces.json.before-coherence-20261005`.
Do not restore it blindly after subsequent user edits. No compositor rollback
is needed because that binary was never replaced.

This receipt covers Home Pi only, not a Neo/Studio fleet rollout.
