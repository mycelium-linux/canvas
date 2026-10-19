# Canvas workspace UI

Host-embedded egui components for dense work, mail and Calendar views. The host
owns window/input/platform outputs, its GPU target, authenticated observations
and domain operations. `shell::Shell` is the synthetic demonstration host; it
cannot reach a mail service, credential store or sender. Its triage and draft
previews exist only in memory and reset with the demo.

The mail component emits exact account/thread intents. It supports scoped
search/splits, keyboard navigation, archive/star/unread/snooze, undo in the demo,
selected conversations, attachment labels, linked context and reply previews.
There is no Send intent. The work view reuses the existing virtualized Marbles
component. Calendar reuses `canvas-calendar`; all share Coherence appearance.

```sh
cargo run --manifest-path crates/canvas-workspace-ui/Cargo.toml --features native --bin canvas-workspace-demo
cargo run --manifest-path crates/canvas-workspace-ui/Cargo.toml --features preview --example render -- /tmp/mail.png 1600 1000 dark mail
cargo test --manifest-path crates/canvas-workspace-ui/Cargo.toml
```

The optional native application is a preview window, not an installed replacement
for the Canvas Linux/Fire TV host. The screenshot example uses the shared gfx
egui pass on a caller-owned target; readback occurs only to export the QA PNG.

For the WebGPU preview:

```sh
cargo build --manifest-path crates/canvas-workspace-ui/Cargo.toml --target wasm32-unknown-unknown --features web --lib
wasm-bindgen --target web --out-dir crates/canvas-workspace-ui/preview/pkg TARGET/wasm32-unknown-unknown/debug/canvas_workspace_ui.wasm
```

Replace `TARGET` with the configured Cargo target directory. Serve `preview/`
over localhost or HTTPS using an existing static server. Browser/WebGPU runtime
acceptance is separate from compilation. This work does not activate real
Gmail access or silently modify the selected Fire TV workspace.

Feature-level references: [Macro's public workspace description](https://github.com/macro-inc/macro#features)
and its public inbox screenshot. No Macro implementation, CSS, themes, icons,
assets or AGPL dependency is included. Fixture contacts and correspondence are
synthetic; the three account labels are the user's explicitly requested set.
