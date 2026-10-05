# Canvas desktop launcher

One keyboard modal searches the selected Canvas instance's workspaces, current
Canvas surfaces, and native application entries. The analog clock is unchanged.
This is Canvas UI control, not a new DCP provider or an application-state owner.

## Use

With Canvas focused, press **Ctrl-Space**, **Ctrl-P**, or **Ctrl-Shift-P**.
Type to filter; arrow keys select; Enter opens; Escape closes. Tab changes the
destination screen; F5 explicitly refreshes its catalog. Keyboard and pointer
events are captured by the modal rather than forwarded to a remote window.
System-wide shortcuts remain the responsibility of the native shell adapter.

The destination appears at the top of the panel. Switching destinations clears
the old catalog immediately. Background refresh runs every six seconds; choices
expire after ten seconds and cannot execute while stale. Late replies from a
previous destination or a closed modal are discarded. Catalog revisions are
semantic digests, not frame counters; the destination rechecks the digest and
availability before invoking a choice. There is no fallback to another screen.

## Configure destinations

`shell/targets.json` under the Canvas runtime configuration root is optional.
The local screen is always available; configured targets are explicit Mycelium
peer identities, not addresses or a hidden presence default. The checked-in
example is for Neo and includes Studio and the TV:

```json
[
  {"id":"studio","label":"Studio","mycelium_peer":"darwin-averys-mac-studio"},
  {"id":"tv","label":"TV","mycelium_peer":"linux-home-pi","unix_user":"ajm"}
]
```

Use the appropriate remote peers on each installation; do not copy a target's
own peer into its remote list. `unix_user` is an explicit legacy-account override
and otherwise Mycelium owns SSH identity selection. Mycelium SSH carries a
bounded JSON request to the destination's `.local/bin/canvasctl`; both ends need
this launcher version. Errors are visible and do not silently select local.

`canvasctl launcher catalog` exposes the same typed catalog. JSON control offers
`shell_catalog` and `shell_activate` (catalog revision plus entry ID), rather than
arbitrary executable paths. Native applications are discovered from macOS app
bundles or Linux desktop files and launched through `/usr/bin/open` or `gio`.
Canvas does not implement its own desktop-file Exec parser. Native launches and
remote requests run off the rendering thread with bounded waits.

## Theme and iteration

`modules/desktop-launcher-v1.lua` and its manifest use the same bounded `widget`
primitives, semantic inks, typography and radius tokens as other Lua widgets.
Runtime overrides are read when the modal is opened; no Rust rebuild is required
to change its presentation. The embedded copy keeps packaged installs usable.
Lua receives display state only: it cannot choose peers, run commands or own
application state. Scenes are retained; Lua never executes in the draw path.

## Verification and rollout

Tests cover destination changes, late replies, stale/unavailable/busy choices,
generation uniqueness, themed Lua scenes, catalog revision rejection and input
capture. Run the Canvas library tests, `desktop_launcher` integration test, and
the binary's `launcher_` tests with `lua-scenes` enabled. Integration tests also
cause Cargo to compile sibling binary targets, so unrelated transport changes
must compile before that gate can pass.

Fleet installation is separate from source completion: reconcile the TV's native
compositor branch, build on Agora, deploy matching Canvas/canvasctl versions, and
verify keyboard focus and target switching on Neo, Studio and TV. Do not replace
a running native compositor with a legacy shell build merely to show this panel.
