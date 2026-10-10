# Explicit workspace sync

Inspect `canvas-fleet status --json` and installed command support. The local
operator registry is `~/.config/unibus/canvas-fleet.json` (or the XDG config
directory). Discovery is not a grant to read/import a workspace.

For cross-platform typed geometry, configure the registry's optional
`document_codec` with the absolute installed path to `canvas-workspace-ctl`.
Canvas owns normalization of its document model; without the codec, comparisons
remain exact JSON. Do not add blanket numeric tolerances to hide conflicts.

```sh
canvas-fleet sync neo project-work studio,firetv --dry-run
canvas-fleet sync neo project-work studio,firetv
canvas-fleet sync neo project-work studio,firetv --activate
```

Use `--config PATH` for another registry. Source must be one endpoint;
workspace is its project ID, not document ID. Default sync preserves receiver
selection; `--activate` presents it. Groups are selections, not owners.

Source is read once. Destinations inspect endpoint revisions and document
generations. Newer or equal-but-different generations conflict. Identical scenes
need no document write. Imports are per-screen, not a distributed transaction.
Read-back verifies retention, not rendering support. Report partial failures.
Current owner-CLI sync bounds each document to 48 KiB and reports larger scenes
as errors rather than trimming them.

No full-catalog `install` fallback: it erases unrelated FireTV menus. No force,
rebase or automatic retry of uncertain writes. TV needs incremental import
deployed first; gateway read/import requires its own advertised granted operations.

A bounded companion can publish verified edits and rerun explicit sync after
milestones. Use existing telemetry for live values. This CLI is not continuous
multi-writer replication, media fanout, or an Iroh deployment test. Keep one
document writer per generation.
