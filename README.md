# Canvas

The shared visualization plane between a person, their agents, and the systems
that own the data. An image, a PDF, a terminal, a camera, a market feed, Elmers'
live graph events, and an agent-generated explanation differ in source contract
and update cadence; they do not need separate window systems. Canvas presents all
of them through one revisioned document and one lifecycle.

The unit of sharing is a **view**, not a screenshot. A view names authorized
sources and records the exact visual context applied to them — source revision or
stream sequence, viewport, attention region, page or playhead, filters. Human
interaction and agent tools observe and mutate the same state, so an agent that
zooms into part of an image before inspecting it shows you that zoom, and a page
you turn is the page its next inspection sees.
[The shared visualization plane](docs/canvas-shared-visualization.md) states the
source, streaming, Lua and agent-capture boundaries in full.

Rust supplies the runtime; iced supplies the GUI; Coherence supplies project
visual identity; Alfalfa supplies project and resource identity. Edgerunner and
the existing OpenCode setup own coding sessions and model selection — Canvas
presents them, it does not own them.

## Crates

| Crate | Owns |
| --- | --- |
| `canvas` | The renderer-independent workspace document, its patch operations, the control socket, and the native presentation host. |
| `canvas-core` | Project identity, control policy, and the Coherence-derived theme. No desktop, transport or inference ownership. |
| `canvas-widgets` | Reusable presentation primitives as a durable `WidgetSpec`, renderable live in Canvas or statically through matplotlib/doc-forge/mmdc. |
| `canvas-mac` | Mac presentation and window actions. |
| `canvas-linux` | Supported Linux operations on Agora One. |
| `canvas-control` | The permission-bearing Mac helper; the only thing that touches Accessibility. |

Binaries: `canvas` (presentation host), `canvasctl` (control socket client),
`canvas-mcp` (agent adapter, `--features mcp`), `canvas-core-ctl` (identity,
policy and theme export), `canvas-mac`, `canvas-linux`.

## Build and run

The agent fleet itself is observable without a terminal: `scripts/cp-server.lua`
exposes one tmux session as JSON with per-pane send, capture, and kill, and
Canvas draws a widget per pane over it. See
[the control plane](docs/CONTROL-PLANE.md).
```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p canvas-mac
```

The default iced interface is a project switcher. Ctrl+Option+Space summons it;
choose a project, then its notes or dashboards, and press Enter to open. Included
notes preview as Markdown; web dashboards and project homes open in the existing
browser. Resources come from Alfalfa, and opening the launcher does not start an
agent or a Linux desktop. See [notes and dashboards](docs/project-resources.md).

The earlier conversation/desktop companion remains available with
`--conversation` or `--connect-project PROJECT`; there, Enter submits a prompt and
Escape interrupts. See [the project workflow](docs/workspace-demo.md) and
[OpenCode sessions](docs/opencode-sessions.md).

`config/projects.json` defines initial project/resource bindings and theme
choices. `canvas-mac --catalog /path/to/projects.json` loads another catalog. The
bundled entries are starting configurations, not discovered running apps.

## The Mac helper

Install the permission-bearing helper separately, once:

```sh
scripts/install-mac-control.sh
```

Enable **Dock Control** in macOS **Privacy & Security → Accessibility**. The
installed `~/Applications/Dock Control.app` stays unchanged during GUI builds;
the installer preserves an existing helper, and upgrading requires `--upgrade`.
See [helper identity and signing](docs/mac-control.md) for the exact boundary.

The helper keeps its old name deliberately. Accessibility grants are keyed to
`com.futurepresentlabs.dock.control` and its signature, so renaming the bundle
would silently revoke a permission the human granted by hand. It is renamed when
someone is at the machine to re-grant it, not as a side effect of a refactor.

Mac window control uses public Accessibility through that helper. Permission
failures are surfaced rather than simulated. `--diagnostics` attaches existing Mac
windows to a project; Recall, Tile, project selection, CLI activation and MCP
recall share one layout operation. Switching projects changes the Coherence
appearance and preserves background work. See
[Mac first-cut usage](docs/mac-first-cut.md) for exact controls and limits.

