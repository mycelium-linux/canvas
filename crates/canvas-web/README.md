# Canvas Web

The browser is the hosted Mycelium desktop. It runs the same iced application
and renderer stack in WebAssembly, loads `/v1/desktop` from the same origin,
and draws the top bar, desktop, welcome transition, and prompt popup. There is
no PNG mirror, no screenshot bridge, and no DOM re-implementation of a Spore.

The checked-in `scenes/home.scene.json` and its iced scene renderer remain as
the compatibility fixture and regression tests. The hosted shell is the
replacement path: JSON from the control plane, then Canvas/WASM presentation.

WebAssembly has no system fonts. Iced's bundled Fira Sans is enabled for the
scene text, and `assets/NotoSansSymbols2-Regular.ttf` (SIL OFL 1.1, license
beside it) supplies the Spore's glyphs.

The welcome/profile copy currently comes from `/v1/desktop`. The onboarding
Spore is the next slice; it will render through this shell rather than growing
a second onboarding surface.

## Run locally

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd crates/canvas-web
env -u NO_COLOR trunk serve --release
```

Open <http://127.0.0.1:8088/>. `NO_COLOR=1` is removed because Trunk expects a
boolean value for that option. The page does not need the native app, a control
socket, or a loopback bridge.

## Regenerate the home scene

The scene is generated from the Spore's own fixture, not hand-authored here.
From the `spores` checkout:

```sh
cd spores/home
lua tools/dump_scene.lua > ../../../canvas/crates/canvas-web/scenes/home.scene.json
```

Run the generator whenever `home/ui/home.lua` or `sdk/ui/ui.lua` changes. The
`canvas-web` tests fail if the launcher's landmark text or the nineteen
openable controls disappear from the fixture.

## Verify

```sh
cargo test -p canvas-web
cargo check -p canvas-web --target wasm32-unknown-unknown
cd crates/canvas-web
FPL_BOB_LOCAL=1 env -u NO_COLOR trunk build --release
```

A `cargo check` for wasm proves the iced scene renderer compiles against the
browser backend. `trunk build` produces `dist/`; serve it from any static origin
that is allowed to publish the Spore's wasm assets. Host actions still belong
to a separately authorized service: the page emits `canvas-intent`, and the
host decides whether and where to execute it.
