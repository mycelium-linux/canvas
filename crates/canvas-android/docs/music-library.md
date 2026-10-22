# TV music library — thin first slice

Canvas's Library button opens a separate cover-first view. Platform Java owns
painting and focus; Rust uses the existing `canvas-feeds::music_library` owner
client, and `fpl-gfx::media::Shelf` supplies geometry. Artwork accents use GFX's
bounded thumbnail algorithm. No MPD/catalog implementation lives in the UI.

The configured `library_endpoint` is an HTTP loopback origin only. Set it using
the authorized ADB activity launch extra; it persists in app-private preferences.
Do not expose unauthenticated Deadwax on the LAN. The development deployment uses
an SSH local forward from Neo to Pi's loopback Deadwax, then authorized ADB reverse
forwarding into the TV. Closing Neo or losing ADB disables this bridge; the UI
reports failed catalog/control requests rather than pretending playback succeeded.
After a TV/ADB restart, restore the reverse mapping. A future authenticated Unibus
owner-control lane can replace this transport without changing the catalog client.

Example transport/runbook (use the actual configured hosts):

```sh
ssh -N -o ExitOnForwardFailure=yes -L127.0.0.1:26601:127.0.0.1:6601 ajm@home-pi.local
adb -s TV reverse tcp:26601 tcp:26601
adb -s TV shell am start -n dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity --es library_endpoint http://127.0.0.1:26601
```

Use the remote to select Library, then a cover. The detail pane lists tracks;
Play starts the selected album, Queue appends without starting, Back restores the
workspace. From record controls, Back first returns to the selected cover; the
next Back closes Library and restores workspace focus. Focus is contained inside
the library while it is open. Closing does not rebuild the workspace or destroy
the underlying video surface. Late catalog responses cannot claim focus after
closing. Reopening retains the current shelf, selection, and decoded covers.
Refreshing covers never changes playback, volume, or queue. Action
receipts say “Started MPD”, not a fabricated continuous now-playing status.
Do not blindly retry an ambiguous playback failure; the owner may have added tracks.

Bounded work: six visible covers, one artwork worker separate from control requests,
8 MiB encoded art, 32 megapixel decode admission, downsampled to at most 640 pixels,
64×64 accent samples, no artwork/geometry network work on the draw thread. No
background catalog polling. Spotify browsing/transport and live now-playing remain
separate follow-up slices; this view does not claim they are already integrated.

Focus metadata is debounced by 120 ms and stale queued requests are skipped.
Decoded cover caching is bounded to 16 MiB, with at most 24 cached track lists.
Rust reuses one pooled owner client rather than creating a runtime per request.

Interaction verification runbook (does not alter playback): open Library, activate
a cover to focus Play without activating it, press Back to return to the cover,
then Back to close. Reopen and verify the same shelf and focus are retained.
Repeat while artwork is loading. Check each current focus before sending another
remote input; do not activate Play or Queue as a navigation test.

## Console appearance (independent of Library and playback)

Signal console painting accepts the shared GFX `ConsoleAppearance` as host theme
data. An injected palette may include a `console_appearance` object. The TV shell
also accepts a persisted `console_appearance` launch extra as a development
override; an empty string clears the override. This does not modify audio, signal
snapshots, labels, or control geometry. Unknown fields and out-of-range material
tokens fail visibly rather than silently selecting a different finish.

Example flat finish (restart the Activity to apply a launch preference):

```sh
adb -s TV shell am force-stop dev.fpl.canvas.tv
adb -s TV shell 'am start -n dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity --es console_appearance '\''{"gloss":0,"grain":0,"hardware_details":false,"cast_shadows":false}'\'''
```

Appearance keys: `face`, `cap`, `slot`, `identity`, `markings` (existing GFX semantic
roles), `gloss` (0–1), `grain` (0–0.02), `hardware_details`, `cast_shadows`. Defaults
retain the physical studio finish. The native Android adapter paints geometry;
wgpu material lighting remains specific to a wgpu host. Assets/filmstrips are not
loaded by this API, and no Pedalkernel artwork is bundled yet.
