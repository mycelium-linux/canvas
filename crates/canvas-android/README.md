# fungOS Fire TV

The Fire TV client of fungOS, powered by Canvas's shared Rust core. The existing
`dev.fpl.canvas.tv` application ID and signing identity are preserved so this
is an in-place upgrade, not a second installed app.

## IPFS PDF workspace

The `Attention` and `HMM` projects in `config/deploy/firetv/workspaces.json` contain
the same PDFs used on Home-Pi, one document per full-width reading workspace.
Their source CIDs are recorded
in Unibus's `docs/ipfs-adapter.md`. Retrieve approved documents through the
existing `unibus-ipfs fetch`/`download` interface; provision the returned bytes
over authorized ADB into the app-private `files/assets/SHA256` directory.
Do not expose Kubo RPC to the TV or treat a UnixFS CID as the file SHA256.

These are existing shared `AssetRef` surfaces, not a separate Android document
schema. Rust validates MIME, size (at most 64 MiB), regular-file type and SHA256
before the native Android PdfRenderer opens a document. Cached assets are
immutable and named only by the validated digest; no arbitrary paths or URLs.
The Android shim rasterizes off the UI thread with one worker and a bounded
queue, renders at reading width (bounded to 1920 × 2560), and releases raster resources
when switching workspaces. The controls inherit the shared Canvas palette.

Use the remote's D-pad to scroll the page and OK to click previous/next page controls.
The workspace strip is 32 dp tall; the page toolbar is 28 dp. Full-width rendering
avoids shrinking an entire portrait page to the TV's landscape height.
Portrait-shaped tiles initially fit the complete page; landscape-shaped tiles
initially use reading width. The `Read` / `Fit` control switches between these
presentations using the cached raster, without another PDF decode. Thus a
half-screen résumé can show its whole page while dense full-width documents
remain scrollable. This sizing choice is local renderer state, like the page cursor.
Single-page PDFs automatically hide the bottom toolbar, filename and `1 / 1`.
Focus the document with the remote's D-pad; OK toggles Fit/Read without needing
on-screen buttons. Up/down scrolls the enlarged reading view. Multi-page
documents retain remote-focusable page controls.
Page cursors currently belong to the local PDF renderer: they are **not** yet
persisted in the shared view or synchronized across screens. Workspace selection
is retained; returning to the PDF workspace starts each document at page one.
The TV uses verified local copies after provisioning, not continuous IPFS I/O.

```sh
printf 'workspace switch ipfs-reading\n' | canvas-tvctl 192.168.2.97:5555
```

## Shared ownership

### Presence-aware ambient screen

`ambient bind main_room` explicitly binds this physical display to a room.
The room setting persists; observations and active screensaver state do not.
Only an authorized owner transport can submit `ambient presence` samples.
The `ambient-stream` CLI accepts only occupancy lines, not workspace commands:

```text
ambient presence main_room empty 1792425600000 5000
```

The timestamp is the actual sensor observation time in Unix milliseconds,
not a delivery heartbeat. The example timestamp is illustrative. Frames expire
after at most five seconds on the receiver's monotonic clock. Future timestamps
allow at most two seconds of host clock skew without extending that lease.
Larger future offsets, expired,
conflicting, regressed and wrong-room observations are rejected. Replaying a
known observation cannot extend its deadline. Empty requires 60 seconds of
continuous fresh evidence. Unknown evidence or a delivery gap resets that timer.
Once asleep, losing evidence keeps the screen dark; occupied or human input
wakes it. Remote wake consumes the complete key gesture and allows two minutes
before empty observations can start a new absence timer.

The initial ambient rendering is black. It releases local video decoders and
PDF panels, stops the GPU background, and hides the workspace chrome. It does
not change the selected workspace, retained document or page. Waking rebuilds
the selected view. It is a screensaver, not an authentication lock or a command
to power off the TV. A remote Surf producer may continue running upstream.

