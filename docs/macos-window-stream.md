# macOS window streaming

`canvas-window-stream` captures one macOS application window with
ScreenCaptureKit, encodes it with VideoToolbox H.264, and advertises the
existing `media.video_stream.v1` lease over Unibus. Canvas continues to own
window chrome, placement, focus, and composition.

This is the shared producer for Simulator, scrcpy, Blender, Ghidra, Surf, and
other native windows. Applications do not gain a Canvas dependency.

## Select a window

List capturable windows:

```sh
canvas-window-stream --list
```

A profile selects by stable bundle ID and, when an application owns several
windows, a title substring:

```json
"selector": {
  "bundle_id": "com.apple.iphonesimulator",
  "title_contains": "iPhone 17 Pro"
}
```

`window_id` is also supported for an intentionally ephemeral session. A
selector that matches no windows or multiple windows fails rather than
capturing an arbitrary window or the desktop.

## Add another application

Copy the Simulator profile, then change only these producer-owned values:

- `bus.producer_id`
- `selector`
- `stream_id` and `display_name`
- `stream_endpoint` and `advertised_endpoint`
- optionally `capture_scale`, `max_height`, and `fps`

Give each simultaneous producer its own UDP port and Unibus producer ID. Open
the advertised `stream_id` as a Canvas `live_video` surface; do not bake Canvas
layout or chrome into the producer profile.

`capture_scale` converts macOS logical window points into captured pixels. It
defaults to `1.0`; use up to `2.0` to retain Retina detail. `max_height` is the
final pixel-height ceiling, so a 600-point window with `capture_scale: 1.5`
and `max_height: 900` is encoded at 1200x900 instead of 800x600.

## macOS permission and startup

The deployed executable lives in `Canvas Window Stream.app` with bundle ID
`dev.fpl.canvas-window-stream`. Grant that app Screen & System Audio Recording
once in System Settings. The LaunchAgent may then run one or more profiles
without keeping the source window frontmost.

The checked-in Simulator LaunchAgent and profile are under
`config/deploy/macbook-neo/`. After changing the executable, replace it inside
the app bundle and re-sign the bundle before restarting the agent.

## Failure behavior

- Missing or ambiguous windows terminate the producer with an explicit error.
- A dead VideoToolbox/FFmpeg encoder terminates the producer; launchd restarts
  it.
- Unibus leases expire if capture stops, so Canvas does not present a stale
  producer as healthy.
- Status-only ScreenCaptureKit samples are ignored; malformed image buffers and
  encoder write failures are fatal.
