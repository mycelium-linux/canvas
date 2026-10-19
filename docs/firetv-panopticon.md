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
weather, PeeringDB and RIPE Atlas. The camera holds the Pacific Northwest in view.
These are point layers; chart tiles, boundaries and interactive camera control
remain separate work. Temporal series render when supplied by the owner.

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
- Receiver ingress: Agora's Tailscale address, UDP port 55212.
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

2. Build `cargo build -p canvas-map-wgpu --bin canvas-map-stream`. Install that
   binary as `~/.local/lib/canvas-panopticon/canvas-map-stream` and the producer
   configuration as `~/.config/canvas-panopticon/firetv-stream.json`. Both names
   are distinct from the existing local-map producer and Panopticon source.
3. Install/verify the receiver unit on Agora and start only
   `canvas-panopticon-firetv.service`. Start the producer through its supplied
   LaunchAgent. Enable the receiver for login only after the stream proof.
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

## Rollback

Switch back to the previously selected workspace. Stop/unload only
`dev.fpl.panopticon-firetv` and `canvas-panopticon-firetv.service` if removing the
stream. Preserve the shared HLS server, other producers and app data. Restore
catalog entries only against a fresh TV revision, preserving intervening edits.
