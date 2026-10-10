# Authoring a project scene

Canvas owns the schema. Inspect maintained sources in the Canvas checkout:

- `crates/canvas-workspace/src/lib.rs`: documents, surfaces, layout and patches.
- `crates/canvas-widgets/src/` and `config/widgets/`: kinds and reusable specimens.
- `config/binders/`: bounded Lua adapters and their tests.
- `crates/canvas/src/control.rs`: desktop owner commands/receipts.
- `crates/canvas-workspace/src/endpoint.rs`: presentation/TV owner commands.

Compose a fitting specimen into one document. Use stable IDs: `project_id` is
the project; `id` is its workspace. An import can create a receiving project
without copying host execution resources. `theme.source=project` inherits its
local project theme; use a supported explicit theme for identical styling.

Create a workspace with a revision-bound owner JSON import:

```json
{
  "schema_version": 1,
  "request_id": "project-scene-import-1",
  "expected_revision": 42,
  "command": {
    "command": "workspace_import",
    "activate": false,
    "document": {
      "schema_version": 1,
      "id": "project-workspace",
      "project_id": "project-work",
      "revision": 1,
      "theme": { "source": "project" },
      "surfaces": [{ "id": "clock", "title": "Local time", "content": {
        "kind": "widget", "spec": {
          "schema_version": 2, "id": "clock", "kind": "clock", "mode": "analog"
        }
      }}],
      "layout": { "kind": "surface", "surface_id": "clock" }
    }
  }
}
```

`42` is illustrative: inspect that endpoint for its real revision. Send JSON
through the owner CLI, not by editing private state. Current desktop import
parks an already-selected project when `activate=false`: preserve selection by
setting it true in that case. The fleet planner does this automatically.

Edit with the current document generation and the owner's revision-bound patch
API. Do not invent revisions to defeat conflicts. For progress, add a matching
existing GFX/widget specimen, declared input contract and source. Reuse telemetry
ingress/Lua binders; verify live updates and freshness. Syncing definitions does
not create publishers, transfer local PDFs or native GPU buffers.

All source URLs/files must be reachable on each screen. No credentials, private
transcripts or bearer URLs in portable documents. Keep sources behind existing
ACL-bound adapters. Test binders/widgets through their owners. Respect transport
size bounds; never truncate a scene to fit.
