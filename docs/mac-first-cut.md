# Mac project shell first cut

The Rust `dock-mac` application uses iced 0.14 and the shared Coherence token
adapter in `dock-core`. Project identity changes the whole palette and its quiet
edge accent; the project name stays in a compact selector. Nothing restores the
old dotfiles or replaces ordinary macOS window management.

Run from the repository root:

```sh
cargo run -p dock-mac
# Or supply a different versioned project catalog:
cargo run -p dock-mac -- --catalog config/projects.json
```

Select a project, refresh windows, and attach existing windows to it. Recall
raises only those attached windows. Tiling divides the primary display’s usable area into columns, deriving its
bounds from AppKit at the moment of use and excluding the menu bar and macOS
Dock. “Custom work area” exposes an optional x, y, width, height override in screen
points for another display or a smaller region. Other windows remain
available and no application is terminated when switching projects.

macOS requires Accessibility permission for the separately installed Dock Control.app
helper. The GUI and CLI communicate with it through a same-user Unix socket. The OS bridge reports denial, ambiguity, timeouts, and application size
constraints. Window attachments use opaque helper-owned native window identities. A restarted
helper invalidates those identities and requires refreshed attachments; old
PID/title-only attachments fail visibly. Attachments persist in
`~/Library/Application Support/Dock/window-bindings.json`.

The GUI and `dockctl` share `.dock/state.json` (`DOCK_STATE` overrides its path).
The GUI polls shared state every second. Human control is the default. “Allow
agent control” enables cooperative agent operations; “Take control” revokes that
policy. The project conversation panel also sends takeover to the Linux backend and
interrupts its managed OpenCode request. This cooperative policy is not
authentication against other processes running as the same OS user. Keyboard and mouse remain available.

Agent callers can enumerate windows and operate on an attached project window:

```sh
cargo run -p dock-mac -- windows
cargo run -p dock-mac -- focus dock 12345 'Exact window title'
cargo run -p dock-mac -- tile dock 12345 'Exact window title' '0,40,900,700'
```

Focus/tile require an attached window and agent control for that project.
A human CLI caller may add `--human`. These CLI actions currently use the bundled
catalog. The library exposes the narrow OS primitives for other trusted adapters;
its primitive calls are not an authorization boundary. Processes under the same
OS account can bypass this cooperative policy.

The resource list opens local applications by name. “Reserve display” opens the
reviewed streaming-display flow described in [mac-stream-display.md](mac-stream-display.md). The video is a remote
desktop, not a list of individually accessible Mac windows. Configured Agora One
managed sessions expose Inspect, Pause process group, and Resume process group
through BatchMode SSH to the installed `dock-linux` CLI. The response is validated against the protocol version and the requested
process freezer state. A resource pause does not mark the whole project paused: a
project can contain other running sessions and local applications. Switching projects never pauses
a process automatically. The project conversation panel now integrates the managed OpenCode server on Agora One;
see [project-session-workflow.md](project-session-workflow.md). Edge Runner itself
is still a separate future provider.

## Validation

`cargo test -p dock-mac` exercises tiling bounds and negative display origins.
`cargo build -p dock-mac` verifies the real iced native application. Own-renderer
visual QA is available without capturing unrelated desktop content:

```sh
DOCK_SCREENSHOT=/tmp/dock.ppm cargo run -p dock-mac
sips -s format png /tmp/dock.ppm --out /tmp/dock.png
```

This captures the actual iced window once after its first polling tick. The
application remains running. Quit it normally after inspection.

iced's application and dynamic-theme APIs are documented at
https://docs.rs/iced/0.14.0/iced/ and
https://docs.rs/iced/0.14.0/iced/application/index.html.
