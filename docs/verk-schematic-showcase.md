# Verk schematics and reference scope

Canvas presents owner artifacts; it does not reconstruct schematics, resolve
SPICE models, or simulate circuits. Legion owns canonical circuit parsing,
graph replay and SPICE. KiCad owns native drawing export. Existing shared PDF
assets and GFX `signal_view` primitives own presentation on the TV.

## What is actually shown

`verk-eda` shows three prepared native-source schematic drawings and their
corresponding **exact graph replay** records. The two inspected owner batches
contain 333 successful graph replays. Only three drawings have been exported
and provisioned; this is not a completed 333-drawing carousel. Exact graph
replay proves connectivity/component identity, not native page-layout
recreation, recovered original edit history, electrical validity or training
admission. The external records remain research-held with unverified rights.

`verk-spice` is a **separate owned RC reference**, not the LED blinker.
The LED source lacks SPICE model properties for its NE555D and LED, as well as
an explicit simulator supply. Keep its original receipt `spice: not-run`.
No invented 555 model or synthetic waveform is used to bypass that boundary.

The completed reference is IN–R1(1k)–OUT–C1(159n)–GND, driven 0→5V at 100μs.
The existing Legion worker's transient request runs for 3ms with a 1μs requested
step. ngspice's adaptive output contains 3,018 points; measured 10–90% rise is
349.358μs. Canvas explicitly resamples those actual points piecewise-linearly
to 1,024 uniformly spaced display samples, retaining the real time interval.
The result is a finite artifact, never a live sensor heartbeat.

## Source and export ownership

The selected Spark owner archive is
`/home/fpladmin/datasets/verk-external/schematics-20261010`.
The two `population-8k-dcp0000-0500-v2` and
`population-8k-dcp0500-1000-v2` report files provide status, project, parts,
nets, pins, canonical graph digest and the digest/path of each native source.
Read only narrow record fields, not their nested frozen dataset rows.

Use the existing KiCad image
`sha256:0f2334cfe7dc27b680900f3967986486bfd2873bcce9f21cc65c3afa258453c6`:

1. Verify the selected native source digest against its owner record.
2. Run `kicad-cli sch export svg --exclude-drawing-sheet
   --no-background-color` in the bounded existing container. Mount the selected
   source read-only and a new task-owned output directory; use network-none,
   read-only root, no capabilities, 512MiB, one CPU and a 30s deadline.
3. Render the transparent owner SVG with existing `rsvg-convert`. Measure its
   alpha-ink bounds with FFmpeg `alphaextract,bbox`, not paper-border removal.
   Add breathing room and export a cropped **vector PDF** with rsvg-convert's
   page-size/top/left options. Do not infer electrical topology from pixels.
4. Render the PDF with Poppler and visually check every label/symbol is retained.
5. Hash and size the PDF, provision its immutable bytes through the documented
   authorized ADB `files/assets/SHA256` path, and use the shared `AssetRef`.
   Never expose Kubo RPC or arbitrary file paths to the PDF widget.

## Bounded scene carousel

`canvas-artifact-carousel MANIFEST TVCTL ENDPOINT DWELL_SECONDS TRANSITIONS`
uses the shared deterministic cursor and bounded owner-local exchange.
It accepts either complete scene documents or `canvas.artifact-carousel.v1`:
one `template`, with `entries` holding immutable PDF `asset`, `title`, and
source-backed caption `body`. It modifies only that workspace, preserves every
other workspace and selection, and terminates after an explicit finite count.
The operator must provision and verify the prepared assets first. This command
does not imply discovery, remote export, raster-readiness acknowledgement,
crossfade, or a perpetual background service. A failure is explicit.

The current private prepared pool contains three PDFs totaling 51,438 bytes.
No source schematic/archive/receipt is deleted or rewritten. Scaling to the
full owner inventory requires lazy owner export, a bounded derived cache and
refresh-at-wrap; it is follow-up work, not claimed by this initial pool.

## Reproduce the reference scope

1. Reuse the existing owner `experiment-worker-v1/input/transient.json`.
2. Run pinned image
   `sha256:c17bfb3136a96219fae1b60937efc84cedda5c2bce3ddf32e61bd022a524322a`
   with input read-only, a new output leaf, network-none, read-only root,
   no capabilities, 512MiB, one CPU, 64 PIDs and a 15s outer deadline.
   The owner worker receives `/input/transient.json /output/reference-transient`.
3. Retain request, receipt and execution together. Observe the pinned image's
   `ngspice --version` (42 for the current image). Missing/failed/cancelled owner
   receipts cannot become a displayed trace.
