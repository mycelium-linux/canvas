# Canvas Web

View-only Canvas client for browser demos. Its default mode presents frames
rendered offscreen by the running native Canvas, which makes the workspace,
content, shader, chrome and selected theme pixel-identical to the TV. Press
`F9` to toggle the independent Rust/WASM Canvas2D renderer for an intentionally
lower-detail, low-bandwidth view.

The exact path is owner-local: `canvas-web-host` is hard-limited to loopback,
talks to Canvas through its owner-only Unix control socket, and returns only
rendered PNG bytes. Credentials and socket authority never enter the browser.
A runtime badge reports bridge failures instead of presenting a silent black
screen.

This first slice deliberately owns no durable workspace state and performs no
host actions. The JARVIS microphone and output controls emit typed intent only;
they do not request browser permissions or open an audio device.

## Run locally

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd crates/canvas-web
env -u NO_COLOR trunk serve --release
```

In a second terminal, with native Canvas running:

```sh
cargo run -p canvas-web --bin canvas-web-host --features native-host
```

Open <http://127.0.0.1:8088/>. `NO_COLOR=1` is removed because Trunk expects a
boolean value for that option.

## Verify

```sh
cargo test -p canvas-web
cargo check -p canvas-web --target wasm32-unknown-unknown
cargo test -p canvas-web --features native-host --target aarch64-apple-darwin
cd crates/canvas-web
env -u NO_COLOR trunk build --release
```

The screenshot bridge establishes visual parity first. The direct-rendering
successor is a retained snapshot plus ordered patch feed driving shared native
widget/layout code. It must resume by sequence or receive a fresh snapshot; it
must never pretend a fire-and-forget stream is current. Audio belongs behind a
separately authenticated WebAudio/WebRTC host that accepts `AudioIntent`.
