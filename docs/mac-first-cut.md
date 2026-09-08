# Mac project shell first cut

The Rust `canvas-mac` application uses iced 0.14 and the shared Coherence token
adapter in `canvas-core`. Project identity changes the whole palette and its quiet
edge accent; the project name stays in a compact selector. Nothing restores the
old dotfiles or replaces ordinary macOS window management.

Run from the repository root:

```sh
cargo run -p canvas-mac
# Or supply a different versioned project catalog:
cargo run -p canvas-mac -- --catalog config/projects.json
```

Select a project, refresh windows, and attach existing windows to it. Selecting
that project again, Recall, and Tile all activate the same project workspace:
Canvas places its browser in the primary region, stacks its supporting windows in
the remaining region, and focuses the primary window. Bounds come from AppKit at
the moment of use and exclude the menu bar and macOS Canvas. “Custom work area”
exposes an optional x, y, width, height override in screen points. Other windows
remain available and no application is terminated when switching projects.
If an application clamps its size, Canvas accepts the acknowledged frame only when
it remains fully inside that window's assigned layout region and returns the
actual geometry in the receipt. Placement outside the region still fails visibly.

macOS requires Accessibility permission for the separately installed Dock Control.app
helper. The GUI and CLI communicate with it through a same-user Unix socket. The OS bridge reports denial, ambiguity, timeouts, and application size
constraints. Window attachments use opaque helper-owned native window identities
plus an app/PID/title fingerprint. After a helper restart, Canvas refreshes the ID
only when the fingerprint resolves to one current window; ambiguity fails visibly.
Attachments persist with owner-only permissions in
`~/Library/Application Support/Dock/window-bindings.json`.

The GUI and `canvas-core-ctl` share `.dock/state.json` (`DOCK_STATE` overrides its path).
The GUI polls shared state every second. Human control is the default. “Allow
agent control” enables cooperative agent operations; “Take control” revokes that
policy. The project conversation panel also sends takeover to the Linux backend and
interrupts its managed OpenCode request. This cooperative policy is not
authentication against other processes running as the same OS user. Keyboard and mouse remain available.

Agent callers can enumerate windows and operate on an attached project window:

```sh
cargo run -p canvas-mac -- windows
cargo run -p canvas-mac -- workspace list
cargo run -p canvas-mac -- workspace bind dock WINDOW_ID
cargo run -p canvas-mac -- workspace activate dock --human
cargo run -p canvas-mac -- workspace unbind dock WINDOW_ID
cargo run -p canvas-mac -- focus dock 12345 'Exact window title'
cargo run -p canvas-mac -- tile dock 12345 'Exact window title' '0,40,900,700'
```

The workspace commands validate the exact project through Alfalfa. Binding uses
an opaque ID from the current tolerant inventory; activation moves only the saved
windows and persists any refreshed IDs. `--human` makes the caller's intent
explicit. The older focus/tile primitives require an attached window and agent control for that project.
A human CLI caller may add `--human`. These CLI actions currently use the bundled
catalog. The library exposes the narrow OS primitives for other trusted adapters;
its primitive calls are not an authorization boundary. Processes under the same
OS account can bypass this cooperative policy.

The resource list opens local applications by name. “Reserve display” opens the
reviewed streaming-display flow described in [mac-stream-display.md](mac-stream-display.md). The video is a remote
desktop, not a list of individually accessible Mac windows. Configured Agora One
managed sessions expose Inspect, Pause process group, and Resume process group
through BatchMode SSH to the installed `canvas-linux` CLI. The response is validated against the protocol version and the requested
process freezer state. A resource pause does not mark the whole project paused: a
project can contain other running sessions and local applications. Switching projects never pauses
a process automatically. The project conversation panel now integrates the managed OpenCode server on Agora One;
see [project-session-workflow.md](project-session-workflow.md). Edge Runner itself
is still a separate future provider.

## Validation

`cargo test -p canvas-mac` exercises tiling bounds and negative display origins.
`cargo build -p canvas-mac` verifies the real iced native application. Own-renderer
visual QA is available without capturing unrelated desktop content:

```sh
DOCK_SCREENSHOT=/tmp/dock.ppm cargo run -p canvas-mac
sips -s format png /tmp/dock.ppm --out /tmp/dock.png
```

This captures the actual iced window once after its first polling tick. The
application remains running. Quit it normally after inspection.

iced's application and dynamic-theme APIs are documented at
https://docs.rs/iced/0.14.0/iced/ and
https://docs.rs/iced/0.14.0/iced/application/index.html.