Manual owner commands are `ambient on`, `ambient off`, and `ambient unbind`.
Inspect reports `data.ambient`; acceptance is intent, not a renderer screenshot.
Use live visual verification after changes. Unbinding disables automatic
ambient mode and clears transient state.

JARVIS owns the source adapter `jarvis-presence-canvas-watch`. It reads the full
owner publication, verifies the exact room and evidence quality, then invokes
the installed Canvas CLI. Do not substitute the older compact presence summary:
it does not preserve the confidence that distinguishes unavailable from empty.
No identities or camera images enter this display-only path.

`config/deploy/firetv/ambient-main-room.plist` supervises the adapter on Studio
using the existing Frigate publication. Install the tested adapter and matching
CLI, upgrade the TV APK in place, bind the room, then bootstrap that LaunchAgent.
Studio cannot directly reach the Omni's LAN address. This deployment invokes
Neo's installed CLI through an authenticated SSH stdin stream. Its dedicated
Ed25519 key is restricted in Neo's `authorized_keys` to the exact
`canvas-tvctl 192.168.2.97:5555 ambient-stream` command, with no PTY or forwarding.
The forced command supplies the existing ADB executable search path. Verify the
Neo host-key fingerprint before pinning it; the adapter requires strict host-key
checking and uses only its dedicated key. Install updated executables through
a new inode and atomic rename so macOS does not retain a stale code-signing
cache for an overwritten executable.
If delivery fails, inspect the service error log and the source owner's health;
do not restart or seize another active TV app. For rollback, boot out only that
LaunchAgent and send `ambient unbind`; keep the prior signed APK.

### Transient assistant observations

`observer_frame` accepts a bounded renderer-neutral activity frame through the
existing authenticated endpoint. It neither mutates the workspace nor grants
control authority. Freshness expires on the receiver's monotonic clock (at most
five seconds); `frame: null` clears it. Same-session revisions cannot regress or
conflict. Shared `fpl_gfx::voice_popup` owns the same 340×54 capsule proportions,
colors, heard-text bounds and reveal/dismiss timing as the native Mac popup;
the shared reactor visitor owns its animated geometry. Android owns only the
platform painting and lifecycle. The capsule shows one verbatim heard-text line,
never route, reply or diagnostic text, and briefly retains the final line on
idle. It stays above the selected workspace rather than replacing it.
Observer updates are excluded from the layout
fingerprint so they do not restart video players or reconstruct PDF views.

This renderer change requires a native-video APK containing both the Rust
descriptor export and Java painting adapter. Merely streaming an observation
to an older installed APK does not upgrade its chrome or make its caption visible.

`canvas-tvctl SERIAL observer-stream` consumes newline-delimited observer-only
requests with one retained authorized ADB forward. Pair it with JARVIS's
`jarvis-reactor-watch CLIENT_CONFIG --canvas-source PRODUCER` projection.
This does not itself configure UniBus grants or enable the JARVIS publisher.

Desktop `canvasctl observer-stream` accepts those same bounded JSON lines and
uses its existing owner socket to present the shared capsule. It rejects all
workspace/control commands on that input. Fresh observation heartbeats renew
the `jarvis-voice` display slot once per second without restarting the reveal;
the desktop renderer independently removes that slot five seconds after its
last upsert. EOF, expiry and invalid input clear the presentation. Idle retains
the final heard line for 1.4 seconds, once per activity transition; repeated
idle heartbeats do not show it again. Updating either renderer preserves the
current workspace selection and documents.

### Workspace header slot

A workspace may declare an unplaced `status_list` surface with the reserved ID
`workspace-header`. Its transient `SurfaceSnapshot` title occupies the right side
of the TV's top bar. Producers use the same authenticated ingress, selected-owner
admission and receiver-owned freshness as other surfaces. The header is not
persisted as live state; stale or disconnected producers remain visibly marked.
No app-specific track lookup or playback logic lives in the TV shell. Workspaces
without the surface retain the full-width selector. Header updates do not rebuild
signal views or PDFs.

