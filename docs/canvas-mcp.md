# Canvas MCP

Canvas owns the document, validation and native presentation. MCP is an adapter
over its existing owner-local control socket, not another renderer or command
engine. This first slice exposes eight tools:

- `canvas_inspect`: atomically returns the document and runtime snapshot.
- `canvas_apply_document`: validates and replaces presentation intent.
- `canvas_apply_patch`: applies the existing atomic document patch operations.
- `canvas_close_surface`: removes presentation without stopping background work.
- `canvas_focus`: brings the Canvas window forward.
- `canvas_navigate`: changes pages in an already-open PDF surface.
- `canvas_open_file`: opens an image, PDF, or Markdown file through the native
  content loader.
- `canvas_mermaid`: renders Mermaid source with `mmdc`, then opens the resulting
  image as a Canvas surface.

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

1. Inspect to obtain exact workspace/surface IDs and current content.
2. Construct a document or patch with the existing Canvas schema.
3. Supply `expected_revision` from the returned runtime snapshot. For patches,
   also use the document's `revision` as `base_revision`, with the next document
   revision in `revision`. These are different counters.
4. Read the returned receipt. Runtime errors set MCP `isError` and retain the
   native error code and current revision. On conflict inspect again.

`request_id` is a bounded correlation ID, not an idempotency key. A socket timeout
or disconnected client does not prove an edit was cancelled. Inspect before any
retry. Each adapter admits one call at a time and retains its slot until socket
I/O finishes, even when the caller disconnects. Canvas itself arbitrates edits
from multiple adapters and direct human input with revision validation.

## Voice input boundary

The intended path is transcription -> context and exact-ID resolution -> typed
Canvas operation -> validation -> receipt -> visible result. Fine-tuning may later
produce the same typed operations; it must not bypass validation or authorization.
Voice capture, training, durable command deduplication, tool enrollment, and live
Mac visual qualification are not implemented by this MCP adapter.

Tracking: `dock-2kf.9.10`.
