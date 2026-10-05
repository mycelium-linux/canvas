# Lua desktop shell

Canvas owns native discovery, focus, input and actions. Lua owns presentation
through the existing bounded `widget.panel`, `widget.label` and `widget.button`
primitives and Coherence theme tokens. The analog clock is unchanged.

The desktop bar and launcher use the same retained presentation host. Identical
source, manifest, inputs and theme reuse the previous output without executing
Lua. A candidate replaces the output only after validation; invalid edits report
an error and retain the last good scene. No file reads or Lua execution happen
inside the frame renderer.

## Iterate without rebuilding

Override these shipped artifacts under the Canvas runtime configuration root
(`CANVAS_CONFIG_DIR` overrides it; otherwise macOS uses
`~/Library/Application Support/Dock/canvas`, and Linux uses
`$XDG_CONFIG_HOME/dock/canvas` or `~/.config/dock/canvas`):

- `modules/desktop-bar-v1.json` and `modules/desktop-bar-v1.lua`
- `modules/desktop-launcher-v1.json` and `modules/desktop-launcher-v1.lua`

The desktop bar polls at one-second cadence. The launcher polls while open and
refreshes on input. Missing overrides use shipped artifacts; malformed or oversized
overrides are errors, not silent fallbacks. Each file is bounded to 32 KiB.

The bar receives width/height, workspace/page, clock, registered source count and
native action availability. It uses opaque themed chrome, reduces controls at
narrow widths, and reserves the same host-owned 44-pixel content inset. Registered
sources are labelled `SOURCES`, not `LIVE`: registration alone does not prove
fresh frames.

The four local controls are back, forward, launcher and create. Rust maps only
those IDs to existing shell actions and rechecks native availability on activation.
They do not depend on a Unibus connection and cannot invoke arbitrary commands.
Application discovery and execution remain revision-bound and target-bound.

## Validation and rollout

```sh
cargo check -p canvas --bin canvas --features lua-scenes
cargo test -p canvas --lib --features lua-scenes shell_presentation::tests
cargo test -p canvas --test desktop_launcher --features lua-scenes
```

Tests cover unchanged-input retention, invalid-edit recovery, theme/input/clock
invalidation, opaque responsive layouts and native-only controls.

Home Pi still runs the isolated older compatibility build. It has neither the
current launcher nor the flat Lua scene renderer. These source changes must not
be reported as deployed there until the compatibility/schema reconciliation in
`dock-yjt.2.2` has passed. Preserve the existing workspace backup and working clock.

Remaining shell migration: notification presentation and fleet rollout. Then
window interaction and owner-respecting application lifecycle follow in that order.
