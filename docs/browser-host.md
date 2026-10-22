# Agent browser host

`canvas-browser-host` runs browser engines near compute and exposes them to
agents without placing a browser engine inside Canvas or on a display-only Pi.
The initial engine is the prebuilt Servo `servoshell`; the broker-facing API is
engine-neutral so Chromium or another engine can implement the same boundary.

## Boundary

- One OS process, temporary storage group, config directory, and loopback
  WebDriver port per browser session.
- Agents authenticate to one broker endpoint with a shared bearer credential.
  Raw WebDriver ports are never bound beyond loopback.
- A browser keeps running when its Canvas surface closes. Rendering starts only
  after an agent creates a WebDriver session and only when the session selected
  a host-authored media profile.
- Browser frames reuse `media.video_stream.v1`. Unibus carries the expiring
  announcement; H.264 MPEG-TS stays on the media plane; `canvas-media-edge`
  hardware-decodes it into a native `live_video` surface.
- The Pi runs only `canvas-media-edge` and Canvas. It never runs Servo,
  WebDriver, FFmpeg encoding, credentials, or browser storage.

Do not share one Servo process between agent principals. Multiple pages for one
principal can become multiple WebDriver windows later, but Servo storage and
preferences are not yet proven isolated at WebView granularity.

## Host setup

Install an official signed Servo release and create a random credential readable
only by the service account. Start from
`config/demo/browser-host.example.json`; production listeners should bind an
explicit tailnet/LAN address rather than `0.0.0.0`.

```sh
cargo build -p canvas-browser-host
target/debug/canvas-browser-host /path/to/browser-host.json
```

The checked-in example has no media profiles and is safe for control-path
testing. A host-owned media profile additionally supplies:

```json
{
  "bus": {
    "installation_id": "home",
    "producer_id": "studio-browser-host",
    "hub_endpoint": "127.0.0.1:9417",
    "token_file": "/path/to/unibus.token",
    "zones": ["living-room"],
    "capabilities": ["media.video_stream.source"],
    "screen_anchors": [],
    "schema_version": 1
  },
  "destination": "living-room",
  "ffmpeg_binary": "/opt/homebrew/bin/ffmpeg",
  "encoder": "apple_video_toolbox",
  "stream_endpoint": "udp://HOME_PI_TAILNET_IP:5600?pkt_size=1316",
  "advertised_endpoint": "udp://0.0.0.0:5600",
  "zone": "living-room",
  "fps": 12,
  "width": 1280,
  "height": 720,
  "ttl_ms": 15000
}
```

The named profile is an authority boundary: callers can select it, but cannot
supply an encoder command, destination, endpoint, bus token, or executable.
Only one live browser may claim a profile at a time.

## Agent runbook

All `/v1` calls require `Authorization: Bearer …`.

1. `POST /v1/sessions` with a safe session ID, viewport, optional initial URL,
   and optional media-profile name.
2. Use the returned `webdriver_path` as the WebDriver base, creating a normal
   W3C session at `<webdriver_path>/session`.
3. Drive the browser with ordinary WebDriver paths below that base. The broker
   forwards methods and JSON bodies to that session's loopback listener.
4. If a media profile was selected, successful WebDriver-session creation starts
   screenshot capture, host hardware H.264 encoding, and expiring Unibus stream
   announcements. The Canvas source ID is `browser-<session_id>`.
5. Add or retain a `live_video` Canvas surface with that source ID and the media
   profile's width and height. Presentation state remains Canvas-owned.
6. `DELETE /v1/sessions/<id>` stops streaming and the browser process. The
   announcement expires, so a disconnected browser cannot look live forever.

`GET /health` is unauthenticated and reveals no session state. `GET
/v1/sessions` is authenticated and reports process IDs, viewport, WebDriver
proxy path, selected media profile, and whether streaming has started.

## Current performance datum

The first live Neo proof used Servo 0.5.0 at 1024×768. Two independent loaded
browser processes consumed about 90–91 MiB RSS each while idle. That is a datum,
not a capacity promise; the acceptance benchmark remains cold/warm RSS and PSS,
idle CPU, action latency, and stream latency for 1/4/8/16 workers on the intended
host.
