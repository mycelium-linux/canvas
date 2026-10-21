# Dock

A project workspace shared by a person and an agent across Agora One and the Mac.
Rust supplies the runtime; iced supplies the GUI; Coherence supplies project visual
identity. [The current first-cut design](docs/first-cut.md) consolidates the user
requirements and supersedes the initial Python/profile-first experiment.

Edgerunner and the user's existing OpenCode setup own coding sessions and model
selection. The local Agora sessions demonstrate integration; they are not the
long-term session owner. [Existing-owner discovery and attachment status](docs/edgerunner-attachment.md)
records the verified connection and the remaining interactive attachment work.

Alfalfa supplies the project context and shared resources. See
[project startup and authentication boundaries](docs/project-startup.md) for the
live context adapter and the remaining seamless-startup integration.

## Build and run

The default iced interface is a project switcher. Ctrl+Option+Space
summons it; choose a project, then choose its notes or dashboards,
and press Enter to open. Included notes preview as Markdown; web dashboards and
project homes open in the existing browser. Resources come from Alfalfa, and
opening this launcher does not start an agent or Linux desktop. See
[notes and dashboards](docs/project-resources.md).

The earlier conversation/desktop companion remains available with
`--conversation` or `--connect-project PROJECT`. In that mode, Enter submits a
prompt and Escape interrupts. See
[the project workflow](docs/workspace-demo.md) and
[OpenCode sessions](docs/opencode-sessions.md).

The agent fleet itself is observable without a terminal: `scripts/cp-server.lua`
exposes one tmux session as JSON with per-pane send, capture, and kill, and
Canvas draws a widget per pane over it. See
[the control plane](docs/CONTROL-PLANE.md).

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p dock-mac
```

Install the permission-bearing Mac helper separately, once:

```sh
scripts/install-mac-control.sh
```

Enable **Dock Control** in macOS **Privacy & Security → Accessibility**. The
installed `~/Applications/Dock Control.app` stays unchanged during GUI builds.
The installer preserves an existing helper; upgrading it requires `--upgrade`.
See [helper identity and signing](docs/mac-control.md) for the exact boundary.

The explicit `--diagnostics` view can attach existing Mac windows to a project.
Recall, Tile, project selection, CLI activation, and MCP recall share one layout
operation for those bindings. A custom work area is available as an optional override. Switching projects changes
its Coherence appearance and preserves background work. Mac window control uses
public Accessibility through the independently installed Rust helper. Permission
failures are surfaced rather than simulated.
See [Mac first-cut usage](docs/mac-first-cut.md) for exact controls and limitations.

`config/projects.json` defines initial project/resource bindings and theme choices.
Pass `dock-mac --catalog /path/to/projects.json` to load another catalog. Bundled
Dock and Studio entries are starting configurations, not discovered running apps.
They use distinct upstream Coherence palettes. Agora One has an independent
headless Sway desktop with matching generated terminal and compositor themes.
Unrelated applications and the surrounding GNOME session are not restyled.

## Tiling desktop and a dedicated stream display

[Agora tiling setup](docs/agora-tiling.md) records the installed compositor,
project workspace commands and the current streaming/input boundary.
The Sunshine endpoint remains the physical GNOME desktop. The independent
project desktop uses wayvnc and TigerVNC through a private SSH tunnel, giving
capture and virtual input the same Wayland seat even when GNOME is locked.
The earlier nested Hyprland setup remains available for compositor experiments.

On Mac, select a streaming display and destination display(s), review the local
windows affected, then start the managed stream. Dock saves their frames before
moving them and retains a restoration journal for failure/crash recovery.
See [display reservation](docs/mac-stream-display.md). A second display is
required for this evacuation flow; ordinary windowed streaming remains available.

Generate Linux themes from the same Rust/Coherence source used by iced:

```sh
cargo run -p dock-core --bin dockctl -- export-desktop config/linux
```

This writes configuration artifacts; it does not change the running compositor.
The installed Linux adapter applies the selected project's theme and workspace
through Hyprland's existing IPC.

## Agent-facing tools

```sh
cargo run -p dock-core --bin dockctl -- projects
cargo run -p dock-core --bin dockctl -- status
cargo run -p dock-core --bin dockctl -- theme dock
```

The CLI returns versioned JSON envelopes and a nonzero exit on failure. Its default
actor is `agent`. The human can grant local workspace control through the GUI or
explicitly through the CLI:

```sh
cargo run -p dock-core --bin dockctl -- --actor human release-control dock
cargo run -p dock-core --bin dockctl -- --actor human release-control studio
cargo run -p dock-core --bin dockctl -- select studio
cargo run -p dock-core --bin dockctl -- --actor human take-control studio
```

These commands change local workspace policy/selection only. GUI and CLI share
`.dock/state.json` when launched from this directory. Set `DOCK_STATE` to the same
absolute path when invoking them from different directories; `dockctl --state`
can also override it. Writes use a cooperative lock and atomic rename. A crash
lock fails visibly; inspect its recorded PID before removing a stale lock.

The actor flag is cooperative intent, not authentication. OS account permissions
are the boundary. Taking local control prevents cooperating agent window actions;
it does not interrupt an external coding agent or revoke SSH access.

## Agora One and streaming

Agora One is the Ubuntu GPU desktop. Its existing GNOME Wayland session remains
in place. `dock-linux` is installed at `~/.local/bin/dock-linux` on that host.
It exposes JSON capabilities plus launch/status/logs and cgroup freeze/thaw for
processes it owns. [Agora runbook](docs/agora-first-cut.md) records setup, live
checks, streaming configuration and remaining constraints.

```sh
ssh -o BatchMode=yes agora-one '~/.local/bin/dock-linux capabilities'
```

Sunshine is installed on Agora One; Moonlight is installed and paired on the Mac.
The initial stream uses Tailscale and Sunshine's required encryption. Start a
single windowed desktop stream with:

```sh
/Applications/Moonlight.app/Contents/MacOS/Moonlight stream 100.80.85.86 Desktop \
  --1080 --fps 60 --bitrate 10000 --display-mode windowed \
  --absolute-mouse --capture-system-keys never
```

A paired host does not guarantee capture readiness: a sleeping Linux display can
prevent encoder initialization. Prepare a bounded wake window when needed:

```sh
ssh -o BatchMode=yes agora-one .local/bin/dock-linux prepare-stream 600
```

Encrypted 1080p60 video delivery/decoding and audio transport were verified from
Agora One to this Mac. The Linux session remained locked. Normal input, sustained
performance and reconnect after idle still need validation. The wake inhibitor
expires automatically; it is not yet tied to stream lifetime.

Pause/resume applies only to Dock-managed Linux process groups. It does not
suspend a desktop, freeze arbitrary terminals, or preserve state across machine
reboots. OpenCode now has persistent per-project sessions, prompting, progress,
abort and continuation through its supported API. Edge Runner integration remains
separate; the existing session owner supplies inference and conversation persistence.

## Design provenance and tracking

`vendor/coherence-tokens` is the existing Coherence native crate copied unchanged
from the sibling checkout, with SHA-256 provenance. Refresh from its owner rather
than hand-editing generated colors or dimensions.

`dock-core` owns identity/control policy; `dock-mac` owns presentation and Mac
window actions; `dock-linux` owns supported Linux operations. Streaming, compute
and agency remain separate per the [mesh design](docs/computing-mesh-design.md).

Beads epic `dock-96j` tracks the first cuts and followups; `dock-2kf` tracks future
architectural candidates. `fpl` supplies the managed engineering baseline.
The old Python package, TOML profiles and generated-token prototype remain as
exploratory history and are not required to build or run the Rust application.
