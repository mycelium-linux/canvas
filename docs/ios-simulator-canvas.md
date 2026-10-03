# iOS Simulator on the home Canvas

The iOS Simulator runs on the Mac. `canvas-simulator-stream` captures the
booted device framebuffer with `simctl`, feeds PNG frames to FFmpeg, encodes
H.264 with VideoToolbox, and sends an MPEG-TS stream directly to `home-pi`.
Its expiring `media.video_stream.v1` announcement travels through Unibus. The
Pi's existing `canvas-media-edge` validates that announcement, decodes only a
source Canvas is displaying, and writes retained frames for the native
`live_video` widget.

The source ID is `simulator-ios`. Canvas and the Pi never run Xcode, hold Apple
credentials, or execute capture commands.

## One-time installation

On the MacBook:

```sh
cargo build --release -p canvas-media-edge --bin canvas-simulator-stream
install -m 0755 target/release/canvas-simulator-stream ~/.local/bin/canvas-simulator-stream
install -m 0600 config/deploy/macbook-neo/simulator-stream.json ~/.config/canvas/simulator-stream.json
cp config/deploy/macbook-neo/dev.fpl.canvas-simulator-stream.plist ~/Library/LaunchAgents/
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.fpl.canvas-simulator-stream.plist
```

The checked-in profile sends UDP to `192.168.10.74:55208` and reaches the
home Unibus router through the existing SSH forward at `127.0.0.1:18794`. Its
token remains in `~/.config/canvas/home-pi.token`; no credential enters a
workspace document or the Pi renderer.

On `home-pi`:

```sh
ssh ajm@home-pi.local 'sudo systemctl enable --now canvas-media-edge'
```

Do not enable the old broad `canvas-media-announce` service for this source.
The Mac publisher owns the Simulator lease and stops renewing it when capture
fails, so a dead capture cannot remain advertised as live.

## Start the stream

Boot any Simulator device, either in Xcode or from the command line:

```sh
open -a Simulator
xcrun simctl boot "iPhone 17 Pro" # omit when Simulator already booted it
launchctl kickstart -k "gui/$(id -u)/dev.fpl.canvas-simulator-stream"
tail -f ~/Library/Logs/canvas-simulator-stream.log
```

The service exits loudly when no device is booted. Launchd retries every five
seconds. When capture succeeds it renews a 15-second lease every 7.5 seconds;
the Pi removes the decoder after expiry.

## Put it on a Canvas

First require a fresh decoded frame. This is the eligibility check that keeps a
dead source off the wall:

```sh
ssh ajm@home-pi.local '
  test -s /run/user/1000/dock/canvas/video-frames/simulator-ios.rgba &&
  echo "live_video simulator-ios 720 1280 ios-simulator" | canvasctl
'
```

Canvas creates a native `live_video` surface named `ios-simulator`. Move or
resize it with the normal Canvas controls. The document form is:

```json
{
  "id": "ios-simulator",
  "title": "iOS Simulator",
  "content": {
    "kind": "live_video",
    "source_id": "simulator-ios",
    "width": 720,
    "height": 1280
  },
  "inputs": []
}
```

To remove it explicitly:

```sh
ssh ajm@home-pi.local 'echo "close ios-simulator" | canvasctl'
```

The intended compositor policy is: place the surface only after the frame file
is fresh; remove it when the file is stale or the Unibus lease expires. A
running process alone is not proof that the source is displayable.

## Control

The Mac remains the input authority. Focus Simulator with:

```sh
open -a Simulator
```

Keyboard input then goes directly to the real Simulator while its pixels are
shown on the TV. Pointer-to-touch forwarding requires a separate authenticated
input contract with normalized coordinates; it must not be inferred from the
video stream or implemented as unrestricted remote desktop access.

## Manual recovery

```sh
launchctl kickstart -k "gui/$(id -u)/dev.fpl.canvas-simulator-stream"
ssh ajm@home-pi.local 'sudo systemctl restart canvas-media-edge'
```

Useful checks:

```sh
xcrun simctl list devices booted
nc -z 127.0.0.1 18794
ssh ajm@home-pi.local 'systemctl is-active canvas-media-edge'
```
