# Reserve a display for Agora One

Canvas uses its iced diagnostics to keep one or two Mac displays for matching
fullscreen project viewers while moving the reviewed local windows to selected
remaining displays.
The separate, installed **Dock Control.app** owns Accessibility authorization and
native window operations. Updating Canvas itself does not replace that helper.

Launch `canvas-mac --diagnostics`. In a project’s Agora One resource, choose **Reserve display**. Select one or two displays
for streaming, select the destination display(s) for local work, and choose
**Review window moves**. The review lists every affected window. **Start stream**
is the authorization to apply that exact snapshot; simply selecting a project or
a monitor moves nothing.

You can open that selection flow directly:

```sh
cargo run -p canvas-mac -- --diagnostics --stream-host 100.80.85.86
```

The plan includes eligible non-minimized windows intersecting the streaming or
selected destination displays, including windows that straddle display edges.
It stacks those windows across the selected destinations while preserving each
window's current size whenever it fits. This avoids application minimum-size
clamping and keeps each title bar inside the destination's usable frame.
One-display setups fail visibly: Canvas needs somewhere to keep local work
available. Existing fullscreen, fixed-size, or immovable windows in the affected
area must be resolved before starting. Close an existing Moonlight stream before
starting one managed by Canvas.

The project path uses `reservation::start_project(plan, project)` to launch one
shared TigerVNC connection per selected Sway output. Live display inventories
determine the mapping order. Both viewers share one recovery journal and child
lifecycle, so a failed or stopped viewer closes the set and restores the reviewed
local windows. The explicit
`--stream-host` path retains Moonlight for ordinary Ubuntu.

After evacuation, Canvas launches its own viewer process in windowed mode,
places its stream window on the selected display, requests native fullscreen,
and verifies that the fullscreen window belongs to that display. It does not
change resolution or display arrangement, and it never kills another viewer
process. If the stream cannot start, accept its frame, or enter fullscreen on the
chosen display, Canvas stops the process it created and restores local windows.

**Stop stream and restore windows** stops the owned stream and restores original
frames. Normal app shutdown does the same. A stopped or failed owned viewer process
is detected by polling. The host session is independent: stopping the viewer does
not terminate remote project processes.

## Recovery

Before any move, Canvas writes the original window identities/frames and display
layout to:

`~/Library/Application Support/Dock/stream-recovery.json`

Each attempted move is persisted before it is sent, so even an operation that
partially moves a window and then fails is included in rollback. Updates use an
atomic rename with file/directory sync. An advisory file lock serializes setup
and recovery across Canvas instances and releases automatically when a process
exits. A live owning Canvas process blocks recovery from another instance.

After a crash, open `canvas-mac --diagnostics` and use **Restore local windows**. It restores known windows
whose original displays still have the recorded identity and layout. If a display
is disconnected or rearranged, reconnect it to retry. Closed windows and IDs from
a restarted Dock Control helper are reported as unresolved; Dock retains the
journal instead of guessing another window with a similar title. The journal
contains the original frames for explicit manual recovery. A PID saved on disk
is never used to kill a process after a restart.

Owned stream output is kept at:

`~/Library/Application Support/Dock/reserved-stream.log`

Ordinary companion-launched TigerVNC output is appended to
`~/Library/Application Support/Dock/desktop-viewer.log`.

## Validation and boundaries

The pure planner/journal tests cover negative coordinates, straddling windows,
single-display rejection, competing operations, stale review snapshots, partial
native failure rollback, disconnected display recovery, and stale helper IDs.
Native helper focus/move/exact restoration has also been exercised against a
Canvas-owned disposable window. Full multi-window evacuation and Moonlight
fullscreen depend on the approved helper and the actual windows accepting their
requested geometry; failures remain visible and trigger restoration.

Project selection recalls the corresponding persistent Sway workspace, existing
conversation and browser. Reservation is an additional explicit operation; it
does not replace or recreate those project sessions.

Moonlight’s public CLI provides windowed, fullscreen, and borderless modes but no
monitor selection flag. Canvas therefore places the owned window before requesting
fullscreen; it does not change Moonlight preferences or patch its streaming
implementation. See the upstream
[CLI parser](https://github.com/moonlight-stream/moonlight-qt/blob/master/app/cli/commandlineparser.cpp)
and [stream window implementation](https://github.com/moonlight-stream/moonlight-qt/blob/master/app/streaming/session.cpp).

## VNC display and image behavior

The installed TigerVNC 1.16.2 supports fullscreen on the current, selected, or all
Mac monitors. Canvas can reserve **one or two** Mac displays and uses the others
for local windows. The previous viewer, if open, remains a
normal local window in the reviewed snapshot; stopping the reservation restores
it rather than killing it. A second connection uses separately served
`HEADLESS-2`; it does not clone or stretch the first output.

Distinct Linux outputs require a multi-output server framebuffer or separately
served outputs. Stretching a single-output framebuffer over several Mac monitors
is not presented as independent screens. Remote size changes are disabled so the
viewer cannot silently rearrange the server desktop when its Mac window changes.

New viewers request full-color lossless ZRLE with JPEG disabled. This removes
lossy encoding as a source of fuzzy text; it does not establish the cause of an
unobserved rendering glitch. TigerVNC’s own shortcuts use **Ctrl+Shift**, leaving
**Ctrl+Option+Space** available for Canvas. Fullscreen system-key capture is off so
local switching remains available. Existing viewer processes retain their launch
settings until reconnected. These supported flags were checked against the
installed executable and the [TigerVNC manual](https://tigervnc.org/doc/vncviewer.html).

A disposable view-only client using these flags authenticated with X509Plain,
completed TLS 1.3 and RFB initialization, and enabled continuous updates against
the live server. No input was sent and existing sessions/viewers were preserved.
The updated helper fixes Finder inventory when AppKit omits a launch date. Because
the helper is independently installed and ad hoc signed, an explicit upgrade
requires granting Accessibility to the new binary once in System Settings.