## Tiling desktop and a dedicated stream display

[Agora tiling setup](docs/agora-tiling.md) records the installed compositor,
project workspace commands, and the current streaming/input boundary. The
Sunshine endpoint remains the physical GNOME desktop; the independent project
desktop uses wayvnc and TigerVNC through a private SSH tunnel, giving capture and
virtual input the same Wayland seat even when GNOME is locked. The earlier nested
Hyprland setup remains available for compositor experiments.

On Mac, select a streaming display and destination display(s), review the local
windows affected, then start the managed stream. Canvas saves their frames before
moving them and retains a restoration journal for failure and crash recovery. See
[display reservation](docs/mac-stream-display.md). A second display is required
for this evacuation flow; ordinary windowed streaming remains available.

Generate Linux themes from the same Rust/Coherence source iced uses:

```sh
cargo run -p canvas-core --bin canvas-core-ctl -- export-desktop config/linux
```

This writes configuration artifacts; it does not change the running compositor.
The installed Linux adapter applies the selected project's theme and workspace
through Hyprland's existing IPC.

## Agora One and streaming

Agora One is the Ubuntu GPU desktop. Its existing GNOME Wayland session remains in
place. `canvas-linux` is installed at `~/.local/bin/canvas-linux` there and
exposes JSON capabilities plus launch/status/logs and cgroup freeze/thaw for
processes it owns. The [Agora runbook](docs/agora-first-cut.md) records setup,
live checks, streaming configuration and remaining constraints.

```sh
ssh -o BatchMode=yes agora-one '~/.local/bin/canvas-linux capabilities'
```

Sunshine is installed on Agora One; Moonlight is installed and paired on the Mac.
The initial stream uses Tailscale and Sunshine's required encryption:

```sh
/Applications/Moonlight.app/Contents/MacOS/Moonlight stream 100.80.85.86 Desktop \
  --1080 --fps 60 --bitrate 10000 --display-mode windowed \
  --absolute-mouse --capture-system-keys never
```

A paired host does not guarantee capture readiness: a sleeping Linux display can
prevent encoder initialization. Prepare a bounded wake window when needed:

```sh
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux prepare-stream 600
```

Encrypted 1080p60 video delivery/decoding and audio transport were verified from
Agora One to this Mac with the Linux session locked. Normal input, sustained
performance and reconnect after idle still need validation. The wake inhibitor
expires on its own; it is not yet tied to stream lifetime.

Pause/resume applies only to Canvas-managed Linux process groups. It does not
suspend a desktop, freeze arbitrary terminals, or preserve state across reboots.

## Agent-facing tools

Identity, policy and theme:

```sh
cargo run -p canvas-core --bin canvas-core-ctl -- projects
cargo run -p canvas-core --bin canvas-core-ctl -- status
cargo run -p canvas-core --bin canvas-core-ctl -- theme dock
```

The CLI returns versioned JSON envelopes and a nonzero exit on failure. Its
default actor is `agent`. The human can take local workspace control through the
GUI or explicitly:

```sh
cargo run -p canvas-core --bin canvas-core-ctl -- --actor human release-control dock
cargo run -p canvas-core --bin canvas-core-ctl -- select studio
cargo run -p canvas-core --bin canvas-core-ctl -- --actor human take-control studio
```

These change local workspace policy and selection only. GUI and CLI share
`.dock/state.json` when launched from this directory; set `DOCK_STATE` (or
`canvas-core-ctl --state`) to the same absolute path when invoking them from
elsewhere. Writes use a cooperative lock and atomic rename, and a crash lock fails
visibly — inspect its recorded PID before removing a stale one.

The actor flag is cooperative intent, not authentication. OS account permissions
are the boundary. Taking local control prevents cooperating agent window actions;
it does not interrupt an external coding agent or revoke SSH access.

Presentation itself goes over the owner-local control socket, directly:

```sh
echo inspect | cargo run -p canvas --bin canvasctl
```

