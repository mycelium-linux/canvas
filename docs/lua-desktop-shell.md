# Lua desktop shell

Canvas owns native discovery, focus, input and actions. Lua owns presentation
through the existing bounded `widget.panel`, `widget.label` and `widget.button`
primitives and Coherence theme tokens. The analog clock is unchanged.

## Local application pins

Right-click a local application row in the launcher to pin/unpin it. Right-click
a pinned dock icon to remove it. Clicking the icon invokes the same fresh,
revision-bound native launch path as the launcher. Launching an app is not window
attachment or input forwarding; those remain separate capabilities.

Pins are ordered and saved under `shell/dock-pins.json` in the local runtime root,
not copied to other machines. The first slice permits four pins and retains the
existing responsive six-item dock limit. Native macOS icons come from read-only
AppKit lookup and are decoded once into bounded cached images. A single background
worker owns lookup and encoding; stale pin generations are discarded and failures
are negatively cached, so icon work cannot hold the shell update thread. Lua cannot name
an image file. Captured windows still use their existing live textures, and an app
pin does not receive a fabricated running indicator.

The CLI supports `dock-pins`, `dock-pin <catalog-revision> <entry-id>`, and
`dock-unpin <catalog-revision> <entry-id>`. Read `shell-catalog` on the intended
host first. JSON equivalents are `shell_dock_pins` and `shell_dock_pin` with
`catalog_revision`, `entry_id`, and `pinned`. A stale catalog fails explicitly;
unknown applications cannot become executable pins. Uninstalling an application
does not erase the saved pin; after reload it is shown disabled and can be removed.

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
- `modules/desktop-dock-v1.json` and `modules/desktop-dock-v1.lua`

## Neo bottom dock

The floating 68-pixel dock sits 12 pixels above the bottom edge. Apps opens the
existing target-labelled application/workspace selector. Up to six actual Canvas
windows fit beside it, with responsive overflow available through Apps. A short
accent underline means focused, a dot means open, and a dim dot means minimized.
These are Canvas window states, **not** invented native process or stream-health
states. Frameless instruments are excluded from the dock: the clock and desktop
identity are content, not applications. Selecting a window restores it, activates its page, and raises it through
the same native focus path as the launcher. Presentation reloads at one-second
cadence; unchanged state retains its output and never executes Lua in `view`.

Launcher rows now support clicks as well as arrows/Enter. Clicks carry entry ID,
target generation and catalog revision; stale/unavailable/busy choices are refused.
Source discovery includes macOS Utilities folders (the first Neo release still
uses the older discovery list until the next binary rollout). Iced-captured
shell clicks are not also forwarded into underlying remote windows.

`config/workspaces/neo/fungos.json` is a quiet local desktop using the existing
analog clock, theme-derived background and bounded Lua welcome module. It does
not change the renderer or the saved TV workspace. Deploy the welcome artifacts
to the runtime modules directory before applying that workspace. This first dock
does not yet pin native apps, show their icons, or claim OS-wide running status.

Shell controls use `widget.button({appearance="quiet", ...})`: borderless at
rest, with native hover/pressed/selected/disabled feedback derived from Coherence.
Omitting `appearance` preserves the standard treatment. Unknown appearances are
rejected; arbitrary CSS/colors/callbacks are not part of the Lua button API.
This requires the updated native host before installing the new Lua files.

## Window switching

Shell layouts now use `widget.pixel_space(width, height)` for logical-pixel
geometry. `widget.icon_button` shares native Coherence hover, selection, and
disabled treatment. Its bounded `icon` vocabulary is `apps`, `window`,
`terminal`, `document`, `browser`, and `chart`; `icon_size` is 8–96 logical pixels.
`icon_caption=true` adds a centered, Unicode-safe shortened label, while the
hover tooltip retains the full supplied label. No arbitrary image paths, SVG,
colors, or native callbacks enter the Lua control contract.

Alt-Tab is a dedicated horizontal strip, sized from its visible window count
instead of a percentage of the display. Its panel and the compact icon dock
share native borders and shadows; Lua owns their panel geometry. The root
`ground` rectangle is the shell panel contract and must be unparented. Captured
external surfaces and raster windows reuse their existing textures as live
previews; other destinations use semantic vector symbols. These are not OS app
brand icons. There are no additional capture sessions or media encoders.

Install the updated native host **before** deploying these Lua sources. Once
installed, their layout and styling can be hot-reloaded without recompilation.

With Canvas focused, **Alt-Tab** (Option-Tab on Neo) previews attached windows
in most-recently-focused order. Shift reverses the cycle; releasing Alt commits
focus (Enter also commits), restores a minimized destination, changes its page if necessary, and
raises it through the same path as a dock click. Escape or loss of Canvas focus
cancels without changing selection. Frameless desktop instruments, including
the analog clock, are excluded. The order stays frozen during each chord;
a destination closed during preview is not replaced with another window.

The overlay reuses bounded Lua launcher presentation, not OS-wide discovery.
It does not intercept macOS Command-Tab or provide a global shortcut. Existing
HID destinations release held keys before switching; native captures remain
read-only until their separate input capability is installed and verified.

## Native attachment boundary

The application catalog discovers launchable apps. Launching one delegates to
macOS; it does not yet attach its windows to Canvas. These are separate operations.

The local external-surface contract already supports `IoSurface`, and the Metal
renderer has an importer in `canvas-model-viewer/src/external_surface.rs`.
`canvas-window-stream` produces H.264 for remote transport. The separate
`canvas-native-window` producer now implements direct owner-local capture; see
[the local window runbook](macos-native-window.md). It is not yet integrated with
the launcher. Live capture and GPU import must be verified before claiming a
successful attachment.

The direct local attachment belongs in the capture adapter: select an explicit
window through ScreenCaptureKit, register a volatile external-surface generation,
and let Canvas tile/composite its stable source ID. IOSurface handles never travel
over Unibus or enter persisted workspace documents. Remote nodes retain the media
transport. Frame ownership/release and bounded in-flight work must be verified
before importing recyclable capture buffers; registration alone is not a GPU
completion acknowledgement. Native keyboard/mouse actions still belong to the
permission-bearing platform helper, not Lua or the pixel transport.

`config/deploy/neo/simulator-local-stream.json` is a local H.264 capture profile
for testing this separately from the TV stream. It sends to loopback only; it is
not auto-enabled and still requires capture permission and a healthy Unibus
registration. Do not present an empty surface as a successful live attachment.

Neo runs the separately signed `~/Applications/Canvas.app` bundle using
`config/deploy/neo/Canvas.Info.plist`. Its existing `dev.fpl.canvas` LaunchAgent
points to `Contents/MacOS/canvas`; environment and the permission-bearing Dock
Control helper stay unchanged. The previous `.local/bin/canvas` remains available
for rollback. Restore the backed-up LaunchAgent executable path and reload that
agent to roll back; do not replace the input helper to update the GUI.

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
