# Control plane: the tmux fleet as JSON

Dock's control plane makes a running Pi fleet observable and actionable without
an agent, a terminal, or a hand-typed command. Phase 1 is deliberately small: a
loopback JSON service that lists the panes of one tmux session and offers four
pane actions. Canvas draws one widget per pane over that service. Phase 2
replaces the polling edge with a live terminal stream while keeping the same
contract.

This document is the interface (`scripts/cp-server.lua`) and the reasoning behind
it. Presentation stays in Canvas, transport stays in the adapter, and the panes
stay owned by tmux.

## Ownership boundaries

| Fact / capability | Owner | Control plane's role |
| --- | --- | --- |
| pane tree, window/pane IDs, layout | tmux | read only, through `tmux list-panes` |
| worker names, fleet window, keystroke semantics | `pi-tmux` | forward actions to it; never reimplement |
| presence classification (idle / working / done / error) | `pi-tmux status` / `presence` | not reimplemented; see "phase 2" |
| what is on screen, layout, attention | Canvas document | not stored here |
| HTTP surface and JSON shape | `scripts/cp-server.lua` | this document |

The adapter is stateless between requests: every response is derived from live
tmux state plus `pi-tmux`'s worker state file. Nothing is cached, so a killed
pane can never be reported as alive because a snapshot said so.

Keeping names and keystrokes in `pi-tmux` is the "call, don't rebuild" rule.
`pi-tmux` already resolves a bare worker name (state file, then fleet pane title,
then prefixed window name), pastes multi-line text through a tmux buffer so
newlines survive, and presses Enter to submit. The server passes an explicit
`%N` target when it knows one and falls back to handing the bare name to
`pi-tmux` when it does not. Both paths end in the same owner.

## The fleet document

`GET /api/panes` returns one document. It is a snapshot, not a subscription;
consumers poll it or re-fetch after any action.

```json
{
  "session": "12",
  "fleet_window": "pi-fleet",
  "generated_at": "2026-10-21T07:45:49Z",
  "count": 2,
  "panes": [
    {
      "name": "alpha",
      "pane_id": "%33",
      "window": "pi-alpha",
      "window_index": 0,
      "title": "Averys-Mac-Studio.local",
      "command": "bash",
      "path": "/Users/ajmwagar/src/fpl/dock/.worktrees/control-plane",
      "active": true,
      "width": 80,
      "height": 24,
      "worker": true,
      "fleet_window": false
    }
  ]
}
```

`?workers=1` narrows the list to panes that look like Pi workers, using the same
filter `pi-tmux list` uses: a `pi-` prefixed window or pane title, or a pane
running a `pi`/`node` command.

### Names

A name is the stable handle a widget, a human, and an agent all use. Resolution
order, most authoritative first:

1. the name recorded in `~/.pi/agent/pi-tmux.state` for this pane or window;
2. the fleet pane title with the `pi-` prefix stripped (what `pi-tmux spawn
   --split` sets);
3. the window name with the `pi-` prefix stripped;
4. the raw window name, and finally `<window>-<pane id>` for a bare second pane
   in an unmanaged window.

Names are unique within a session; a collision falls through to the next rule.
`pane_id` is always present, so a client that needs certainty should key its
surface on `pane_id` and display `name`. Names are accepted by the action routes
and match `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`; anything else is a `400` before a
single argument reaches `tmux`.

## Routes

| Method & path | Body | Result |
| --- | --- | --- |
| `GET /api/panes` | — | fleet document above; `?workers=1` filters |
| `POST /api/pane/{name}/send` | `{"message": "...", "submit": true}` | `{"ok":true,"name","target","chars","submitted"}` |
| `GET /api/pane/{name}/capture?lines=N` | — | `{"ok":true,"name","target","lines","text"}` |
| `POST /api/pane/{name}/kill` | — | `{"ok":true,"name","target","killed"}` |

`send` pastes through `pi-tmux send`, so multi-line prompts keep their newlines.
Any path not in this table is `404` with `{"ok":false,"error":"not found"}`;
a path in the table called with the wrong method is `405` with
`{"ok":false,"error":"no such route for METHOD"}`. The router keeps those two
cases distinct, so an unknown path never claims to be a known route.
`submit` defaults to `true` and appends the single Enter that submits a Pi
editor; `submit: false` stages text without running it. `capture` returns the
visible pane text oldest-first via `pi-tmux capture --lines N` (`N` defaults to
80, is clamped to 1..2000, and a non-integer is a `400`). `kill` kills the pane
or window through `pi-tmux kill`; killing the fleet window itself is refused
with `409`, matching `pi-tmux`'s own refusal, because the fleet is a container,
not a worker.

Every failure is JSON and explicit — no silent fallbacks:

| Status | When |
| --- | --- |
| `400` | bad name, bad `lines`, invalid JSON, empty or non-string message |
| `404` | unknown route, unknown pane, or a name whose pane is gone since the snapshot |
| `405` | known path with the wrong method (e.g. `POST .../capture`) |
| `409` | refusing to kill the fleet window |
| `413` | body over 1 MiB |
| `502` | `tmux` or `pi-tmux` returned an error; its message is passed through |
| `504` | the `tmux`/`pi-tmux` call exceeded its timeout |

