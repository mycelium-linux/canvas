# Optional tuner control

Enable the Rust `android-tuner-control` feature in an independent build of this
crate. Package that build's `libcanvas_android.so` as **`libcanvas_tuner.so`**;
it exports only the tuner JNI boundary, not renderer/control-server JNI hooks.
Keep the renderer's `libcanvas_android.so` from its matching, verified build.
Use separate target directories so a tuner build cannot replace the renderer
artifact. Java loads the tuner library optionally; without it, Channels is absent.

Rust owns private HTTP admission, response limits/timeouts and channel validation.
Presentation changes use the renderer's existing token-authenticated loopback
control endpoint, not a second in-memory workspace catalog. The token stays in
the app-private files directory. Workspace and surface provenance are checked
before applying a revision-bound patch.

Configure the app with `--es tuner_endpoint http://PRIVATE-IP:PORT/` when opening
its Activity. The endpoint is retained in app-private preferences. The API is
the optional `unibus-hdhomerun-hls` projection owner, documented in Unibus's
`docs/hdhomerun-hls-control.md`. Do not replace it with the raw MPEG-2 proxy.

Channels replaces the active video tile. Other surfaces, workspace selection,
audio mute and geometry are untouched. Up/Down browses; Select tunes; Back or
Back to TV restores the video. There is no modal/Android popup. Program-guide
listings are not currently supplied by the owner and are not fabricated.

Validated on the Omni with Java API-33 compilation, ARMv7 NDK-r26d builds,
focused Rust tests, the live 55-channel catalog and KOMO → KING-HD tuning.
APK installation must be followed by a controlled process restart and a fresh
control receipt; Fire TV may otherwise keep the prior Activity process alive.
Never clear app data to recover an update: that loses retained workspaces/assets.
