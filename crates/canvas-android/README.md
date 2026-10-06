# Canvas Android platform shell

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

## Playback command after APK installation

```sh
adb -s 192.168.2.97:5555 shell am start \
  -n dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity \
  --es stream_url http://192.168.2.74:5004/auto/v4.1
```

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