4. Run `canvas-spice-projection EXECUTION RECEIPT REQUEST OUTPUT IMAGE_SHA256
   NGSPICE_VERSION`. It verifies canonical source, request and execution
   digests, exact admitted RC stimulus and complete state, rejects nonfinite or
   nonmonotonic points, and writes a new bounded `canvas.spice-trace.v1` artifact.
   Image/version pins are explicitly operator-recorded run provenance, not
   magically proven by the old owner's receipt schema.
5. Compose with `canvas-widget-host config/binders/spice-reference-v1.lua`.
   The bounded Lua module only maps data to shared GFX/card primitives.
6. Fill `config/workspaces/shared/verk-spice.json` with that composed view and
   import through the revision-bound owner API; activate only when requested.

The TV's currently installed core incorrectly marks retained, completed sample
signals `no_data` unless a transient frame arrives. The shared endpoint fix
preserves declared sample readiness while retaining missing-live delivery
semantics. Deployment of that core fix requires the normal app update; do not
introduce a fake live heartbeat to conceal it.

## Same-circuit LED comparison and modeled transient

The `verk-led` TV workspace pairs the native LED sheet with a **reconstructed
physical-pin connectivity drawing**, alternating only the drawing. It is not
a recreated native layout or recovered designer history. The matching finite
waveform stays retained and unchanged throughout both phases. True replayed
KiCad drawing export is tracked separately in `dock-z7nh`.

The circuit owner is LoB's `555-led-experiment` example. It uses the existing
SPICE deck generator with the pinned upstream transistor-level UA555 model,
a generic red LED model and an explicitly declared 5 V supply at J1.2/J1.1.
Neither semiconductor model is a characterization of the original part.
The completed one-second experiment is not a live oscilloscope instrument.

Manual runbook (never put private dataset/model artifacts into Git):

1. Obtain the owner's `source.net`, `experiment.cir`, `models.lib`,
   `execution.json`, `receipt.json` and `circuit.json`. Keep the completed
   episode header/footer as independent exact graph evidence.
2. Run `canvas-spice-projection modeled EXECUTION RECEIPT SOURCE DECK MODELS
   OUTPUT VERSION`. Each supplied artifact must match its owner receipt digest;
   the execution must match the receipt and be complete. The version is
   operator-recorded. Adaptive points are explicitly resampled to 1024 uniform
   display samples; the declared supply contributes to the voltage range.
3. Compose the admitted LED observation with `spice-modeled-v1.lua`. The binder
   rejects another source/probe/supply/timebase rather than mislabeled RC data.
4. Run `canvas-circuit-comparison TRACE CIRCUIT WIDGETS NATIVE_WORKSPACE PROOF
   OUTPUT [CONNECTIVITY_SVG [PLOT_SVG]]`. It checks source identity, exact replay
   completion, physical pin groups and component inventory. Connectivity lines
   carry actual pin labels and values, with no inferred signal direction.
5. Compose the waveform as `signal_view` with `frame.view = "plot"`, using the
   canonical `gfx::plot` contract: physical X/Y domains and units, stable trace
   identity, semantic theme role and explicit `{x,y}` observations. The Lua
   binder supplies this frame; do not substitute a PDF screenshot for the
   widget. The one canonical `plot` stores observations without a duplicate
   Scope sample array. Desktop paints via Iced/wGPU, Android via its hardware
   Canvas (the installed TV reports Skia/OpenGL), both using shared geometry.
   This says nothing about measured frame time.
6. For the constrained TV comparison pane, convert the **connectivity** SVG
   using existing `rsvg-convert --format pdf`, provision immutable bytes through
   the established private asset path, then replace only the second phase's
   drawing content with that PDF AssetRef. This conventional engineering-paper
   artifact matches the native schematic and avoids the current dense topology
   label clipping; it is not original-layout recovery. Keep the Plot widget in
   both phases byte-for-byte identical, then import revision-bound.
7. Run `canvas-artifact-carousel MANIFEST TVCTL ENDPOINT 20 30` for a bounded
   roughly ten-minute preview. Drawing-only `apply_patch` operations preserve
   the exact same plot surface; switching to another workspace ends the command
   visibly rather than taking over the newly selected project. Repeat manually
   as desired; this is not an unadvertised forever-running daemon.

The initial PDF plot was replaced by the actual Plot widget in APK 44. The
retained-sample core fix preserves ready finite evidence without a fake live
heartbeat. Both drawing phases were checked on the TV with the same readable
axes, grid and trace legend. Typed viewport bounds support subsequent view
controls; pointer/remote zoom interaction is not claimed here. Desktop's
existing geometry CacheState is reused, keyed once per Program by the frame
and resolved theme; invalid geometry/labels paint an explicit error.
Private receipts and derived assets are retained under
`~/.cache/dock/canvas/verk-led-source`.
