# Canvas Web

View-only Canvas client for browser demos. The whole-wall mirror presents
frames rendered offscreen by the running native Canvas, so shaders, chrome and
the selected theme are pixel-identical to the TV.

A named surface is different: `?surface=contacts` fetches that surface's
retained Canvas scene and draws it directly in the browser. It is the app
window, not a screenshot, and local module controls are activated through the
same host action path as a native click. There is no surface zoom step.

The bridge is owner-local: `canvas-web-host` is hard-limited to loopback,
talks to Canvas through its owner-only Unix control socket, and serves either
rendered PNG frames for the whole-wall mirror or retained scene JSON for one
surface. Credentials and socket authority never enter the browser. A runtime
badge reports bridge failures instead of presenting a silent black screen.

The browser never owns workspace state. Surface controls are requests routed
back to the same Canvas host action path a native click uses; the redraw comes
from the host's retained scene. The JARVIS microphone and output controls emit
typed intent only; they do not request browser permissions or open an audio
device.

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

The whole-wall screenshot path remains the parity baseline. Named surfaces use
the retained scene so the browser renders the window itself instead of
letterboxing a capture. A future patch feed can replace scene polling; it must
resume by sequence or receive a fresh snapshot, never pretend a fire-and-forget
stream is current. Audio belongs behind a separately authenticated
WebAudio/WebRTC host that accepts `AudioIntent`.
