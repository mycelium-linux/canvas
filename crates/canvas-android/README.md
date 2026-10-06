# fungOS Fire TV

The Fire TV client of fungOS, powered by Canvas's shared Rust core. The existing
`dev.fpl.canvas.tv` application ID and signing identity are preserved so this
is an in-place upgrade, not a second installed app.

## Shared ownership

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
adapters. The TV has no live selector or authenticated control
endpoint yet. Do not use a bundled Neo catalog to stand in for the TV's owner
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
CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER=/home/ajmwagar/.local/share/canvas-android-toolchain/android-ndk-r26d/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi24-clang \
cargo build --release --target armv7-linux-androideabi --features android-native-video
```

On the Mac, use a fresh packaging directory. Set `tv_package` to its path,
`tv_sdk` to the installed SDK, and `JAVA_HOME` to JDK 17. Copy the release
library from Agora into `$tv_package/lib/armeabi-v7a/`. From this crate directory:

```sh
mkdir -p "$tv_package/classes" "$tv_package/dex"
"$JAVA_HOME/bin/javac" -source 8 -target 8 \
  -classpath "$tv_sdk/platforms/android-33/android.jar" \
  -d "$tv_package/classes" android/dev/fpl/canvas/TvActivity.java
"$tv_sdk/build-tools/33.0.2/d8" --min-api 24 \
  --lib "$tv_sdk/platforms/android-33/android.jar" \
  --output "$tv_package/dex" "$tv_package/classes/dev/fpl/canvas/TvActivity.class"
"$tv_sdk/build-tools/33.0.2/aapt2" link \
  --manifest android/AndroidManifest.xml \
  -I "$tv_sdk/platforms/android-33/android.jar" \
  -o "$tv_package/unsigned.apk" --version-code 1 --version-name 0.1.0
cp -f "$tv_package/dex/classes.dex" "$tv_package/classes.dex"
cd "$tv_package"
zip -q unsigned.apk classes.dex lib/armeabi-v7a/libcanvas_android.so
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

Do not give javac the Android JAR as `-bootclasspath`: its API stubs lack Java's
lambda compiler runtime. D8 desugars the compiled listeners for the minimum API.

### Hardware verification (2026-10-06)

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
per-surface IDs, workspace projection and tiling are not implemented yet.
Windowed mode uses Canvas's shared layout solver with the measured Android
frame, and the decoder preserves aspect ratio inside those bounds.

This is an ADB-authorized local operation, **not** an authenticated Canvas
network control endpoint. Pairing, workspace projection and `canvasctl`
integration are not implemented by this shim. Never copy an owner-local DCP
token onto the TV to substitute for pairing.

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
