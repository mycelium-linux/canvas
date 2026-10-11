# Verk recovered-model showcase

Canvas presents the owner's verified reconstructed geometry. It does not own CAD
reconstruction, STEP interpretation, feature recognition, or dataset admission.
`canvas-model-catalog` captures a bounded, digest-addressed owner inventory;
`canvas-model-carousel` invokes the existing Transmog `import-part` STEP→GLB
interface lazily. Original STEP files and receipts are never modified or evicted.

## Contracts

- `canvas.model-catalog.v1`: stable sorted generation of verified STEP paths,
  corresponding receipt paths, SHA-256 digests and measured geometry evidence.
  Finish the generation before refreshing at wrap; growing inventory cannot
  starve older models.
- Model scene: current `asset`, optional preloaded `next_asset`, `orbit`,
  `camera_distance`, `crossfade_ms`. Safe relative GLB names only.
- Renderer acknowledgement: `current_asset`, `next_asset`, `next_ready`,
  `completed_orbits`, `observed_at_ms`. Identity is not the requested scene.
- `canvas.model-showcase.v1`: acknowledged current identity and owner evidence,
  catalog generation/count, explicit failed-preview count and optional recovered
  feature counts from the owner's existing `recognition.json`.
- `model-showcase-v1.lua` composes the right-hand card. Missing recognition
  metadata is missing, not zero; recovered features are not original design
  history. Verified geometry is not training admission.

Two GPU slots are resident: current and next. A successful bake/upload must
acknowledge readiness and the current model must complete at least two rendered
orbits before requesting a 500ms crossfade. Rotation continues while conversion
or preload is pending. The previous derived GLB is removed only after successful
promotion acknowledgement. Each derived model is bounded to 32MiB; temporary
copied STEP input is bounded to 8MiB and removed after conversion. A failed
conversion is reported and skipped; a failed GPU preload holds the current model.

## Deployment and manual runbook

1. Capture the explicitly selected owner's inventory with `canvas-model-catalog`.
   Supply the authorized owner SSH route; discovery does not grant access.
2. Configure the private operator carousel JSON: owner route, Studio host,
   existing Transmog CLI, catalog, scenario and derived-cache paths. Set
   `orbit_count=2`, `degrees_per_second=18`; no all-inventory eager conversion.
3. Install the independent Canvas widget host and carousel/catalog binaries.
   Install the Lua binder outside the source checkout.
4. On Studio install `dev.fpl.canvas-verk-showcase`: hardware encoder produces
   the model stream. `CANVAS_MODEL_STATE_FILE` enables acknowledgement-based
   carousel rendering; `CANVAS_MODEL_PUBLISH_LOCAL_FRAME=0` avoids unnecessary
   local RGBA writes. Stop only the showcase's temporary camera driver before
   enabling automatic orbit, otherwise manual-camera takeover correctly prevents
   automatic-orbit accounting.
5. Neo's dedicated `dev.fpl.canvas-verk-relay` receives Studio's private UDP
   stream on the tailnet interface and stream-copies MPEG-TS over authenticated
   SSH to Home-Pi loopback UDP. This avoids the unreliable inter-LAN UDP route.
   Home-Pi stream-copies into a rolling bounded HLS playlist served only on its
   private TV-facing address. Do not touch the separate CCTV or Transmog relays.
6. Install `dev.fpl.canvas-verk-carousel` and `dev.fpl.canvas-verk-caption` on Neo.
   The controller atomically publishes a private bounded observation file; the
   independently restarted presenter updates only the provenance card with
   revision-bound owner imports. A TV failure cannot terminate the controller.
   Other workspaces and the selected workspace are preserved.
7. Import `config/workspaces/shared/verk-showcase.json` through Canvas's owner API.
   Select the showcase only when requested; the model occupies the left two
   thirds, readable evidence occupies the right third.

Verify source FPS, advancing HLS media sequence, actual TV motion, and at least
three distinct acknowledged model identities. Observe two complete rendered
orbits before each transition. Never claim the full inventory has been *shown*
merely because it is in the catalog. Source failure must retain the last identity
with its original renderer timestamp, so the card becomes stale instead of
pretending to be fresh. A player completion/error requires bounded live-stream
recovery; it must not blindly loop arbitrary finite video.

Studio and Neo clocks currently differ by several seconds. The projection uses
Neo's receipt timestamp only when the renderer's observation timestamp advances;
re-reading a frozen acknowledgement cannot refresh it. Owner reconstruction
evidence and the catalog generation keep their own separate timestamps.
