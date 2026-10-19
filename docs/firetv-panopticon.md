# Panopticon globe on Fire TV

The TV presents the shared Canvas / `fpl-gfx` globe as H.264 HLS. The Studio
renders the globe; Agora remuxes the remote stream through its existing private
TV media edge. Android owns native playback. This does not embed Bevy in Canvas
or add another app to the TV. This is a display-only globe; the remote camera
is not yet an interactive, synchronized Galileo session.

The profile uses Panopticon's existing Earth raster and canonical Coherence
theme. Spatial layers come from the local owner's authenticated
`/operator/spatial/snapshot` endpoint, refreshed every five seconds on a separate
thread. The selected domains are vessels, HF radar, NOAA CO-OPS, marine and NWS
weather, PeeringDB and RIPE Atlas. A deterministic tour holds over Puget Sound,
pulls out to North America, flies into Puerto Rico, then returns through a
Caribbean orbital view. Stops, dwell times and travel times live in
`camera.tour.stops`. Camera travel follows short longitude arcs with eased
position and geometric distance interpolation, remaining above the surface.
These are point layers; nautical chart overlays, boundaries and interactive
camera control remain separate work. Temporal series render when supplied by the owner.

Detailed imagery uses Galileo's public renderer-neutral `tile_lod` and
`tile_cache` modules with its desktop feature disabled. Camera ground resolution
selects XYZ detail. A bounded worker fetches at most 64 tiles per region using
four concurrent requests, validates 256×256 images, caches compressed provider
bytes, and uploads a north-up mosaic. `fpl-gfx` samples that mosaic in Web
Mercator over its retained global basemap, preserving global coverage while new
regions load. The worker replaces pending camera views with the newest request,
skips unchanged regions and retains four decoded mosaics (up to 64 MiB of cached
pixels). Provider failures remain visible in the producer log and keep the
last valid imagery. The TV caption credits the configured Esri World Imagery
provider: Esri, Vantor, Earthstar Geographics and the GIS User Community.

The credential lives in an owner-private file referenced by `live_snapshot.key_file`;
never commit it or pass it in process arguments. The curl transport only accepts
loopback URLs, follows no redirects, limits responses to 16 MiB and times out
after four seconds. Failed requests clear overlays. Snapshots expire at the
owner's maximum age (capped at 60 seconds); individual entities expire using the
owner's `summary.expires_at_unix`. Thus a fresh snapshot cannot revive an expired
vessel. Source errors are recorded in the producer log; the TV caption describes
layer selection and expiry policy, rather than claiming that every feed is healthy.
The legacy file-snapshot configuration remains supported for offline consumers.

## Stream route

- Producer: `config/deploy/mac-studio/panopticon-firetv-stream.json`.
- Supervisor: `config/deploy/mac-studio/dev.fpl.panopticon-firetv.plist`.
- Receiver: `config/deploy/agora-one/canvas-panopticon-firetv.service`.
- Receiver ingress: Agora's direct LAN address `192.168.2.67`, UDP port 55212.
- TV URL: `http://192.168.2.67:18089/panopticon.m3u8`.
- Workspace: `config/deploy/firetv/panopticon.json`.

The receiver reuses `canvas-tv-media-http.service`; do not restart that shared
server or change another stream. FFmpeg copies H.264 into a rolling eight-entry
playlist with atomic updates, bounded segment deletion and independent segments.
Only the remote stream adapter reads GPU pixels; Studio's native map composition
continues to use its existing direct GPU path.

## Manual setup and verification

1. Check the explicit TV target, `192.168.2.97:5555`, with authorized ADB. Never
   clear app data or replace its signing key to get access. If this Mac needs a
   network route, SSH-forward its ADB connection through Agora:

   ```sh
   ssh -o BatchMode=yes -o ExitOnForwardFailure=yes -N \
     -L 127.0.0.1:15555:192.168.2.97:5555 agora-one
   ```

   Then `adb connect 127.0.0.1:15555`. Use that exact serial for the local client.
   The TV's debugging fingerprint dialog must authorize this host. The existing
   `canvas-tvctl` handles the app-private credential without exposing it.

   The verified installation used Neo's already-authorized ADB identity. Connect
   to Neo with the existing Mycelium certificate (ordinary key-only SSH is not
   the configured route):

   ```sh
   ssh -o BatchMode=yes \
     -o CertificateFile="$HOME/.mycelium/ssh/user-cert.pub" \
     -o ExitOnForwardFailure=yes -N \
     -R 127.0.0.1:15556:127.0.0.1:15555 averys-macbook-neo
   ```

   On Neo, connect ADB to `127.0.0.1:15556` and pass that explicit serial to its
   installed `~/.local/bin/canvas-tvctl`. Its child `adb` needs
   `/opt/homebrew/bin` in `PATH`. The SSH tunnels are setup/control paths only;
   they are not part of the TV's persistent video route.

2. Build an optimized host with `cargo rustc -p canvas-map-wgpu --bin
   canvas-map-stream -- -C opt-level=3 -C debuginfo=1`. Install that
   binary as `~/.local/lib/canvas-panopticon/canvas-map-stream` and the producer
   configuration as `~/.config/canvas-panopticon/firetv-stream.json`. Both names
   are distinct from the existing local-map producer and Panopticon source.
