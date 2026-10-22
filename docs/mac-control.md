# Stable Mac window control

`Dock Control.app` is a small, independently installed Rust app that calls public
AppKit and Accessibility APIs. The iced shell uses the Rust client library for its
versioned Unix socket. Rebuilding either client leaves the installed app and its
Accessibility identity untouched. No AppleScript or System Events process
performs window control.

Install once from the repository:

```sh
scripts/install-mac-control.sh
```

The fixed installation is `~/Applications/Dock Control.app`, bundle identifier
`com.futurepresentlabs.dock.control`. The installer launches it through
LaunchServices, independently of the terminal and iced application. Grant **Canvas
Control** access in System Settings → Privacy & Security → Accessibility. macOS
requires the user's grant; the helper can request the standard prompt but cannot
grant access. An already running client can retry after access is granted.

Normal `cargo build`, `cargo run -p canvas-mac`, and repeated installer calls **do
not replace the installed helper**. The source-built binary is not the production
permission holder. To launch the existing app after logging in:

```sh
open -g "$HOME/Applications/Dock Control.app"
```

For a deliberate helper upgrade, first restore any reserved streaming display,
quit Dock Control in Activity Monitor, then run:

```sh
scripts/install-mac-control.sh --upgrade
```

The installer preserves the previous bundle as a dated backup. Initial installs
use ad-hoc signing by default. A fixed path and identifier preserve the installed
identity during client development, but **ad-hoc signing cannot guarantee that
macOS will retain Accessibility permission when the helper itself is replaced**.
Use a consistent Developer ID signing identity via
`DOCK_CONTROL_SIGN_IDENTITY='Developer ID Application: …'` for upgrades; macOS
still controls permission decisions. There is no TCC reset or bypass.

## Control boundary

The socket is `~/Library/Application Support/Dock/control/control.sock` (0600),
inside a directory owned by the current account (0700). Both peers check the
other endpoint's UID; the server uses a lock to prevent duplicate instances and
only removes stale sockets after acquiring it. A request is one newline-delimited
JSON object, maximum 1 MiB, with a version and a typed operation. Reads and writes
have deadlines. The helper accepts status, permission prompt, inventory,
focus, frame placement, and fullscreen operations; there is no command execution
operation. Clients and agents running under the same macOS account can use this
interface. This is an OS-account boundary, not isolation between same-user apps
or an agent authorization policy; orchestration policy remains in Canvas.

`canvas-control` exposes the Rust protocol and `request(Operation)` client. Frames
are integer logical desktop points, with origin at the primary display's top
left; displays to its left or above can have negative coordinates. Display IDs
use CoreGraphics display UUIDs. Frame acknowledgements contain the actual window
state so callers can detect clamping and refused placement. The helper polls native fullscreen state for up to three seconds; callers also
verify the resulting display placement before claiming success.

Window IDs refer to retained Accessibility objects and include a server-instance
nonce. The registry also verifies each process launch timestamp, rejecting PID reuse.
IDs expire when a window disappears or the helper restarts. Project bindings retain
the app, PID, and title as a resolver fingerprint. After a helper restart, the
helper reattaches only when that fingerprint identifies one current window; title
changes are accepted only when the app exposes a single candidate. Ambiguity and
duplicate resolution fail visibly.

Inventory covers regular applications exposing AX windows. The project picker
uses tolerant inventory and reports skipped, unresponsive apps as warnings. The
display-reservation snapshot stays strict: AX window-list errors, invalid geometry,
and state-read failures reject the complete snapshot, so a client never reserves
a display from partial inventory.
Applications without accessibility support and closed windows cannot be controlled. macOS Spaces and
application sizing constraints remain authoritative.

The native implementation uses Apple's
[Accessibility trust API](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions)
and
[window attribute API](https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue).

## Verified permission recovery

The installed helper now reports `trusted: true`. macOS logs showed that a stale
code requirement still referenced an older ad-hoc build despite the visible toggle.
A targeted `tccutil reset Accessibility com.futurepresentlabs.dock.control`,
followed by restarting the unchanged helper and the user's approval, resolved it.
This is a diagnosed recovery action, not part of routine startup or GUI builds.

The authorized installed binary then exposed a separate inventory bug: Finder's
optional AppKit `launchDate` is absent. The helper uses the kernel process start
time, retaining PID-reuse checks.
