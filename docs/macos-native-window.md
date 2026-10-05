# Direct local macOS windows

`canvas-native-window` captures one explicitly selected window with
ScreenCaptureKit and publishes IOSurface handles to the owner-local Canvas socket.
There is no codec, ffmpeg process, Unibus pixel stream, or network hop. Remote
window streaming remains the responsibility of `canvas-window-stream`.

## Build and run

```sh
cargo build --release -p canvas --bin canvas --features lua-scenes \
  -p canvas-media-edge --bin canvas-native-window
target/release/canvas-native-window --list
target/release/canvas-native-window config/deploy/neo/simulator-native.json
```

Run the updated Canvas first and wait for its control CLI to respond before
starting capture. The example selects the Simulator's iPhone 17 Pro
window; edit the explicit selector for another device or app. Missing or ambiguous
matches fail rather than capturing an entire desktop. Screen Recording permission
is required for the capture executable. Do not replace the independently installed
Dock Control or Canvas Window Stream helpers or reset their permissions.

Attach the source using the local Canvas owner's stdin CLI:

```sh
canvasctl <<'EOF'
surface add-external simulator native/simulator
workspace save fungos
EOF
```

Use the existing active workspace or switch explicitly before attaching; replace
`fungos` with that workspace's name when saving. Capture
registration alone does not change workspaces or create a visible window.

## Boundary and lifetime

The descriptor carries source identity, generation, actual native dimensions,
BGRA/sRGB format, premultiplied alpha, and IOSurface ID. Non-global capture
surfaces use the explicit `io_surface_mach_port` transport. Their native send
right travels through a local Mach rendezvous named from the owned Canvas Unix
socket's UID/device/inode; source hash, generation and surface ID correlate the
two channels. Restarting Canvas requires reconnecting the producer. The bounded
rights receiver retains at most eight unmatched rights and checks a one-second
expiry on consumption. Legacy globally lookupable IDs remain a separate transport.

A dedicated ancillary
datagram FD carries a versioned, generation-bound acceptance receipt. This is not
an input channel and is not a GPU completion signal.

Canvas resolves the transferred Mach right and validates the real IOSurface,
retains it and increments its
use count before acknowledging. The producer holds the capture sample and its
own use count until acceptance. The renderer retains its native buffer until
submitted GPU work completes. Capture requests a transparent background and
Canvas omits the opaque video matte for alpha-bearing native frames.
The Metal importer uses that retained reference,
not a second global ID lookup. Unsupported formats, unavailable surfaces, stale
generations and timeouts fail explicitly.

Acceptance waits are bounded at ten seconds; successful acknowledgements taking
over 100 ms are logged. On failure, withdrawal targets the latest sent registration,
including an unacknowledged pending generation, rather than leaving it retained.

Capture uses a five-buffer pool and a latest-only, one-sample mailbox. Real capture
idle observations renew the unchanged frame; process liveness alone does not.
The capture profile requests 30 FPS; the renderer schedules native redraws at
33 ms. These are targets, not measured performance claims.

This first slice is read-only: it does not advertise an input transport. Native
keyboard/mouse forwarding must use a separate, permission-bearing control adapter.
It is not an automatic app catalog or launcher.

## Verification and rollback

Check the producer's `native frames accepted` log, Canvas's external-source list,
and a screenshot of the attached window. Resize and scroll the source window and
verify shape, alpha and updates; compilation alone does not prove cross-process
IOSurface sharing works. Stop the producer before restoring an older Canvas
binary. Normal process exit withdraws the source; abrupt termination instead
leaves a stale frame until replacement or Canvas exit.
