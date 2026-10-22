# Handoff: click-drag orbit for streamed models

Updated 2026-09-24 after the viewer split and Neo/Studio deployment. The
release Canvas binary is running on Neo and the matching `canvas-model-host`
binary is running for both Transmog and Benchy on the Studio. Pointer-level
end-to-end acceptance still needs a human drag on the visible model.

## The problem this solves

The model is rendered by `canvas-model-host` on the Mac Studio and reaches every
screen as H.264. Video travels one way, so a pointer on a screen is several hops
from the camera it wants to move. This adds the return path.

## Shape of the chain

```
neo: pointer/key on the surface
  -> Viewer state machine decides a pose       crates/canvas-model-viewer/src/state.rs
  -> Canvas writes <frames>/<stream>.camera    crates/canvas/src/main.rs
  -> canvas-edge forwards it                   crates/canvas/src/bin/canvas-edge.rs
  -> unibus  canvas.model.camera.v1
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
| `crates/canvas-model-viewer/src/orbit.rs` | `OrbitController` — the pure input→pose state machine, plus key actions. No GPU, filesystem, or clock. |
| `crates/canvas-model-viewer/src/video.rs` | Retained WGPU video surface and gesture reporting. It interprets no camera policy. |
| `crates/canvas-model-viewer/src/state.rs` | One viewer state machine owns controllers, throttling, gestures, camera/timeline actions, and camera publication. |
| `crates/canvas-model-viewer/src/view.rs` | Compact toolbar, direct-access operation timeline, and the rotating clickable view cube. |
| `crates/canvas/src/main.rs` | Integrates the viewer through its narrow state/view API; it no longer owns orbit or retained-video internals. |
| `crates/canvas/src/bin/canvas-edge.rs` | `start_camera_bridge` / `changed_cameras` / `forward_cameras` / `apply_camera_pose`, and a `CAMERA_KIND` arm in `ingest`. 7 tests. |
| `crates/canvas-model-host/src/renderer.rs` | `render` now takes a settled `eye_direction` + `distance_scale` instead of sampling the flight and orbit itself. |
| `crates/canvas-model-host/src/main.rs` | The host settles the camera each frame: viewer pose outranks scene fly-in and idle orbit, eased not assigned. `ease_toward`, `read_viewer_camera`, `unix_ms`. 4 tests. |

The 2026-09-24 gate passed: `canvas-model-viewer` 23 tests,
`canvas-core::part_inspector` 9 tests, and `canvas-widgets` 49 tests, followed
by checks of Canvas (with Lua scenes) and the model host and release builds of
both deployed binaries.

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

## Router contract

Camera poses use `canvas.model.camera.v1`. The checked-in home-pi deployment
configuration now documents the route and Studio capability. Treat changes to
the installed router as shared installation changes and review them separately
from rebuilding Canvas.

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

## Current deployment and remaining acceptance

1. **Deployed**: release `canvas` on Neo; release `canvas-model-host` on Studio
   (`ajmwagar@100.108.9.93`). Both Studio launch agents are running.
2. **Accept end to end**: drag on Neo → `<frames>/transmog-step.camera` changes
   → the same file appears on the Studio → the streamed model turns.

The model-host plists pass their required eighth stream-id argument. The view
overlay now exposes eleven named poses, including extra chamfer/corner views,
and rotates its visible face labels with the model.

Neo renders the view cube itself from the viewer controller's current pose.
Local cube repainting is intentionally independent of the 30 Hz camera-file
publication limit: every accepted drag sample updates the cube immediately,
while only the latest useful pose crosses to Studio. Before the first gesture,
the cube uses the same default isometric pose as the model host instead of an
unrelated front-facing fallback. This behavior is covered by the viewer's
`throttled_drag_still_reports_a_local_change` regression test and is deployed
in the Neo Canvas binary built on 2026-09-24.

## Part/project direction

The viewer is no longer assumed to be a Transmog-only workspace. Canvas core
can project an Alfalfa revisioned manifest into explicit `parts/<name>` and
`assemblies/<name>` inspectors. An inspector groups model artifacts, drawing
PDFs, metadata, supporting files, and named revisions without inventing IDs or
copying Alfalfa's schema. Canvas already has paged PDF rendering; the remaining
work is runtime composition: selecting an Alfalfa part should open its model
and drawing side by side, and an iteration action should send the bounded
`IterationRequest` (project, manifest revision, part, prompt, artifact IDs and
CIDs) to Transmog. No live Alfalfa Transmog manifest is currently registered on
Neo, so discovery and that combined inspector page are not yet operational.

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