or through MCP, which is an adapter over that same socket rather than a second
renderer — `canvas_inspect`, `canvas_apply_patch`, `canvas_open_widget`,
`canvas_view_patch`, `canvas_screenshot` and the rest are listed in
[Canvas MCP](docs/canvas-mcp.md). A capture identifies the view revision and
source positions it rendered, so an agent cannot silently reason about pixels
different from the ones shown to the person.

## Lua modules

A module is a manifest plus Luau source that turns typed host data into a small
retained scene. It runs once per `load`, per data or interaction event, and per
candidate hot reload; Canvas keeps the last successful scene between calls and
never runs Lua in the render loop. The manifest lists readable input IDs, whether
theme tokens are visible, and which host resources may be presented — there is no
filesystem, network, process, clock, GPU or workspace-mutation capability.

Because a fresh VM runs each invocation, a module is stateless: anything that
accumulates across frames lives host-side. `module.theme` is derived at invocation
time from `canvas_core::ProjectTheme`, so a module receives semantic colours and
metrics without owning or duplicating a palette. Scene text nodes currently take
id, parent, text, translation, rotation and scale; letting a module name a
semantic role for the host to resolve against the project palette is in flight on
`feat/scene-tint` and not on this branch.

See [Canvas Lua modules](docs/canvas-lua-modules.md) and the executable
[`graph.lua`](crates/canvas/examples/lua/graph.lua),
[`pnl-status.lua`](crates/canvas/examples/lua/pnl-status.lua) and
[`activity-graph.lua`](crates/canvas/examples/lua/activity-graph.lua) examples.
The last of those draws any producer of the
[activity stream contract](../shared-infra/docs/activity-stream.md) — Elmers'
workflows and capabilities, the MCP bridge's calling systems, connectors and
tools — from one typed graph input rather than one view per source.

## Naming

The project is Canvas. The repository directory and its GitHub remote are still
`dock`, and the MCP server registrations are still `fpl-local-dock-canvas` and
`fpl-local-dock-workspaces`; those are renamed deliberately and separately,
because each one breaks something already installed on a machine. The same goes
for `.dock/state.json`, `DOCK_STATE`, and the Mac helper bundle above. Crate,
binary and module names inside this repository are Canvas already.

## Design provenance and tracking

`vendor/coherence-tokens` is the Coherence native crate copied unchanged from the
sibling checkout, with SHA-256 provenance. Refresh from its owner rather than
hand-editing generated colors or dimensions. Theme colors come from `canvas-core`
and those tokens, so Canvas never creates a second palette.

`canvas-core` owns identity and control policy; `canvas-mac` owns presentation and
Mac window actions; `canvas-linux` owns supported Linux operations. Streaming,
compute and agency remain separate per the
[mesh design](docs/computing-mesh-design.md).

Beads epic `dock-96j` tracks the first cuts and followups; `dock-2kf` tracks
future architectural candidates. `fpl` supplies the managed engineering baseline.
The old Python package, TOML profiles and generated-token prototype remain as
exploratory history and are not required to build or run the Rust application.

## Documents

- [First-cut design](docs/first-cut.md) — consolidated user requirements.
- [Shared visualization plane](docs/canvas-shared-visualization.md) — sources, views, boundaries.
- [Data model](docs/canvas-data-model.md) — what the document durably owns.
- [Lua modules](docs/canvas-lua-modules.md) — sandbox, lifecycle, inputs.
- [Canvas MCP](docs/canvas-mcp.md) — the agent adapter.
- [Project startup](docs/project-startup.md) and [project resources](docs/project-resources.md) — the Alfalfa boundary.
- [Mac control](docs/mac-control.md), [Mac first cut](docs/mac-first-cut.md), [stream display](docs/mac-stream-display.md).
- [Agora first cut](docs/agora-first-cut.md), [tiling](docs/agora-tiling.md), [desktop](docs/agora-desktop.md).
- [Computing mesh design](docs/computing-mesh-design.md) — why streaming, compute and agency stay separate.
