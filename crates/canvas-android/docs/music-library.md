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
workspace. Refreshing covers never changes playback, volume, or queue. Action
receipts say “Started MPD”, not a fabricated continuous now-playing status.
Do not blindly retry an ambiguous playback failure; the owner may have added tracks.

Bounded work: six visible covers, one artwork worker separate from control requests,
8 MiB encoded art, 32 megapixel decode admission, downsampled to at most 640 pixels,
64×64 accent samples, no artwork/geometry network work on the draw thread. No
background catalog polling. Spotify browsing/transport and live now-playing remain
separate follow-up slices; this view does not claim they are already integrated.
