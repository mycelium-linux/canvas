# New Canvas dashboard in minutes

A dashboard is a revisioned `WorkspaceDocument`: a theme, one canvas layout, and
bounded surfaces. Use the scaffold rather than retyping the envelope:

```sh
S=skills/canvas-live/scripts/new-dashboard.sh
$S --id fleet --project fpl --title "Fleet" --template kpi --out /tmp/fleet.json
$S --id fleet --project fpl --title "Fleet" --template list --out /tmp/fleet.json
$S --id fleet --project fpl --title "Fleet" --template series --out /tmp/fleet.json
$S --id fleet --project fpl --title "Fleet" --template kanban --out /tmp/fleet.json
```

All templates are valid workspace JSON and deliberately start in `no_data`; they
do not fabricate metric values. `--out -` prints to stdout.

Import to a TV through the revision-bound owner path:

```sh
CANVAS_TVCTL=/path/to/canvas-tvctl \
  skills/canvas-live/scripts/import-tv-dashboard.sh 10.0.0.44:5555 /tmp/fleet.json
```

For a desktop Canvas control socket, use the same `workspace_import` request
shape with `canvasctl`; the scaffold is transport-neutral.

## After the scaffold

1. Read the current document with `inspect`/`read_document`; do not invent a
   revision.
2. Replace the placeholder surface with an existing widget specimen from
   `config/widgets/` and the canonical kind from `canvas-widgets`.
3. Attach a declared source/input/binder. The surface is presentation only.
4. Verify with a read-back and one live update. A copied definition is not a
   live feed.
5. Keep the scene in the project repository and commit a small verified diff.

The generated file is revision `1`. The importer reads the destination revision
and wraps the document in `workspace_import`, so it is safe to run when another
agent has advanced the target; a stale inspect revision is rejected and should be
retried, never forced.

## Why this is faster

Hand-authoring a dashboard means recreating the document envelope, placement
rectangles, widget schema version, IDs, and freshness metadata before any data
work. The scaffold removes that boilerplate and the common invalid states. The
remaining cost is the real work: choosing a source, binding it, and verifying it.
