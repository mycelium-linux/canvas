# Canvas MCP

Canvas owns the document, validation and native presentation. MCP is an adapter
over its existing owner-local control socket, not another renderer or command
engine. The current adapter exposes fifteen tools:

The longer-lived contract is the
[shared visualization plane](canvas-shared-visualization.md): agent inspection
and the physical display use the same revisioned view, including its viewport,
attention, page/playhead, filters, and source position. MCP resolves intent and
requests state transitions; it does not maintain a private visual context.

- `canvas_inspect`: atomically returns the document and runtime snapshot.
- `canvas_apply_document`: validates and replaces presentation intent.
- `canvas_apply_patch`: applies the existing atomic document patch operations.
- `canvas_close_surface`: removes presentation without stopping background work.
- `canvas_focus`: brings the Canvas window forward.
- `canvas_display_target`: selects all connected displays or exact display IDs.
- `canvas_set_overlays`: places host-local overlays by display role.
- `canvas_set_visibility`: shows or hides all Canvas host windows atomically.
- `canvas_navigate`: changes pages in an already-open PDF surface.
- `canvas_open_file`: opens an image, PDF, or Markdown file through the native
  content loader.
- `canvas_mermaid`: renders Mermaid source with `mmdc`, then opens the resulting
  image as a Canvas surface.
- `canvas_reconcile_intent`: inspects the live document, deterministically
  computes the validated state change for an already-resolved workspace intent,
  and applies it with optimistic revision checks. A dry run returns the same
  reconciliation without changing Canvas.
- `canvas_view_patch`: atomically changes the normalized viewport, visible
  attention, source position, navigation state, and typed parameters of a
  mounted image or PDF view. Use `canvas_navigate` first when changing the
  rasterized PDF page.
- `canvas_screenshot`: captures an exact Canvas surface or screen region for
  agent vision and returns its resolved bounds and any mounted view metadata.
- `canvas_view_capture`: captures an exact mounted shared-view revision. It
  rejects hidden, unmounted, stale, or replaced views instead of returning an
  image from a different visual state.

MCP input schemas are generated from the Rust document and patch types at build
time using Schemars. There is no separately maintained JSON dialect. Operations
cover surfaces, splits, stacks, freeform placement, focus, scene nodes, and typed
data bindings. They do not execute Lua, shell commands, or launch applications.
Existing renderer limitations remain; a valid scene is not a promise that every
node type is rendered by every backend. File paths accepted by these tools are
validated again by the Canvas runtime and remain local to the Canvas host.

## Build and connect

```sh
cargo build -p dock-canvas --features mcp --bin dock-canvas --bin dock-canvas-mcp
```

Start the matching `dock-canvas` GUI on the intended desktop, then configure the
MCP client to run the absolute path to `dock-canvas-mcp` with no arguments over
stdio. Run `scripts/setup-mac-canvas-mcp.py --canvas PATH --adapter PATH` to
atomically install the matching pair, register the `dock-canvas` connector, and
start the Canvas LaunchAgent. Do not enable a remote connector until its
presentation access is authorized. This adapter does not itself enroll the device
or expose a public port. File-opening tools address paths on the Canvas host; the
socket remains owner-local and the runtime rejects unsafe or unsupported content.

The runtime and adapter must run as the same OS user. macOS uses the existing
`~/Library/Application Support/Dock/canvas/control.sock`; Linux uses
`$XDG_RUNTIME_DIR/dock/canvas/control.sock`. Socket permissions, ownership and
peer credentials are checked by the existing control client. Windows awaits its
native local control transport; this is not advertised as Windows-ready.

The new `read_document` control command requires the matching runtime build.
Do not replace Dock Control or its permission-bearing app to install this adapter.

## Read, resolve, apply

For direct document and patch tools:

1. Inspect to obtain exact workspace/surface IDs and current content.
2. Construct a document or patch with the existing Canvas schema.
3. Supply `expected_revision` from the returned runtime snapshot. For patches,
   also use the document's `revision` as `base_revision`, with the next document
   revision in `revision`. These are different counters.
4. Read the returned receipt. Runtime errors set MCP `isError` and retain the
   native error code and current revision. On conflict inspect again.

For high-level presentation intent, resolve the request to exact project,
resource, surface and binding IDs, then call `canvas_reconcile_intent`. The tool
holds one adapter slot while it reads the current document, computes either no
change, a minimal patch, or an exact project replacement, and applies the result
at the runtime revision it observed. Preserve mode leaves unmentioned human-opened
surfaces in place. Exact mode removes unmentioned state and requires an explicit
layout. A project switch also requires exact mode. The returned ordered change
list makes the decision inspectable and machine-readable.

`request_id` is a bounded correlation ID, not an idempotency key. A socket timeout
or disconnected client does not prove an edit was cancelled. Inspect before any
retry. Each adapter admits one call at a time and retains its slot until socket
I/O finishes, even when the caller disconnects. Canvas itself arbitrates edits
from multiple adapters and direct human input with revision validation.

## Voice input boundary

The intended path is transcription -> bounded LLM-assisted context and exact-ID
resolution -> desired workspace state -> deterministic reconciliation against the
live document -> revision-checked socket mutation -> receipt -> visible result.
The model identifies the requested project and authorized assets. It does not
decide which existing surfaces to keep, remove or update; the reconciler derives
those changes from current and desired state. Fine-tuning may later produce the
same typed desired state; it must not bypass validation or authorization.
Voice capture, training, durable command deduplication, tool enrollment, and live
Mac visual qualification are not implemented by this MCP adapter.

## Shared agent vision

An agent opens an image or PDF with `canvas_open_file`, then reads the returned
surface and shared-view state with `canvas_inspect`. It can use
`canvas_navigate` to select a PDF page and `canvas_view_patch` to set a normalized
viewport and attention region. Both changes are immediately visible on Canvas.
The patch receipt returns the new view revision. Supplying that revision to
`canvas_view_capture` returns two MCP content blocks: JSON metadata identifying
the Canvas revision, absolute surface bounds, full shared view, source positions,
page, and viewport; then the PNG the agent can inspect. A human and an agent
therefore refer to the same bounded visual state rather than maintaining separate
copies of a document view.

Surface capture also verifies the three existing Canvas window controls at the
expected pixel positions using the runtime's current theme colors. If macOS or a
Wayland compositor returns an occluding window, another desktop, or bare
wallpaper, the tool returns an error instead of attaching that PNG to the shared
view revision.

The first capture backend reads the composited native Canvas surface using
macOS `screencapture` or Wayland `grim`. Rendering the same view into an offscreen
GPU target is the next backend step for display-independent and headless use;
the shared-view and MCP contracts do not depend on either capture mechanism.

Tracking: `dock-cov.2`, `dock-2kf.9.10`, `dock-q26.10`.
