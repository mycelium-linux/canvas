# Handoff: click-drag orbit for streamed models

Updated after the local implementation was completed and verified. Nothing is
deployed yet. The remaining end-to-end blocker changes shared infrastructure —
the Unibus router on home-pi — and still requires explicit review before anyone
runs it.

## The problem this solves

The model is rendered by `canvas-model-host` on the Mac Studio and reaches every
screen as H.264. Video travels one way, so a pointer on a screen is several hops
from the camera it wants to move. This adds the return path.

## Shape of the chain

```
neo: pointer/key on the surface
  -> OrbitController decides a pose            crates/canvas/src/model_orbit.rs
  -> Canvas writes <frames>/<stream>.camera    crates/canvas/src/main.rs
  -> canvas-edge forwards it                   crates/canvas/src/bin/canvas-edge.rs
  -> unibus  canvas.model.camera.v1            [ROUTE NOT YET ADDED]
  -> canvas-edge on the Studio writes the same file
  -> canvas-model-host reads it each frame     crates/canvas-model-host/src/main.rs
  -> the video everyone is already watching turns
```

Two design points worth keeping:

- **The pose is absolute, never a delta.** A dropped or reordered delta leaves
  the camera permanently out of step with the hand that moved it and nothing
  ever corrects it. A dropped absolute pose is invisible as soon as the next one
  lands. It also makes the channel idempotent and replayable.
- **Latest-value, not a queue.** A drag makes ~30 poses a second and only the
  newest is ever wanted. One file per stream, replaced whole. There is
  deliberately nowhere for an old pose to wait.

## What is written

All of this is uncommitted working-tree state in `~/src/fpl/dock`.

| File | What it gained |
| --- | --- |
| `crates/canvas-core/src/lib.rs` | `model_camera` module: `ViewerCamera`, `NAMED_VIEWS`, `named_view`, `view_at`, `camera_file`, `CAMERA_CONTRACT`, `HOLD_MS`. Shared so Canvas, the edge and the renderer cannot disagree. |
| `crates/canvas/src/model_orbit.rs` | **new.** `OrbitController` — the pure input→pose state machine, plus `KEYBINDS` and `action_for_key`. No GPU, no filesystem, no clock; 14 tests. |
| `crates/canvas/src/retained_video.rs` | The video widget reports gestures (`OrbitEvent`) through an optional hook. It interprets nothing. |
| `crates/canvas/src/main.rs` | `Message::ModelOrbit`, `handle_model_orbit` (throttled publish), `apply_orbit_action`, `focused_model_stream`, `orbit_action` key mapping, `publish_model_camera`. |
| `crates/canvas/src/bin/canvas-edge.rs` | `start_camera_bridge` / `changed_cameras` / `forward_cameras` / `apply_camera_pose`, and a `CAMERA_KIND` arm in `ingest`. 7 tests. |
| `crates/canvas-model-host/src/renderer.rs` | `render` now takes a settled `eye_direction` + `distance_scale` instead of sampling the flight and orbit itself. |
| `crates/canvas-model-host/src/main.rs` | The host settles the camera each frame: viewer pose outranks scene fly-in and idle orbit, eased not assigned. `ease_toward`, `read_viewer_camera`, `unix_ms`. 4 tests. |

Current focused test counts: `canvas-core` 108, `canvas --lib` 282,
`canvas-edge` 30, `canvas-model-host` 30. All passing. Strict Clippy could not
run because the installed stable Apple ARM toolchain has no applicable
`cargo-clippy` component.

## Controls

Drag to orbit. Scroll to zoom. Keys apply **only while a model surface has
focus** — they are bare digits and arrows, and claimed unconditionally they
would eat what someone is typing into the prompt on the same board.

| Key | Action |
| --- | --- |
| `1`–`6` | front / back / right / left / top / bottom |
| `7` / `8` | isometric / isometric rear |
| `0` | reset to the opening view |
| arrows | turn 15° |
| `+` / `-` | zoom |

A cube face, a keybind and a drag all end in the same two numbers, so the three
ways of aiming cannot drift apart.

The model workspaces declare `orbitable: true` on their `live_video` content.
The flag defaults false and is omitted when false on the wire, so ordinary
cameras, games, and browser streams neither steal pointer gestures nor show a
view cube they cannot use.

## Blocked: the router change

This is the step that was rejected, and it is the only thing between the code
and a working orbit. It edits **shared installation state on home-pi**, so it
deserves a look before it runs.

It would have done two things to `/home/ajm/.config/unibus/router.json`:

1. Added a route
   ```json
   { "kind": "canvas.model.camera.v1",
     "target": { "mode": "any_holder", "name": "canvas.model" } }
   ```
2. Added the capability `canvas.model` to the node `canvas-studio-display`,
   so poses reach the host that actually runs the renderer and nowhere else.

then `sudo systemctl restart unibus-router.service`.

Things to weigh before doing it:

- `any_holder` delivers to **all** live holders, despite the name. With only
  `canvas-studio-display` granted that is one host, but adding the capability
  anywhere else silently fans poses out.
- The router config on home-pi has **no copy in this repo**, so this edit exists
  only on that machine. That is a pre-existing gap, not one this change
  introduced, but it means the change is invisible to review.
- The script took a `.bak` first. It did not run, so no backup exists yet.

## Remaining work after the route

1. **Deploy**: `canvas` + `canvas-edge` to neo; `canvas-model-host` +
   `canvas-edge` to the Studio (`ajmwagar@100.108.9.93`).
2. **Verify end to end**: drag on neo → `<frames>/transmog-step.camera` appears
   → the same file appears on the Studio → the streamed model turns.

The model-host plists now pass their required eighth stream-id argument and
were parsed with Python `plistlib`. The clickable eight-view overlay is also
built and highlights the active named pose.

## Context that is easy to lose

- **Latency is inherent.** The pointer is on neo, the renderer is on the Studio,
  and the result comes back as video. Expect a visible delay on the order of a
  few hundred milliseconds. The easing in the host (`CAMERA_EASING`, 25% of the
  remaining gap per frame) is what keeps that from feeling like stutter: one
  rule covers takeover, uneven network arrival, and handing back to the scene.
- **One renderer means one camera.** Whoever drags, every screen showing that
  stream turns. That is a property of rendering once and broadcasting, not a
  bug, and arguably the point.
- **A held camera never expires.** `grabbed: true` suspends the hold timeout so
  studying one angle for a minute does not let the scene pull the view away.
  The release is published even when the throttle would suppress it — dropping
  it would strand the renderer believing a pointer is still down.
- Recent related commits: `cbc6e5b` (720p + launchd QoS), `6cb0e59` (decoder
  publishes its frame size).