### Renderer-confirmed source audio control

The authenticated app-private endpoint also accepts `audio_inspect` and
`audio_set_muted`. These are source-local gain controls, not Android master
volume. `audio_inspect` reports the renderer's current surface, opaque stream
identity, title, readiness, applied mute state, revision and pending request.
For example, send this JSON through `canvas-tvctl SERIAL`:

```json
{"schema_version":1,"request_id":"audio-observe","expected_revision":null,"command":{"command":"audio_inspect"}}
```

To set gain, use `audio_set_muted` with `source` copied exactly from that
observation, a boolean `muted`, and the observed `expected_revision`. Unknown
fields and stale sources are rejected. Requests remain pending until the UI
thread applies gain to that player and acknowledges the same request/revision.
Switching streams, losing the player, leaving the app, or human gain changes
invalidate pending work. A timeout is an unknown outcome, not permission to
retry a mutation automatically. Always inspect again for confirmation.

The native shell outlines the exact current player while an admitted audio
operation is pending, then briefly marks a renderer acknowledgement. Confirmed
notices expire after two seconds and are revoked by source, readiness or human
gain changes. This is owner evidence, never inferred from assistant captions.

The existing `canvas-dcp` binary has an explicit Android audio backend selected
by `CANVAS_DCP_ANDROID_SERIAL`; `CANVAS_TVCTL_BIN` selects the installed owner
transport helper. Configure a distinct `CANVAS_DCP_PROVIDER_ID` and
`CANVAS_DCP_INSTANCE` for this physical target. Do not reuse Home-Pi's
`canvas-tv` identity or present its workspace state as Fire TV evidence.
The action schema derives exact surface/stream options and semantic title
from live app state. Only final-transcript commit requests are eligible.
`Applied` requires a correlated owner acknowledgement and fresh renderer
readback of the requested mute state. An old app without these endpoints
fails explicitly; it must be upgraded before publishing this provider.

`canvas-core::workspace_layout` owns durable layout types and validation;
`canvas-core::layout_tree` owns the BSP solver and mutations. Desktop's old
`canvas::layout_tree` import is a compatibility re-export, not a fork. Android
applies the pixel rectangles returned by that same solver; Java contains no
video sizing/tiling policy.

`canvas-core::workspace_navigation` owns bounded MRU history. Both desktop
`WorkspaceStore` and the Android adapter call it. Android's optional
`shared-workspaces` adapter also invokes the existing core Catalog and
WorkspaceState admission/control policy. `android-native-video` enables this
shared dependency, so the installed video shell uses it for geometry.

Full documents, scene intent, charts, surfaces, theme references, page switching,
validation and revision-bound patches now live in renderer-neutral
`canvas-workspace`. Desktop's former type and chart paths re-export that model.
The Android workspace adapter re-exports the same owner-bound session, including
complete-document retention and transactional patches. Widget GPU dependencies
are enabled only by `canvas-widgets/live`, not document parsing.

Desktop filesystem storage, GPU resources and Android playback remain platform
adapters. The TV has a native remote-focusable workspace selector and an
ADB-authorized, app-credential control path. Do not use a bundled Neo catalog to stand in for the TV's owner
catalog, or infer presentation success from metadata selection alone.

