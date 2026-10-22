# Verk model evidence dashboard

`config/workspaces/shared/verk-models.json` is a portable retained scene. CAD
teacher replay, a measured exploratory CAD checkpoint, EDA coverage, and the
existing analog clock are separate surfaces. Teacher execution success is not
learned model quality. EDA trained-model telemetry remains unavailable until an
owner supplies a validated source; it must never appear as zero performance.

Ownership: Verk's private `reporting` crate sanitizes owner evidence. Unibus
transports `verk.model-observation.v1`. Canvas's `verk-model-v1.lua` composes
declared observations into existing widgets, without networking or credentials.
The independently buildable Canvas `canvas-widget-host` executes a trusted
operator-selected binder with a 64 KiB input/output limit, 1 MiB Lua memory
limit, and 500 ms Luau composition budget. It is not an agent harness.

Read-only runbook:

1. Read the explicitly selected owner report (for example Studio's current
   CAD tree-batch `batch-progress.json`) through an authorized private path.
2. Pipe it into `verk-model-observation tree-batch cad-recovery`, or feed the
   supported native training report into `native-cad-training cad-checkpoint`.
   Unknown schemas fail; raw paths/errors are not published. `source_sha256`
   hashes canonical JSON evidence, not original file bytes.
3. Publish normalized observations with the existing `unibus-stream publish
   CONFIG verk/models/cad-recovery verk.model-observation.v1`. Use distinct
   source and consumer identities, not another application's credentials.
4. Subscribe with the expected source producer. Feed each verified observation
   into `canvas-widget-host config/binders/verk-model-v1.lua`.
5. Deliver the composed `widget` and `data` through the owning presentation
   endpoint's `surface_snapshot` command. Use `tv-verk-models` and the matching
   surface ID. Layout remains retained; transient data expires independently.

Expected cadence is 5 seconds; maximum observation age is 30 seconds. A recent
read of an old training report is **recorded evidence**, not a fresh training
run. An incomplete batch does not establish active execution. Ownership and
model/dataset provenance must be attached by the explicit selected source, not
guessed from process names or file names.
The TV's declared refresh interval is 10 seconds, giving a 31-second frame
expiry budget for the five-second publisher plus bounded reads and restart delay.
`unibus/config/verk-model-observation-v2.json` is descriptive metadata, not a
replacement for runtime grants or a claim that the router automatically admits it.

The installed main Home-Pi router reads node credentials once and has no hot
reload. Dedicated `verk-cad-owner` and `canvas-verk-viewer` nodes were enabled
through a user-approved router restart. Their credential files remain operator-
private under `~/.config/unibus/verk`; repository files contain no tokens.
Kind and zone grants are not cryptographic per-stream grants: the consumer also
checks the exact stream, contract and expected authenticated producer.

Neo's `dev.fpl.verk-model-owner` launchagent re-reads the selected Studio owner
artifact, projects it, and publishes every five seconds. Source reads have a
ten-second deadline and a 1 MiB allocation limit. Its stdin is detached so SSH
cannot consume observer input. The matching `dev.fpl.canvas-verk-viewer` agent
subscribes and runs Lua composition, sending transient `surface_snapshot` frames.
Both use Interactive scheduling, KeepAlive and ten-second restart throttling.
Failures stay visible in logs; absent delivery expires on TV. Do not set normal
polls to exceed the declared frame lifetime or treat a re-read as new run evidence.

The sampled checkpoint card is retained, explicitly dated historical evidence;
it is not a transient live-training instrument. Changing selected artifacts is
an explicit operator configuration edit, not discovery of every model on the mesh.

`canvas-fleet sync` copies definitions, not publisher credentials or feed services.
