# Canvas Linux desktop readiness

Canvas owns surface composition, workspace intent, focus and presentation.
The Linux compositor lives in `crates/canvas-linux/src/{compositor,runner,launcher}.rs`.
Build `canvas-linux` on Linux with `--features compositor` for the Smithay
Wayland/KMS server. The headless Sway adapter is retired; old project-scoped
capture/input and terminal-attachment commands fail explicitly rather than
silently reaching a different seat. The iced Canvas
application itself is a Wayland client. Native application lifecycle,
clipboard, portals and secure session locking belong to platform providers.
Remote applications continue to own their application state.

## Source recovery and deployment boundary

The compositor was recovered from the original linked working copy
`unibus/tmp/dock-launcher`, whose three source files matched Home-Pi's
`/home/ajm/src/fpl/canvas-build/crates/canvas-linux/src` byte-for-byte before
integration. That Pi directory is an unversioned build staging tree: its
manifest and main entry point had been overwritten with the older adapter.
Dock/Canvas is now the maintained source location, not that staging directory.
The recovered runner SHA-256 is
`5933c296afff2db6ac7d2876613d990c433472bf91974652b7ae4755ecaa7823`.
Home-Pi's existing installed executable SHA-256 at recovery was
`261866e35f2e362ff0fd50cf8415498be205d7e45d5c3af82c47faa49818ab7b`.
Matching source files are evidence of provenance, not a reproducible-build
proof for that installed executable. Recovery does not replace it or restart
the live `canvas-compositor.service`.

The hardened native control endpoint is
`~/.local/state/dock/native-control/compositor-control.sock`, in an owner-only
directory with a mode-0600 socket. One connection carries one newline-terminated
JSON request and response (maximum 64 KiB including newline). A single worker,
two-second I/O/frame/reply deadlines and a 16-request queue bound resource use;
the compositor handles at most 16 commands per iteration. A stalled client can
delay other control clients but not the rendering thread. Bind/worker failures
stop startup/session explicitly. Existing socket paths are never unlinked or
stolen automatically; after a crash, verify the previous compositor is stopped
before removing its stale endpoint. A timeout means the outcome is unknown:
refresh inventory before retrying a mutation. This is same-user local control,
not a remote-authentication or project-scoped authorization API.

Commands use separate arguments: `desktop-compositor start` and
`desktop-compositor status`. Native control uses
`desktop-compositor-control SOCKET JSON` (for example `{"command":"list"}`).
The recovered control protocol is not yet a secure replacement for the old
project-scoped agent input API, and clipboard/portal/lock integration is not
completed by this recovery. Validate the Linux feature build before installing.

## Input contract

A surface opts into desktop input using an `input.desktop_state.v1` input port.
The port ID identifies the target; the local `unibus-hid-socket` owns transport
identity and the destination adapter owns device injection.

Clicking inside the fitted video content acquires focus. Title bars, matte and
letterboxing are outside that content. Pointer normalization uses decoder size
metadata, with document dimensions as the initial fallback. Rendering and
input use the same matte sizing rule.

Ctrl+Option+Escape releases remote input. Input resumes after another content
click. Losing window focus or changing the focused destination releases held
keys and buttons. Destination changes also clear held state in the local HID
publisher. This complements the adapter's expiry-based release.

## Performance inspection

After installing matching `canvas` and `canvasctl` binaries:

```sh
printf 'perf\n' | canvasctl
```

For machine-readable receipts, send the versioned control request:

```sh
printf '%s\n' '{"schema_version":1,"request_id":"perf-manual","command":{"command":"performance"}}' | canvasctl
```

CPU update durations are retained in a rolling window of 512 events, with
p50, p95, p99 and maximum in microseconds. They include synchronous update
work but exclude asynchronous work and GPU presentation. An empty measurement
is null. The report also includes pending raster decodes and video artifact
freshness. File publication age is not capture latency: clock synchronization,
producer timestamps and presentation feedback are needed for that measurement.
The GPU and input-to-presentation fields remain null until instrumented.

## Manual verification

1. Display a remotely controlled application with a visible pointer or buttons.
2. Click its four content corners and center; repeat after resizing and after
   a producer resolution change. Click matte and title areas separately.
3. Hold a key or mouse button, switch focus, and confirm the original application
   receives release. Repeat with Ctrl+Option+Escape and native window focus loss.
4. Compare `perf` receipts while idle, dragging a surface, switching workspaces
   and displaying video. Record the host and workload with each receipt.
5. Stop one producer and confirm its video source becomes stale or missing in
   the performance report while Canvas's control socket remains responsive.

This is a diagnostic foundation, not a completed Linux desktop certification.
Native app integration and true presentation latency still require Linux
hardware validation. The staged work is tracked in Beads under `dock-yjt`.
