# Independent project desktop on Agora One

The working project desktop is **headless Sway with wayvnc over SSH**, with a
real OpenCode terminal and private Firefox profile in each project workspace.
This runs independently of GNOME's physical screen, lock, and idle state.
Hyprland remains an exploratory nested desktop. Sunshine/Moonlight still
provides the original GNOME desktop; neither was replaced.

Sway's headless backend provides two 1600×1000 outputs using the software renderer.
One WayVNC service captures each output and creates Wayland virtual input devices on the
same compositor seat. This avoids the earlier nested Hyprland problem: GNOME
could suspend its window's rendering, and Sunshine's kernel input targeted the
physical seat instead of the nested desktop. This first independent transport
uses RFB rather than GPU video encoding; performance tuning is separate work.

## Setup and recovery

Generate the shared Coherence artifacts at the repository root, then run the
Linux setup script on Agora One:

```sh
cargo run -p dock-core --bin dockctl -- export-desktop config/linux
sh scripts/setup-agora-desktop.sh
```

The script installs established Ubuntu desktop packages, a native Firefox
runtime with separate project profiles, generated theme files, and the Rust
adapter. It creates authentication credentials at runtime and preserves them
when rerun. It never writes credentials into the repository.

The Rust `desktop-start` command starts or registers two transient user units:

| Unit | Responsibility |
| --- | --- |
| `dock-desktop.service` | Sway in a private D-Bus session and runtime directory |
| `dock-wayvnc.service` | Authenticated remote viewing/input on `127.0.0.1:5901` |
| `dock-wayvnc-headless-2.service` | `HEADLESS-2` on `127.0.0.1:5902` |

Runtime sockets live in `/run/user/1000/dock-desktop`; the generated local
registry is `~/.local/state/dock/desktop.json`. GNOME's global activation
environment remains unchanged. Start after reboot with:

```sh
ssh -o BatchMode=yes agora-one .local/bin/dock-linux desktop-start
ssh -o BatchMode=yes agora-one 'cd ~/src/dock && sh scripts/setup-agora-multi-monitor-stream.sh'
```

Re-running this against a live desktop preserves its windows. Stopping Sway
closes its windows; save browser/terminal work first. OpenCode's server and tmux
session live separately and can be reattached. To restart only the transport,
stop `dock-wayvnc`, then run `desktop-start`. Inspect failures with
`journalctl --user -u dock-desktop -u dock-wayvnc`.

