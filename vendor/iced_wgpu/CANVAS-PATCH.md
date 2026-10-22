# Canvas frame admission patch

Upstream: `iced_wgpu` 0.14.0 from crates.io, Iced MIT license. All renderer,
shader and surface configuration code is unchanged except the integration
in `src/window/compositor.rs` and crate-private shell visibility in `engine.rs`.
The new `src/window/frame_gate.rs` owns the admission/completion policy.

Iced exposes custom primitives but no whole-window submission interception.
This pinned source patch is temporary until an upstream admission hook exists;
do not maintain a second renderer or implement throttling in each widget.

Native window rendering admits one unfinished frame **per native window**.
Permits are owned by the surface, so an animated window cannot starve another
display and destroyed surfaces retire their workers. Busy redraws submit no GPU
work and store no frame/scene
history. A dedicated worker waits for that frame's submission to complete, then
wakes the shell only if redraws were coalesced. State updates remain live;
the next draw uses current state. The event loop never waits for GPU completion.
Render preparation and custom-primitive uploads happen only after admission.
Offscreen screenshots and independently owned external-app GPUs are not gated.
The bound counts whole window frames, not individual driver jobs within a frame.
Total unfinished UI frames are bounded by the number of live native windows,
not by the number of redraw attempts; the single-window TV has a bound of one.

`CANVAS_GPU_MAX_IN_FLIGHT=0` disables admission for comparable A/B measurement;
default/`1` enables it. Other values fail loudly. Optional
`CANVAS_GPU_FRAME_METRICS=1` logs completed submissions, coalesced attempts and
submit-to-completion wall time; this is not presentation FPS or shader timing.
Completion failure is visible and fails the next admission rather than freezing
silently. A lost/outdated surface cancels its reservation so recovery can proceed.

On upgrade, reapply only these three touched files to the new upstream source,
rerun admission tests and actual GPU completion tests, then repeat display,
input and scheduler measurements on Home Pi before deployment.
