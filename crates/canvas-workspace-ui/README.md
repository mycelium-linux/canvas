# Canvas workspace UI

Host-embedded egui components for dense work, mail and Calendar views. The host
owns window/input/platform outputs, its GPU target, authenticated observations
and domain operations. `shell::Shell` is the synthetic demonstration host; it
cannot reach a mail service, credential store or sender. Its triage and draft
previews use local preview storage and reset with the demo. The embedded
components still own no filesystem paths or authentication.

The mail component emits exact account/thread intents. It supports scoped
search/splits, keyboard navigation, archive/star/unread/snooze, undo in the demo,
selected conversations, attachment labels, linked context and reply previews.
Use X or row checkboxes for bulk triage; the toolbar applies to the selected
rows in the current account/filter/search scope. A bulk change validates all
identities before mutation and undoes as one operation. Archive keeps the
selection at its list position, advancing to the next remaining conversation.
There is no Send intent. The work view reuses the existing virtualized Marbles
component. Calendar reuses `canvas-calendar`; all share Coherence appearance.

```sh
cargo run --manifest-path crates/canvas-workspace-ui/Cargo.toml --features native --bin canvas-workspace-demo
cargo run --manifest-path crates/canvas-workspace-review/Cargo.toml -- /tmp/mail.png 1600 1000 dark mail
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

The native and browser hosts checkpoint demo flags, snooze deadlines, saved
previews and unfinished compose/reply buffers through eframe storage every
second and on normal exit. Native storage uses the OS application-data location;
browser storage is local to the preview origin. This is mock data, with no token
or credential persistence. A checkpoint must validate and round-trip before it
replaces existing state; invalid or unsupported saved state fails startup
explicitly. Undo history is session-local.

To exercise recovery: type a reply, switch conversations, create a new draft
with an explicit From account, wait for the next checkpoint, then close and
reopen the preview. Use the Draft previews split to find reply drafts. Snooze
a conversation and choose “Advance one day” to verify its return to Inbox;
real host time also wakes overdue snoozes after a restart. “Reset demo” clears
mock triage and draft state on the next checkpoint. Screenshot exports always
start from deterministic fixtures and never load persisted preview data.

Feature-level references: [Macro's public workspace description](https://github.com/macro-inc/macro#features)
and its public inbox screenshot. No Macro implementation, CSS, themes, icons,
assets or AGPL dependency is included. Fixture contacts and correspondence are
synthetic; the three account labels are the user's explicitly requested set.