## Secure Mac transport

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 15901:127.0.0.1:5901 -L 15902:127.0.0.1:5902 agora-one
```

TigerVNC connects to `127.0.0.1::15901` using `X509Plain`. Copy the server
certificate over trusted SSH from `~/.config/dock/wayvnc/tls-cert.pem` and pass
it as the viewer's X509 CA. Account `dock` uses the generated password in
`~/.config/dock/wayvnc/password`, mode 0600. The Mac launcher reads that
credential privately; it is not passed in the visible command line. Wayvnc's
listener is loopback-only. SSH protects the network path, and the viewer also
validates the server certificate. Native Apple authentication compatibility is
enabled in wayvnc; the validated client is TigerVNC with X509Plain.

The iced app can recall projects and show real screenshots; the separate viewer
provides the continuous desktop and normal keyboard/mouse interaction.

## Project and agent control

```sh
dock-linux desktop-recall dock
dock-linux session-ensure dock /home/ajmwagar/src/dock
dock-linux session-attach dock
dock-linux desktop-browser dock
dock-linux desktop-status dock
dock-linux desktop-capture-file dock
```

`desktop-recall` applies the generated Coherence theme, sets a tabbed workspace,
and verifies both the active workspace and resulting layout. Each browser or
terminal receives the full workspace instead of being compressed into a narrow
horizontal slice. Use `Alt+h` and `Alt+l` to move between tabs. Existing windows
remain alive when switching projects. Dock uses
the dark colorway with a cyan accent; Studio uses the light colorway with blue.
Terminal palettes stay with their project. The same commands drive human
project recall and the iced app. Browser and terminal launch wait for the
actual assigned window to map, rather than reporting a queued launch as ready.

Agent input requires an explicit backend grant:

```sh
dock-linux desktop-control dock agent
dock-linux desktop-focus dock WINDOW_ID
dock-linux desktop-click dock 965 500 left
dock-linux desktop-key dock CTRL+l
dock-linux desktop-text dock 'example text'
dock-linux desktop-capture-file dock
dock-linux desktop-control dock human
```

`desktop-status` provides live window IDs and geometry. The coordinates above
are examples from the validated fixture; use a fresh screenshot before acting.
`desktop-capture` returns real PNG bytes as base64 plus dimensions; the `-file`
variant returns the private local PNG path for an agent's image-reading tool.
Captures are capped at 16 MiB and are stored in a private directory.

Pointer input uses the actual wlroots virtual-pointer protocol from Rust with
motion/button frames, server acknowledgement, and a three-second transaction
deadline. Keyboard/text use `wtype` on this same private Wayland display.
The desktop uses click-to-focus to avoid incidental cursor motion switching the
typing target. Input is sent to the focused window; focus it explicitly first.

The backend ownership default is human. A shared file lock orders agent input,
project navigation, prompt submission, and takeover. Input verifies the active
workspace matches its requested project. Human takeover persists revocation
and aborts OpenCode under that lock; subsequent automated input/prompt requests
fail. Automatic clients read `ownership.epoch` from `desktop-status` and grant
with `desktop-control PROJECT agent EXPECTED_EPOCH`. Every human revocation
increments the epoch, so a delayed automatic grant cannot undo a newer takeover.
The three-argument grant remains an explicit manual operation.
This is cooperative control among Dock clients, not an OS sandbox against
arbitrary programs running as the same Unix user. Normal VNC human input remains
available. Project switching does not suspend its processes; process freeze and
coding-session abort are separate operations.

## Observed results

* Independent full 1600×1000 captures and authenticated Mac VNC frames.
* VNC pointer click incremented the browser counter; VNC Tab+Enter activated
  Reset. This verifies the viewer's own input route, not just local IPC.
* Native Rust pointer changed the actual browser counter; virtual keyboard
  typed `Dock text input verified` into Firefox's address bar. Escape restored
  the address without navigating to an external site.
* Project recall restored both OpenCode and browser windows with distinct
  themes. Dock and Studio retained their original OpenCode session IDs.
* Human takeover aborted a running OpenCode tool. Subsequent text/click and
  prompt requests were refused; same-session resume retained prior context.
* A Dock-targeted click while Studio was active was rejected even when Dock's
  agent grant was enabled.
* Restarting only wayvnc and reconnecting authenticated RFB returned all
  1,600,000 pixels with the same running browser/terminal windows. TigerVNC was
  reconnected afterward. The reproducible setup script was rerun successfully
  against the live desktop without replacing credentials or restarting Sway.

Thirteen native Linux tests pass, including a deliberately unresponsive
Wayland server that proves the pointer transaction times out, and the live
process freeze/thaw test. Local static checks passed before the final test-only
addition; repository-wide validation is recorded in the main handoff.

### Interaction latency measurements

Read-only measurements after the Enter-delay report found a Seattle DERP
network path with 4.43 ms average ping and no loss. An authenticated VNC request
for a small 64×64 region returned in 8–25 ms. A forced uncompressed full
1600×1000 frame took 2.29–2.38 seconds (6.4 MB); that bandwidth baseline does
not measure TigerVNC's normal compressed, incremental display updates.
The compositor uses CPU pixman; wayvnc defaults to a 30 FPS cap. Neither was
restarted or retuned on the basis of the Enter report.

Fresh SSH processes took 450–524 ms even for `true`; multiplexed commands
took 17–23 ms after connection setup. The Mac client now reuses its existing
private stream SSH control socket. Actual session-status calls dropped from
485 ms to 92–125 ms in the Mac adapter measurements. These measurements
separate transport and command setup from OpenCode model response time; no
inference request or model configuration was changed for this investigation.

See [agent click](screenshots/agora-desktop-agent-click.png),
[typed text](screenshots/agora-desktop-agent-text.png), and
[Studio desktop](screenshots/agora-desktop-studio.png) for actual captures.
The screenshots show the test fixtures and genuine OpenCode tool activity.

References: [wayvnc architecture and authentication](https://github.com/any1/wayvnc),
[Sway input interface](https://man.archlinux.org/man/sway-input.5.en).

## Updated transport and multiple outputs

Agora now has upstream WayVNC 0.10.1, NeatVNC 1.0.1 and AML 1.0.0 in
`~/.local/lib/dock-wayvnc/0.10.1`; Ubuntu packages remain installed for rollback.
Rebuild with `sh scripts/build-agora-wayvnc.sh` on Agora. The script verifies
pinned release archive SHA256 values and runs upstream tests. The installed
`~/.local/bin/dock-wayvnc` launcher pins primary capture to `HEADLESS-1` and
disables client-driven resizing. `desktop-start` selects this launcher when
installed. Updating/restarting the transport does not restart Sway or its apps.

The live desktop has two 1600×1000 headless outputs at (0,0) and (1600,0).
Primary port 5901 still presents only the first output, preserving its aspect
ratio. A separate authenticated `--desktop` probe produced a real 3200×1000
frame containing both outputs. This is one composite framebuffer, not two
independent Mac windows or automatic matching to physical Mac monitors.
Workspace recall applies the project's generated solid wallpaper only to the
workspace's exact output. Border colors remain compositor-wide until Sway's
window criteria are generated from the shared project layout roles.
Captures now identify and capture the active project's actual output; Rust
pointer transactions bind that named output so coordinates stay local to it.
Named-output selection and bounds have regression tests; actual capture output
and human-owned input refusal were verified live. New multi-output pointer
delivery was not exercised against the human's active workspace.

Composite streaming is opt-in. Configure the output topology before starting
the composite transport, with the primary output position explicitly pinned.
For an already configured two-output desktop, a separate loopback listener is:

```sh
XDG_RUNTIME_DIR=/run/user/1000/dock-desktop \
WAYLAND_DISPLAY=wayland-1 \
LD_LIBRARY_PATH="$HOME/.local/lib/dock-wayvnc/0.10.1/lib" \
"$HOME/.local/lib/dock-wayvnc/0.10.1-hotplug1/bin/wayvnc" \
  --config="$HOME/.config/dock/wayvnc/config" \
  --desktop --disable-resizing \
  --socket=/run/user/1000/dock-desktop/wayvnc-composite 127.0.0.1 5902
