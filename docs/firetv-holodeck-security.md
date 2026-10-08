# Holodeck and Security on Canvas TV

The shared retained workspaces are `holodeck` and `security`. They are explicit
top-level choices in `config/deploy/firetv/menu.json`, not modal dialogs. Existing
projects and documents must be preserved when adding them to a live catalog.
The install command replaces the catalog; never install just these two projects.
Use the current app-private retained projection as the merge base and a
revision-bound request. Menu changes require packaging the descriptor into the
APK; they do not require changing the native player or Rust renderer.

## Holodeck

The default view consumes Agora's existing animated room tour HLS output.
The page controls in the workspace menu select Cameras, First-person cast or AirPlay.
One video is decoded at a time, using
the existing Android hardware playback path and surface mute control.

These are separate sources, not automatic substitutes for each other:

- `holodeck-tour.m3u8`: animated rendered tour, not live camera footage.
- `holodeck-cctv.m3u8`: live Agora-rendered virtual spectator cameras.
- First-person cast automatically waits while no capture producer is running,
  then plays the existing Quest reduced-buffering `quest-fast.m3u8` output when
  its media sequence/segment identities advance. A file existing is not enough.
- AirPlay's existing `/live.m3u8`: requires an active sender session.

"CCTV-like cameras" here means virtual spectator cameras rendered inside the
VR room on Agora, not physical Frigate cameras. Holodeck owns spectator
rendering, room state and capture. Producers continue to own rendering/capture,
Unibus owns transport, Mycelium owns discovery, and Canvas owns layout and
playback. A headset/FPS capture and a virtual spectator view are distinct
sources. The existing HTTP service retains its TV client allowlist.

### Automatic first-person lifecycle

`canvas-tv-stream-watch` reads the explicitly configured producer playlist over
authorized SSH, never scans for sources, and updates only the named surface
while its workspace is selected. It does not steal workspace/page selection.
The Quest capture producer remains separate; wake/wear the headset and start
that producer using Holodeck's `docs/quest-tv-casting.md` runbook.

```sh
cargo check --manifest-path crates/canvas-android/Cargo.toml --features shared-workspaces --bin canvas-tv-stream-watch
cargo build --manifest-path crates/canvas-android/Cargo.toml --features shared-workspaces --bin canvas-tv-stream-watch
canvas-tv-stream-watch 192.168.2.97:5555 config/deploy/firetv/holodeck-cast-source.json
```

The observer emits the narrow shared `live_stream` content contract:

```json
{"kind":"live_stream","schema_version":1,"url":"http://192.168.2.67:18089/quest-fast.m3u8","generation":"producer-session","issued_at_unix_ms":0,"expires_at_unix_ms":5000}
```

The sample timestamps are illustrative; the observer supplies actual wall-clock
milliseconds. The maximum lease is five seconds and consumer clocks must be
synchronized (up to one second of future issue-time tolerance). Canvas Rust
normalizes it into the ordinary stream primitive while valid, and a waiting
scene when expired. Retained state is not rewritten by the renderer. The TV
checks independently every presentation refresh, including when the observer
dies or the user returns from another workspace. Older endpoints retain this
unknown kind and show an unsupported placeholder rather than playing stale
video. Other platform renderers must opt into the same lease contract before
using it for playback.

Unchanging playlists and metadata-only rewrites cannot renew a lease. ENDLIST,
missing/stale files, SSH timeouts and malformed playlists return to waiting.
At startup or after failure, two observations with actual media progress are
required. A sequence rollback or recovery after expiry changes generation;
the player URL receives that generation to invalidate cached decoder state.
Renewals in the same generation produce identical presentation content and do
not reconstruct/restart playback. Neo's launch agent is
`config/deploy/neo/dev.fpl.canvas-tv-cast.plist`.

Channels is available only in Television. Both the Java affordance and Rust
tuner admission check enforce that scope before any catalog/tune request.

## Security

Security opens on the Cameras page. CAT is relayed from Frigate's existing
`cat_sub` restream as HLS because the TV presentation endpoint accepts private
LAN HTTP streams, not RTSP URLs. DOOR uses the same relay from `door_sub` at
`http://192.168.2.74:18090/door.m3u8`. The separate Fleet page retains the
Mycelium status view. Camera2 is on the untagged TV LAN with static address
`192.168.2.13/24`, gateway `192.168.2.1`; Frigate's four DOOR source paths
point there. The old CCTV reservation `192.168.30.13` is no longer the live
address. Camera2 uses `time.cloudflare.com` for NTP, Pacific standard time,
and daylight saving time. If the feed fails, check Frigate's DOOR camera FPS,
probe `door_sub`, and confirm the TV can fetch the HLS playlist before changing
its surface.

The home-pi relay units are
`config/deploy/home-pi/canvas-tv-cctv-http.service` and
`canvas-tv-cctv@.service`. Install their HTTP allowlist as
`/home/ajm/.config/canvas/tv-cctv/httpd.conf`; it admits only the TV and local
loopback. The HTTP unit owns `/run/canvas-tv-cctv`, and the FFmpeg units
copy H.264 from the existing restreams into short HLS segments. Both camera
relay units and the HTTP unit are enabled on home-pi. The TV's retained
Security document was updated through revision-bound workspace operations,
preserving the other installed projects.

The relay template uses `Restart=always` because FFmpeg can exit successfully
when Frigate restarts and closes an RTSP session; otherwise that camera remains
stopped after Frigate recovers.

`canvas-security-status` calls Mycelium's existing read-only `security status
--json` and `security events --json` commands and emits a generic status-list
projection. It performs no scan, discovery, remediation, or credential handling.
It selects the newest host observation per site/hostname, deduplicates event
IDs, bounds visible rows, and explicitly shows scanner coverage and stale host
observations. Zero findings with unavailable scanners is not green.

```sh
cargo check -p canvas-feeds --bin canvas-security-status
cargo test -p canvas-feeds --bin canvas-security-status
cargo build -p canvas-feeds --bin canvas-security-status
canvas-tvctl 192.168.2.97:5555 widget-watch security security-status ./target/debug/canvas-security-status
```

The existing generic watcher refreshes while Security is selected, applies
revision-bound surface patches, and marks source failures disconnected. The TV
marks an old observation stale if the watcher disappears. Neo's deployment
launch agent is `config/deploy/neo/dev.fpl.canvas-tv-security.plist` and assumes
the two binaries are installed under that operator's `.local/bin`. It uses
authorized ADB and obtains the current private app credential dynamically.
No token belongs in this document or the workspace JSON.

Open either view with:

```sh
printf 'workspace switch holodeck\n' | canvas-tvctl 192.168.2.97:5555
printf 'workspace switch security\n' | canvas-tvctl 192.168.2.97:5555
```