A pane that vanished between listing and acting returns `404` rather than an
`ok: true` with a stale target, because the caller's next move (retry, respawn,
or drop the widget) depends on the difference.

## Canvas: one widget per pane

Canvas renders the fleet as *surfaces*, one per pane, keyed by pane name or
`pane_id` — one surface per agent, not one surface per event, which is the
level-of-detail rule in `dock-ybc.22`. The ladder maps directly:

- **fleet** — one card summarizing `GET /api/panes`: how many panes, how many
  workers, which are active. No pane text.
- **agent** — one widget per pane: name, window, command, path, active flag, and
  a bounded `capture?lines=8` tail. This is the rung the wall shows by default.
- **detail** — the pane's full text buffer at surface size, scrollable.

Widget interactions map one-to-one onto the routes, and Canvas does not invent
state for them:

| Widget affordance | Control-plane call | What Canvas must not do |
| --- | --- | --- |
| prompt box + Send | `POST .../send` | echo the message as if delivered; show it only after `ok:true` |
| body / tail / refresh | `GET .../capture` | treat captured text as a document revision |
| Click-to-focus | `POST .../send` intent is wrong — focus is tmux's; today Canvas raises its own surface and the operator can jump with `pi-tmux focus NAME` | claim the tmux window is focused |
| Close / Kill | `POST .../kill`, then drop the surface | keep the surface after `ok:true` |

The projection rule matters more than the routes: a widget is a *view* of tmux
state, rebuilt from the fleet document plus a capture. Canvas keeps the layout,
title, theme, and attention for that surface in its own document (see
[canvas-data-model.md](canvas-data-model.md)); it never stores pane identity
facts that tmux already owns, so a revived session with new `%N` ids reattaches
by name rather than resurrecting a stale surface.

Coherence palettes come from `dock-core` as everywhere else; the control plane
adds no colors and no second theme.

Focus is the one affordance that has no `pi-tmux` route of its own. Rather than
have Canvas fabricate a focus notion, phase 1 keeps focus in the terminal and
the tmux status bar, and Canvas focus means "raise the surface". If interactive
focus from a widget becomes worth it, it becomes a fifth route that calls
`pi-tmux focus NAME` — the same delegation the other three already use.

## Deployment

```sh
scripts/cp-server.lua                       # 127.0.0.1:8791, session auto-detected
scripts/cp-server.lua --session 12          # explicit tmux session
CP_PORT=8795 scripts/cp-server.lua          # override the port
```

**Runtime.** Lua 5.4 plus luasocket, the only socket binding Lua has:
`brew install lua` on macOS, `apt-get install -y lua5.4 lua-socket` on
Debian/Ubuntu. Nothing else is required: JSON, the HTTP write path, argument
quoting, and process execution are the Lua standard library. A missing luasocket
is a startup error with the install hint, not a silent fallback.

Every `tmux`/`pi-tmux` call is `os.execute` of an argv array whose elements are
single-quoted, so a pane name or message is never reinterpreted by a shell; the
message is one argv element, which is why multi-line text survives intact.

Session discovery is explicit-first: `--session`, then `CP_SESSION`, then
`PI_TMUX_SESSION`, then the current tmux client, then the single session owning
a `pi-fleet` window. Anything ambiguous is a startup error naming the candidates
rather than a guess. Startup also lists the session's panes once, so a wrong
session fails immediately instead of on the first widget poll.

Defaults are `PI_TMUX_PREFIX=pi`, `PI_TMUX_FLEET=pi-fleet`,
`PI_TMUX_STATE=~/.pi/agent/pi-tmux.state`, and `PI_TMUX_BIN` from `PATH` or
`~/.pi/agent/bin/pi-tmux`.

**Port 8791.** The contract default is 8791. On the Mac that port is already
owned by Alfalfa's local service, so the default bind fails loudly and names the
conflict; run with `--port 8795` (or `CP_PORT`) there. The intended home for the
control plane is the host that runs the fleet, where 8791 is free.

**Exposure.** The service binds loopback and speaks no authentication, because
`send` is a keystroke-injection primitive equivalent to typing into the
operator's shell. It must not be exposed on Tailscale or the LAN. Remote control
goes through SSH to the local service, the same shape the OpenCode backend uses
([opencode-sessions.md](opencode-sessions.md)); binding a non-loopback address
prints a warning and is a deliberate, reviewed act, not a flag to sprinkle.

## Phase 1 limits

These are real, not polish debt:

- **Capture is a lossy snapshot.** No colors or attributes, no cursor, no
  scrollback beyond tmux's history limit, and a busy pane loses intermediate
  frames. Fine for a tail, wrong for a terminal.
- **Polling costs a process per pane per interval.** `list-panes` is cheap;
  `capture` is a `tmux` client per widget. A ten-pane wall polling capture every
  second is ten processes a second, which is why the tail is bounded and the
  dashboard defaults to the fleet rung.
- **Input is paste-plus-Enter, not terminal input.** Good for submitting a
  prompt to a Pi worker, unusable for interactive TUIs, conditional prompts,
  pagers, or anything that needs arrow keys or control characters.