```

Forward port 5902 through SSH to a separate local port before connecting a
viewer. No second unauthenticated listener is exposed. Simultaneous capture
servers can log inability to acquire output power control; actual frames were
verified, and the primary server retains control.

Two limitations were observed rather than hidden: the first frame can be gray
before the next update contains the desktop, and adding an output while the
idle composite server ran caused an upstream SIGSEGV. Bead `dock-96j.24.1`
records the reproduced hotplug crash and validated fix described below.
The primary single-output path was verified separately. The upgrade includes
upstream frame/cursor fixes, but does not establish that every reported visual
glitch is resolved. See [WayVNC releases](https://github.com/any1/wayvnc/releases)
and [NeatVNC releases](https://github.com/any1/neatvnc/releases).

### Adopted hotplug fix

The hotplug crash was reproduced under GDB in a separate temporary Sway
compositor, with no access to the active project's outputs. An added composite
display lacked its initial framebuffer while capture was idle. The next
connection entered NeatVNC's buffer-failure cleanup and crashed in
`stream_tcp_destroy → aml_obj_unref`.

`config/linux/patches/wayvnc-idle-hotplug.patch` initializes each newly added
display using WayVNC's existing placeholder-frame function. Build the isolated
fix with `sh scripts/build-agora-wayvnc-hotplug-fix.sh`; it installs to
`~/.local/lib/dock-wayvnc/0.10.1-hotplug1` and uses the existing versioned
NeatVNC/AML libraries. It does not modify the primary launcher.

The candidate passed five successive idle output additions followed by RFB
handshakes, output removal/repositioning followed by another handshake, and
authenticated capture of 960,000 actual pixels from two differently colored
800×600 test outputs. The test client advertised desktop resizing, as the real
viewer does. Upstream tests passed and the reproducible build script was rerun.
See [actual isolated capture](screenshots/wayvnc-hotplug-proof.png).
All test services and the temporary tunnel were stopped afterward.

The primary now runs `0.10.1-hotplug1` on its fixed single output. Adoption is
explicit: `sh scripts/adopt-agora-wayvnc-hotplug-fix.sh --restart` updates the
launcher and restarts only WayVNC; omitting `--restart` installs the launcher
without interrupting a connection. The previous launcher is preserved at
`~/.local/bin/dock-wayvnc.pre-hotplug1`. To roll back, copy it over
`~/.local/bin/dock-wayvnc` and restart only `dock-wayvnc`.

After adoption, an authenticated connection returned all 1,600,000 primary
pixels with actual desktop content. Focus, all five project window IDs, and
both original OpenCode session IDs were unchanged. Bead `dock-96j.24.1`
records the completed fix. This workaround addresses the reproduced
missing-frame trigger; it does not claim to repair every NeatVNC cleanup path.

## Human SSO navigation

`desktop-browser-url PROJECT HTTPS_URL` accepts only the official
`https://sso.fpl.dev/desktop-bridges/pair?id=PUBLIC_ID` form. It opens a persistent
project web Firefox profile at `~/.local/state/dock/web/PROJECT`, preserving
the existing demonstration browser tabs. It passes arguments directly and
uses the isolated compositor's D-Bus session for subsequent tab requests.
The result acknowledges a mapped browser and requested navigation; it explicitly
does not assert successful authentication. The human completes SSO consent.
The real pairing flow opened window `dock-web-studio` with the official FPL
Sign In page; a read-only screenshot verified it beside the preserved OpenCode
terminal and original browser. No credentials or consent were entered by the
agent during this verification.
