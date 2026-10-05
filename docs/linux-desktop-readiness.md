# Canvas Linux desktop readiness

Canvas owns surface composition, workspace intent, focus and presentation.
The current Linux native-window provider is Sway (`canvas-linux`); the iced
Canvas application is not a Wayland server. Native application lifecycle,
clipboard, portals and secure session locking belong to platform providers.
Remote applications continue to own their application state.

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