3. Install/verify the receiver unit on Agora and start only
   `canvas-panopticon-firetv.service`. Start the producer through its supplied
   LaunchAgent. Enable the receiver for login only after the stream proof.
   Preserve `ProcessType=Interactive` in the producer LaunchAgent: this is a
   continuous video workload. Under Studio's background scheduling policy the
   same optimized binary delivered too slowly to sustain playback.
   Before enabling live layers, provision `spatial-owner.key` with mode 0600
   from the existing Panopticon owner's internal credential. On this Studio,
   its source is the `EnvironmentVariables.PANOPTICON_INTERNAL_KEY` entry in
   `~/Library/LaunchAgents/dev.fpl.panopticon-ais-local.plist`; use `plutil
   -extract ... raw -o DESTINATION` under `umask 077` so its value never appears
   on stdout. Do not generate a replacement owner key.
4. Use Agora's `ffprobe` to check the HLS output: H.264, 1280 × 720, moving frames.
   Check at least two distinct decoded frame hashes and playlist advancement.
   A running service or a file's existence is not frame-delivery proof.
5. Inspect the TV with `canvas-tvctl SERIAL` and save its app-owned checkpoint
   using authorized `adb -s SERIAL shell run-as dev.fpl.canvas.tv cat
   files/workspaces.json`. Keep the checkpoint owner-private. Check owner and
   revision against a fresh inspect result before preparing a mutation.
6. Merge one Panopticon project and the supplied workspace into the checkpoint's
   complete `projection.catalog` and `projection.documents`. Preserve all other
   projects, resources, documents and the current selection. If Panopticon
   already exists, review its current document before replacing it. Send an
   `install` request with that live revision through the existing `canvas-tvctl`.
   Do not use a bundled example catalog as the TV's current state.
7. Check that no playback measurement is active. Switch to `panopticon`, inspect
   the retained URL, and capture the actual TV screen with authorized ADB.
   Confirm the visible globe and source-state footer and that subsequent video
   frames change. Do not claim on-screen success from workspace selection alone.

   For this Fire OS version, use `adb shell screencap -p
   /data/local/tmp/canvas-panopticon-proof.png` followed by `adb pull`. Its
   vendor shell prints an initialization banner, which contaminates a direct
   `exec-out screencap -p` PNG stream.

## Verified installation, 2026-10-19

The actual `firetv-omni` app selected `tv-panopticon`. Its 28 original projects
and documents were compared against the owner checkpoint and retained exactly;
Panopticon globe is the 29th entry. The existing `panopticon-wall` workspace
remains separate. Actual TV screenshots show the globe rotating and the
basemap-only caption. Agora decoded three consecutive, distinct video frame
hashes at H.264 1280 × 720, 24 fps. The producer LaunchAgent is running and the
dedicated receiver is active and enabled. No APK update or app reset was needed.

## Transition performance investigation, 2026-10-19

Fire TV's native player logged repeated buffer starvation lasting 11–15 seconds.
With the same optimized producer binary, warm imagery cache, 720p24 configuration
and initial Puget hold, changing the LaunchAgent's scheduling policy to
`Interactive` produced the following measurements. The common window excludes
startup and covers elapsed seconds 2–25:

| LaunchAgent policy | Frames | p99 frame work | Frames over 41.67 ms |
| --- | ---: | ---: | ---: |
| Default | 153 | 533.10 ms | 35.3% |
| Interactive | 547 | 40.26 ms | 0.7% |

Agora subsequently published ten one-second segments in ten wall-clock seconds;
the earlier measurement advanced only three. The producer and receiver now use
the direct LAN route, avoiding the VPN's 1280-byte MTU. Live playlists omit the
end marker so a receiver shutdown cannot permanently end playback on the TV.
After a receiver restart, wait for a newly advancing live playlist before opening
it; an old playlist may still contain an end marker from the previous version.

The separate optimized-host cache experiment did **not** establish a frame-time
win: p99 was 24.33 ms before and 22.66 ms after, with 0.58% versus 0.66% over
budget. The cache change removes repeated decoding/uploads and replaces obsolete
pending views, but scheduling was the substantial measured delivery problem.
This distinction matters when diagnosing future regressions.

A remaining cadence bug reset the absolute deadline after a late frame while
FFmpeg continued assigning fixed-rate timestamps. Over 305 seconds, the running
producer averaged 23.56 fps; such drift eventually drains a live buffer even
when most individual frames are cheap. Keeping the absolute deadline lets short
stalls catch up. The corrected installed producer delivered 24.007 fps over a
130-second interval containing a complete tour. Raw trace:
`/tmp/canvas-live-cadence.csv`. This producer measurement alone does not establish
the TV player's buffering behavior.

Measurements used M1 Max / 32 GiB, macOS 15.0.1, Rust 1.98.1, the existing cached
development dependency artifacts and an `opt-level=3, debuginfo=1` stream host.
They do not represent a fully optimized dependency build. Studio was under heavy
concurrent load; these are bounded observations, not confidence intervals.
The source baseline was `e655dfd` plus optional tracing. Configuration SHA-256:
`b9399b4f50397defa556f7c2c6e48dbf6026ef436b3f96fe3df0245eabd5f167`.

For diagnosis, set `CANVAS_MAP_STREAM_TRACE` to a CSV path before starting the
producer. It records updates, render/readback, encoder handoff and total frame
work. Leave it unset for normal operation. Retained local evidence includes
`/tmp/canvas-smoothness-{baseline,after}.csv`,
`/tmp/canvas-launchagent-{before,interactive-proof}.csv` and the matching logs.
Instruments was unavailable with Command Line Tools; native `sample` captures
are a separately collected profiling alternative, not wall-time benchmarks.
The stream's 15 focused tests pass. Full workspace checks have unrelated existing
failures recorded separately in Beads.

## Rollback

Switch back to the previously selected workspace. Stop/unload only
`dev.fpl.panopticon-firetv` and `canvas-panopticon-firetv.service` if removing the
stream. Preserve the shared HLS server, other producers and app data. Restore
catalog entries only against a fresh TV revision, preserving intervening edits.
