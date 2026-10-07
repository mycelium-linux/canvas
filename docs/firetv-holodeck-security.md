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
- First-person cast is an explicit waiting state while no capture producer is
  running. When confirmed live, replace that surface content with a `stream`
  URL for the producer's HLS endpoint (the existing Quest reduced-buffering
  profile uses `quest-fast.m3u8`). Do not show a stale playlist as live footage.
- AirPlay's existing `/live.m3u8`: requires an active sender session.

"CCTV-like cameras" here means virtual spectator cameras rendered inside the
VR room on Agora, not physical Frigate cameras. Holodeck owns spectator
rendering, room state and capture. Producers continue to own rendering/capture,
Unibus owns transport, Mycelium owns discovery, and Canvas owns layout and
playback. A headset/FPS capture and a virtual spectator view are distinct
sources. The existing HTTP service retains its TV client allowlist.

## Security

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
