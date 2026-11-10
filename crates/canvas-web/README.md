# Canvas Web

The browser is another Canvas backend. This crate runs the same iced
application, scene renderer and message type as native Canvas; the only
difference is the renderer underneath (WebGL on wasm, wgpu on the desktop). It
contains no PNG mirror, no screenshot bridge, and no DOM re-implementation of a
Spore.

The first scene is the home Spore's checked-in fixture:
`scenes/home.scene.json`. That is the exact document `fungos.ui` hands to a
host (`{"kind":"scene","schema":1,"nodes":[...]}`), plus the host-owned
control list. A scene is data: a pointer layer hit-tests the control
rectangles, and pressing a tile emits a typed `canvas-intent` event to the page
and never opens a Spore itself.

WebAssembly has no system fonts. Iced's bundled Fira Sans is enabled for the
scene text, and `assets/NotoSansSymbols2-Regular.ttf` (SIL OFL 1.1, license
beside it) supplies the Spore's glyphs.

This slice is deliberately pre-Luau. Browsers cannot compile a module at
runtime, so the next slice ships a Luau wasm runtime and generates the same
document from the composed `spore-dev-home.lua`. The renderer does not change.

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
env -u NO_COLOR trunk build --release
```

A `cargo check` for wasm proves the iced scene renderer compiles against the
browser backend. `trunk build` produces `dist/`; serve it from any static origin
that is allowed to publish the Spore's wasm assets. Host actions still belong
to a separately authorized service: the page emits `canvas-intent`, and the
host decides whether and where to execute it.