Apple TV feasibility: the shared core type-checks for `aarch64-apple-tvos`, and
the Mac has the tvOS SDK. A future client can reuse the same core with native
AVPlayer/Metal presentation and platform focus/input. This does not constitute
a tvOS app, signing/provisioning, physical-device install or playback test.
See [Rust tvOS support](https://doc.rust-lang.org/rustc/platform-support/apple-tvos.html)
and [Apple tvOS development](https://developer.apple.com/tvos/get-started/).

This is an independent Android package, not a compositor dependency. The
`android-native-video` Cargo feature is **off by default** and exports the JNI
entry point used by `android/dev/fpl/canvas/TvActivity.java`. Package the Java
shim only with that feature enabled; a core-only library is not a runnable APK.

The native UI exception is scoped to Android video surfaces, lifecycle and
remote input. Rust validates playback commands. This follows ohmOS's JNI
boundary without importing its phone permissions, Slint UI or audio policy.
The app uses Android VideoView/MediaPlayer and the TV's normal media audio
route; it does not send audio through Isochrone. Hardware codec selection is
owned by Android and must be verified on the actual TV, not inferred from the
API name. There is no forced maximum volume.

## Validation

On Agora, use a disk-backed target directory (not the quota-limited `/tmp`):

```sh
cargo check --manifest-path Cargo.toml --features android-native-video
cargo check --manifest-path Cargo.toml --target armv7-linux-androideabi --features android-native-video
cargo test --manifest-path Cargo.toml --features android-native-video
```

Compile Java against the installed Android platform JAR with JDK 17. The Omni
reports API 28 and `armeabi-v7a`; package the matching Rust shared library as
`lib/armeabi-v7a/libcanvas_android.so` alongside the shim's dex and manifest.

### Reproducible APK packaging

Build the Rust shared library on Agora with the Android NDK linker and the
explicit feature. NDK r26d matches the existing ohmOS toolchain. Use the
disk-backed Cargo target directory:

```sh
CARGO_TARGET_DIR=/home/ajmwagar/src/fpl/dock/target/canvas-android \
CC_armv7_linux_androideabi=/home/ajmwagar/.local/share/canvas-android-toolchain/android-ndk-r26d/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi24-clang \
AR_armv7_linux_androideabi=/home/ajmwagar/.local/share/canvas-android-toolchain/android-ndk-r26d/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar \
CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER=/home/ajmwagar/.local/share/canvas-android-toolchain/android-ndk-r26d/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi24-clang \
cargo build --release --lib --target armv7-linux-androideabi --features android-native-video
```

On the Mac, use a fresh packaging directory. Set `tv_package` to its path,
`tv_sdk` to the installed SDK, and `JAVA_HOME` to JDK 17. Copy the release
library from Agora into `$tv_package/lib/armeabi-v7a/`. From this crate directory:

```sh
mkdir -p "$tv_package/classes" "$tv_package/dex" "$tv_package/assets"
cp ../../config/deploy/firetv/menu.json "$tv_package/assets/canvas-menu.json"
"$JAVA_HOME/bin/javac" -source 8 -target 8 \
  -classpath "$tv_sdk/platforms/android-33/android.jar" \
  -d "$tv_package/classes" android/dev/fpl/canvas/*.java
"$tv_sdk/build-tools/33.0.2/d8" --min-api 24 \
  --lib "$tv_sdk/platforms/android-33/android.jar" \
  --output "$tv_package/dex" "$tv_package/classes/dev/fpl/canvas/"*.class
"$tv_sdk/build-tools/33.0.2/aapt2" compile --dir android/res \
  -o "$tv_package/resources.zip"
"$tv_sdk/build-tools/33.0.2/aapt2" link \
  --manifest android/AndroidManifest.xml \
  -I "$tv_sdk/platforms/android-33/android.jar" \
  -o "$tv_package/unsigned.apk" --version-code 1 --version-name 0.1.0 \
  "$tv_package/resources.zip"
cp -f "$tv_package/dex/classes.dex" "$tv_package/classes.dex"
cd "$tv_package"
zip -q unsigned.apk classes.dex lib/armeabi-v7a/libcanvas_android.so assets/canvas-menu.json
"$tv_sdk/build-tools/33.0.2/zipalign" -f 4 unsigned.apk aligned.apk
"$tv_sdk/build-tools/33.0.2/apksigner" sign \
  --ks "$tv_keystore" --ks-key-alias canvas-tv-debug \
  --out canvas-tv.apk aligned.apk
"$tv_sdk/build-tools/33.0.2/apksigner" verify canvas-tv.apk
adb -s 192.168.2.97:5555 install -r canvas-tv.apk
```

`tv_keystore` is the development signing key outside the repository. Reuse it
for upgrades; let apksigner prompt for its password rather than recording
production secrets in a command or runbook. The currently installed build is
development-signed, not a release-distribution artifact.

For upgrades, read the installed version with `adb shell dumpsys package
dev.fpl.canvas.tv` and select an equal or higher version code instead of the
initial-install example above. Package `canvas-menu.json` with every APK:
omitting it silently falls back to a flat project list and loses the configured
navigation groups, though the saved workspaces remain intact.

Do not give javac the Android JAR as `-bootclasspath`: its API stubs lack Java's
lambda compiler runtime. D8 desugars the compiled listeners for the minimum API.

### Hardware verification (2026-10-06)

Reactor upgrade `0.1.28-reactor` (version code 29) was installed in-place on the
Omni using the existing development certificate. The selected Taig Simulator
workspace and endpoint revision 255 survived the upgrade. Authenticated owner
observations painted the shared reactor on the actual TV, then expired without
advancing the workspace revision. Restoring the pre-upgrade source-local mute
produced renderer-confirmed readback and a visible exact-player confirmation
outline. The latest Android suite passed 18 tests. Live JARVIS UniBus publication
is a separate deployment step; this smoke used explicitly labelled test frames.

Installed successfully on Omni `AFTTIFF43`, API 28, ARMv7. An ADB screenshot
verified full-screen KOMO playback. `dumpsys media.player` identified
`OMX.MS.MPEG2.Decoder` and `OMX.MS.AC3.Decoder`, stereo 48 kHz AudioTrack routed
device 2, with zero video drops in an initial 314-frame sample. That is not an
end-to-end latency or sustained-performance benchmark. Initial buffering was
observed; audible speaker quality still needs human confirmation.

## Playback command after APK installation

```sh
adb -s 192.168.2.97:5555 shell am start \
  -n dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity \
  --es stream_url http://192.168.2.67:18089/komo4.m3u8
```

Add `--ez windowed true --ez muted true` for the smaller, initially muted
KOMO surface. The visible button toggles the player's own gain; Rust owns
its mute state. This does not change TV master volume. The TV master mute is
also currently enabled at the user's request, so unmuting the surface alone
will not override that. This initial client has one audio surface; general
per-surface audio IDs are not implemented yet. Workspace geometry now comes
from the active shared document's layout.
Windowed mode uses Canvas's shared layout solver with the measured Android
frame, and the decoder preserves aspect ratio inside those bounds.

This is an ADB-authorized development operation, **not** production LAN pairing.
Never copy an owner-local DCP token onto the TV to substitute for pairing.

## Workspace control

The development APK enables `run-as` through `android:debuggable`. Rust generates
a private 0600 app credential, listens only on 127.0.0.1:27841 and accepts bounded
newline-framed presentation requests. `canvas-tvctl` obtains that credential
through an already-authorized, explicitly addressed ADB connection; it neither
prints the credential nor exports a LAN control listener. Ephemeral ADB forwards
are removed on exit. Do not ship this development provisioning method as a
production pairing solution.

Build the controller with:

```sh
cargo build --manifest-path crates/canvas-android/Cargo.toml \
  --bin canvas-tvctl --features shared-workspaces
printf 'inspect\n' | canvas-tvctl 192.168.2.97:5555
printf 'workspace install config/deploy/firetv/workspaces.json\n' | canvas-tvctl 192.168.2.97:5555
printf 'workspace switch clock\n' | canvas-tvctl 192.168.2.97:5555
printf 'workspace switch television\n' | canvas-tvctl 192.168.2.97:5555
```

The source `canvasctl --android ADB_SERIAL` delegates to this controller.
Existing installed desktop CLIs must be upgraded before using that option;
the standalone controller is installed on Neo and avoids any local-catalog
fallback. Text mutations inspect the same explicit TV target first. JSON
requests preserve their supplied expected revision and fail when stale.

`canvas-workspace::endpoint` owns commands, validation, revisions and receipts,
independent of ADB. The Android remote and controller drive that same session.
Workspace documents are atomically checkpointed in the app's private storage;
invalid imports/patches retain the previous document. MENU focuses the selector;
D-pad/OK switches between retained workspaces. Only the active workspace is
presented. Supported: one hardware stream, digital or analog clock, local PDF assets and
text scene summaries. Other kinds display explicit unsupported placeholders; this
is not the full desktop GPU widget renderer.

The analog clock reuses `fpl-gfx::AnalogClock` geometry and semantic theme roles.
Android paints the shared primitives using its native Canvas, refreshing at
25 Hz while visible; it does not maintain a separate clock design.

### Native GPU backgrounds

With `android-native-video`, the app renders `tidepool`, `aurora-drift`,
`dusk-ember`, and `slow-ripple` backgrounds using OpenGL ES 3.1+. The canonical
WGSL sources live in `canvas-workspace::background_shaders` and are also used
by the desktop renderer; Naga translates them to GLES in Rust. Android owns
only EGL resources, presentation, and lifecycle. Other shaders fail visibly;
there is no downloaded shader execution.

Use the existing revision-bound `apply_patch` endpoint with a `set_background`
operation, for example `{"kind":"shader","shader_id":"tidepool","speed":1}`.
The active Coherence background color tints the shader. Speed zero draws a
stationary frame; switching to `{"kind":"theme"}` removes the GPU layer.

The independent background surface sits below the hardware video surface and
native widgets. Its buffer is aspect-preserving and bounded to 1280×720; PDFs
and UI remain at the measured display resolution. PDF fit margins are transparent,
so only the opaque paper covers the background. One frame completes before
the next is paced, capped at approximately 30 FPS, with no CPU readback. The
loop pauses offscreen and releases EGL resources when its surface disappears.
`adb logcat -s CanvasBackground` reports the GPU, buffer extent, achieved FPS,
and mean frame work (including completion wait, not a GPU timestamp query).

Preserve the TV's existing logical display size when deploying Canvas. Do not
run `adb shell wm size reset` to sharpen PDFs: this device-wide change can make
other streaming apps render 1080p video in the top-left quarter of a 4K screen.
On the qualified FireTV, `adb shell wm size 1920x1080` restores the compatible
UI size while the physical panel remains 3840×2160. Improve PDF framing within
Canvas instead. Inspect both the logical window and the hardware video display
frame when verifying playback; a successful Canvas screenshot is insufficient
to qualify unrelated streaming apps.

The endpoint's response confirms accepted presentation state, not completed
hardware presentation. Verify the visible surface separately. A future
Unibus Canvas adapter should carry this narrow contract, not own application
state or discover devices. Mycelium can advertise distinct display, media and
control capabilities; advertisement must never grant control permission.

URLs must use an explicit private IPv4 address and HTTP, without embedded
credentials or fragments. Errors are displayed rather than silently replacing
the source. Leaving the app releases playback and the tuner; opening the app
without a stream shows a selection prompt. HDHomeRun direct playback can
consume a second tuner alongside the existing Pi stream. Stop or reuse the
old source only after live TV playback and audio verification.

## Live HDHomeRun projection

Direct MPEG-TS playback decoded successfully but Android's HTTP reader
repeatedly attempted a read/reconnect around byte 20,709,376, raised a
ProtocolException and reached end-of-stream. Do not use that path as the
verified deployment. The live Omni uses an HLS projection from Agora instead:

- HDHomeRun private proxy remains on Home-Pi; its Fire TV discovery path is
  unchanged.
- Agora consumes one tuner connection, NVENC-encodes 1280×720 H.264 at 30 fps
  and produces AAC stereo 48 kHz.
- One-second segments, eight-segment live playlist, and deletion of old
  segments bound disk retention. This adds seconds of live delay; it is not
  the low-latency external GPU transport.
- Existing BusyBox httpd serves only the projection directory, bound to
  `192.168.2.67:18089`. `android/hls-httpd.conf` permits only Omni and Agora's
  LAN IPs. A request from Neo was verified to return 403.
- Omni hardware H.264 decoder is `OMX.MS.AVC.Decoder`; AAC uses Android's
  `OMX.google.aac.decoder`. Both video presentation and audio playback are on
  the TV. User confirmed visible picture and audible TV sound.

Current Agora user units are transient (not reboot-persistent):
`canvas-tv-komo4-hls.service` and `canvas-tv-hls-http-omni.service`.
The old `canvas-tv-komo4-transcode.service`, `canvas-tv-audio-gateway.service`
and Pi `canvas-tv-audio.service` were stopped; other Isochrone services and the
Pi compositor were left unchanged. The HLS producer currently remains active
when the app closes; source lease/lifetime wiring belongs in the control
integration, not the Java shim.

To recreate the development projection on Agora:

```sh
mkdir -p /home/ajmwagar/.local/share/canvas-tv-hls
systemd-run --user --unit=canvas-tv-komo4-hls --property=Restart=on-failure \
  /usr/bin/ffmpeg -nostdin -hide_banner -loglevel warning \
  -rw_timeout 15000000 -threads 2 \
  -i http://192.168.2.74:5004/auto/v4.1 -map 0:v:0 -map 0:a:0 \
  -vf scale=1280:720,fps=30,format=nv12 \
  -c:v h264_nvenc -preset p1 -tune ull -profile:v main \
  -b:v 5M -maxrate 5M -bufsize 1M -g 30 -bf 0 \
  -c:a aac -b:a 160k -ac 2 -ar 48000 \
  -f hls -hls_time 1 -hls_list_size 8 -hls_delete_threshold 12 \
  -hls_flags delete_segments+independent_segments+temp_file \
  -hls_segment_filename /home/ajmwagar/.local/share/canvas-tv-hls/segment-%09d.ts \
  /home/ajmwagar/.local/share/canvas-tv-hls/komo4.m3u8
```

Copy `android/hls-httpd.conf` to
`/home/ajmwagar/.local/share/canvas-tv-hls-httpd.conf`, then:

```sh
systemd-run --user --unit=canvas-tv-hls-http-omni --property=Restart=on-failure \
  /usr/bin/busybox httpd -f -p 192.168.2.67:18089 \
  -h /home/ajmwagar/.local/share/canvas-tv-hls \
  -c /home/ajmwagar/.local/share/canvas-tv-hls-httpd.conf
```

Stop the matching unit before recreating it; do not start duplicate producers.
The allowlist is a development constraint, not pairing or cryptographic
authentication. Do not expose this endpoint publicly.

### Grouped FireTV workspace menu

`config/deploy/firetv/menu.json` is the presentation-only menu descriptor.
Its items activate existing shared workspace IDs; sections reveal child choices
inline without reconstructing the active content. The default groups are Home
(Résumé), Studio, TV & AirPlay, and Toys. PDF demos and Clock remain retained and
CLI-addressable, but are not shown in this menu. Library appears only in Studio.
With `include_unlisted`, new catalog projects appear under Other unless their
IDs are in `hidden_projects`; explicit sections do not freeze discovery.

Include the descriptor in a native-video APK **before zipalign and signing**:

```sh
mkdir -p "$tv_package/assets"
cp -f config/deploy/firetv/menu.json "$tv_package/assets/canvas-menu.json"
(cd "$tv_package" && zip -q unsigned.apk assets/canvas-menu.json)
```

The descriptor is bounded to 64 KiB, with at most 32 top-level items and 32
children per section. An APK without a descriptor retains the flat catalog menu;
an invalid descriptor surfaces an explicit menu error rather than silently
inventing navigation. Menu labels do not rename or delete shared projects.