- **No presence classification.** Deriving idle/working/done/error from pane
  text is `pi-tmux`'s classifier. When Canvas needs it, the server should call
  the owner (`pi-tmux status`) and shape its output, not grow a second copy of
  the heuristics.
- **No wall-clock timeout on delegated calls.** The Lua stdlib has no portable
  way to bound a child process, so a wedged `tmux` call blocks the one request
  that made it. tmux answers in milliseconds and phase 2 replaces the polling
  path with a stream, so this is accepted; a timeout wrapper is the first thing
  to add if a wedged call is ever observed.

Each limit points at the same missing piece: a real terminal stream.

## Phase 2: a live terminal per pane

Recommendation: **embed a tmux control-mode client, one per fleet or per
session, and feed each pane's output into a VT grid that Canvas renders.**

Concretely:

1. Run `tmux -C attach-session -t SESSION` as one long-lived child of the
   control-plane process. Its stdout is line-oriented and documented:
   `%output %N <escaped-bytes>`, `%layout-change`, `%window-add`,
   `%pane-mode-changed`, `%session-changed`.
2. Fan `%output` per `%N` out to the matching surface subscription and keep the
   latest layout notification.
3. Input becomes `send-keys -t %N -l -- <text>` per keystroke batch, so real
   terminal input (arrows, control characters, `C-c`) works and `send`/`capture`
   stop being the only input path.
4. On connect, or after a dropped stream, resynchronize instead of replaying
   history: `capture-pane -p -e -t %N` gives the visible grid *with* escape
   sequences, and `%output` continues live from there. That single call is what
   makes joining mid-stream correct.
5. Canvas renders the grid in a VT surface. Today's `ContentSource::Terminal`
   is a pipe plus a plain text buffer (`crates/dock-canvas/src/terminal.rs`,
   rendered as `text(buffer)`), so this step needs a real grid — an emulator
   crate such as `alacritty_terminal`/`vte` behind the existing surface, not a
   new renderer for the whole document.

Why control mode and not a custom protocol: tmux already solves terminal state
convergence, resize reflow, and reconnect, and it is the same process that owns
the panes. Phase 1's `capture` polling would otherwise have to be reinvented as
a worse streaming protocol. This is the "PTY via control mode" option sketched
in the streaming layer of
[computing-mesh-design.md](computing-mesh-design.md), applied to the one pane
type that needs it; structural (AX/CDP) and opaque (pixel) streaming stay out of
scope here.

Why it is a phase and not a rewrite: the four JSON routes survive as the
structured, agent-facing interface — agents and remote calls stay on HTTP, where
a bounded snapshot is exactly right, while human surfaces upgrade to the stream.
The pane document and its names are unchanged, so the change is additive: one
new subscription route (or a WebSocket/SSE stream) plus a VT surface, with
`capture` retained as the resync and fallback path.

Order of work when phase 2 starts:

1. control-mode client with `%output` demultiplexing and a reconnect/resync
   loop, tested against a killed and restarted tmux server;
2. a VT grid surface in Canvas consuming it, with the widget-per-pane mapping
   above unchanged;
3. real keystroke input (`send-keys -l`, `C-c`) replacing paste-plus-Enter for
   the detail rung, keeping `send` for prompts;
4. presence via `pi-tmux status` once the stream is in place, so the fleet rung
   and the per-pane rung agree on one classifier.

## Validated

Phase 1 was smoke-tested end to end against a disposable tmux session
(`pi-alpha`, `pi-beta`, `pi-fleet`), not the operator's live fleet:

- `GET /api/panes` listed both workers with names derived from `pi-` prefixes
  and correct `pane_id`, size, path, and `worker` flags;
- `POST .../send` typed `echo cp-smoke-$((1+1))` and the subsequent
  `capture?lines=5` showed `cp-smoke-2`, proving the submitted keystrokes ran in
  that pane; a second pane received a multi-line `submit:false` payload and kept
  the last line staged at the prompt;
- `POST .../kill` removed the pane, and the next `GET /api/panes` no longer
  listed it;
- `404` (unknown pane `ghost`), `400` (`lines=zero`), `400` (non-JSON body),
  `405` (`POST .../capture`), `404` (unknown route), and `409` (killing the
  fleet window via a state-file `fleet` entry) all returned the documented JSON;
- `scripts/cp-server-selftest.sh` boots a scratch `cp-404-selftest-*` session on
  an ephemeral port and asserts the status matrix — `GET /api/nope` → `404`,
  `POST .../capture` → `405`, `GET /api/panes` → `200` — exiting non-zero on
  drift, so a future router edit cannot silently restore the old 405-for-404
  behavior;
- binding `127.0.0.1:8791` on the Mac failed loudly and named Alfalfa as the
  owner, rather than silently taking another port.

## Non-goals

The control plane does not spawn panes, decide which worker runs which task,
own queues or retries, or classify intent. Spawning is `pi-tmux spawn` /
`pi-wt` work; scheduling is the orchestration core's. This service answers "what
is running" and carries out four explicit actions on exactly one of those
panes.
