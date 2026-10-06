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

## Native focus, close and switching

`{"command":"focus","window":"surface-0"}` now sets and verifies keyboard-seat
focus, raises the window in the compositor's front-to-back stack, and configures
only that toplevel as activated. `list` includes `focused_window`, MRU ordering
and client-provided app/title metadata. Pointer motion does not steal keyboard
focus; pointer presses still focus the hit-tested window.

`{"command":"close","window":"surface-0"}` sends `xdg_toplevel.close`. Its receipt
says `close_requested:true, closed:false`, not that the app has exited. The
window remains in inventory until its toplevel is destroyed; unsaved-work
dialogs are not bypassed. Closing the focused window restores the remaining
MRU window. No process kill is involved.

Alt-Tab and Alt-Shift-Tab are handled by the existing leased HID snapshot path.
The window order freezes for one held Alt gesture; closed windows are skipped,
new windows join the next gesture, and Tab is consumed rather than sent into
the selected app. Alt release or input-lease expiry finishes the gesture.
Socket clients may use `{"command":"cycle","reverse":false}` repeatedly and
`{"command":"cycle_finish"}` to finish the same native switching session.
`list.result.switcher` exposes `active`, the frozen `windows` order,
`selected_window` and the derived centered `frame` for shell presentation.
The `canvas` Lua-scenes host now has a dedicated native switcher window:

```sh
CANVAS_NATIVE_CONTROL_SOCKET=/home/ajm/.local/state/dock/native-control/compositor-control.sock \
  canvas
```

For isolated headless tests, use the test server's private
`XDG_RUNTIME_DIR/native-control/compositor-control.sock` instead. This is explicit
opt-in: ordinary Neo/macOS Canvas instances do not guess or connect to a Linux
seat. The compositor's child Canvas process inherits this environment variable
when it is configured on the compositor service.

The window advertises the reserved `dev.fpl.canvas.switcher` Wayland app ID.
The compositor recognizes it as a presentation-only shell overlay, centers and
sizes it using the same geometry returned to the Lua host, renders it above
apps only during a native switching gesture, excludes it from app inventory/MRU
and refuses keyboard focus or pointer hit-testing for it. The app ID is a local
role hint, not an authentication credential. `list.result.shell_overlays`
provides its diagnostic inventory separately. This first panel is keyboard-only.

`config/modules/native-switcher-v1.lua` controls the panel through the existing
bounded primitives and Coherence theme tokens. It hot-reloads without rebuilding
the shell, keeps the selected window visible across more than six apps, and
shows disconnected state rather than inventing an updated selection. A single
background worker observes the bounded owner socket; the GUI never waits on it.
Only the latest observation is retained. Source reads occur at one-second
cadence outside drawing; unchanged inputs reuse the retained Lua scene.

Linux type-checks, Lua execution and isolated real Wayland-client role/focus
proofs validate the source. Physical scanout/visual QA and ARM deployment remain
before claiming the panel works on the TV's installed compositor.

The native compositor now opens libinput's udev `seat0` context in hardware
mode. It handles accelerated relative/absolute pointer motion, five mouse
buttons, wheel scrolling and the existing HID keyboard vocabulary through the
same focus/Alt-Tab policy. Device removal releases held state. Headless mode
never opens physical input. Touch, gestures, touchpad scrolling and unsupported
keyboard usages are not implemented yet.

Physical activity releases remote held keys/buttons and revokes its lease.
`list.result.input_owner` then reports `local`; remote snapshots are rejected
until an owner-socket client explicitly sends `{"command":"input_takeover"}`.
Takeover is rejected while local keys/buttons are held. Physical keys do not
expire merely because the user holds them without generating another event.

No device permissions are changed by the compositor. On Home Pi, the current
`ajm` service account cannot read `/dev/input/event*`; proper active-session
device access is a rollout prerequisite, not a reason to run the shell as root
or grant world-readable input devices. This source has not replaced the TV's
installed compositor yet.

The opt-in native-client proof runs three real Foot processes on an isolated
headless Wayland server, verifies keyboard-enter/close protocol logs and
forward/reverse switching, and never opens DRM devices or changes the live
desktop. With a matching Linux binary built on Agora:

```sh
CANVAS_COMPOSITOR_TEST_BIN=/absolute/path/to/canvas-linux \
  cargo test -p canvas-linux --no-default-features --features compositor \
  native_focus_close_and_alt_tab_reach_real_clients -- --ignored --nocapture
```

The probe prints its retained `/tmp/canvas-seat-proof-*` evidence directory.
`desktop-compositor headless` requires a private `XDG_RUNTIME_DIR`, publishes
state inside that directory and never accesses physical output devices. It is
protocol-test mode, not a performance or hardware-presentation benchmark.

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

## Brokered Linux seat access

The native compositor acquires DRM and libinput devices through Smithay's
libseat session backend. There is no direct-open fallback. A hardware launch
without an active seat or usable DRM output fails visibly; headless mode is
explicit and never acquires a physical seat.

On seat pause, Canvas releases held input, revokes remote takeover, dismisses
the switcher, clears keyboard/pointer focus, suspends libinput, and pauses DRM.
Control remains readable (`list.result.seat_active`), but mutations are rejected.
On resume, Canvas reactivates DRM, resets compositor state, resumes libinput,
and restores application focus. Remote takeover requires a new explicit grant.

Deployment prerequisites and manual runbook:

1. Install the distribution's libseat runtime and build against its development
   package (alongside libinput, libudev, GBM/EGL and xkbcommon development files).
2. Launch from a local PAM/logind login session bound to the intended seat/VT,
   or a deliberately configured seatd broker. An SSH login without a seat is
   not a replacement. Do not run Canvas as root or widen device permissions.
3. Confirm the local session with `loginctl session-status` and select the
   intended libseat backend according to host policy.
4. Start `canvas-linux desktop-compositor start` from that local session. Check
   startup errors and query the native control socket's `list` response.
5. Hold input, switch away from the VT, and verify app input releases and
   `seat_active: false`. Return and verify fresh input and rendering; an old
   remote lease must not resume automatically.

Linux compilation, unit tests and isolated headless Wayland proofs do not
replace this hardware test. The existing Home Pi system service is not yet a
seat-bound PAM launch; do not replace its live compositor until the runtime,
launch session and rollback binary have been prepared.

### Secure lock-screen follow-up (not implemented)

Use `ext-session-lock-v1` with an OS-authenticated locker, not a Lua unlock
panel. Only a compositor-launched locker on a private Wayland socket should
receive the lock capability. Reject remote input and app mutation while locked,
render only opaque lock content, and acknowledge locking only after a protected
frame is actually presented. Locker death must leave the session protected.
Unlock authorization must be tied to the accepted, confirmed lock object;
neither the owner control socket nor Lua may bypass authentication.

The pinned Smithay handler forwards `unlock_and_destroy` even after posting
an invalid-unlock protocol error. Integration therefore needs an explicit
ownership/confirmation guard before forwarding that request. Restart recovery,
locker authentication and physical-output presentation must all be verified
before calling this a secure lock screen (`dock-yjt.10`).
