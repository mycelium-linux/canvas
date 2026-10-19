# Native Canvas work inbox

`canvas-marbles` is a read-only Marbles presentation producer for the current
shared Canvas workspace endpoint used by canvas-linux and Android/Fire TV.
It does not launch dock-mac or an iced companion. The renderer owns layout,
theme and native input; Marbles remains the task source of truth.

The default page is a dense status list: task title first, then status,
priority, readable project label and assignment. Completed work sorts last.
An unexpired owner lease is labelled **Claimed**, never inferred to mean
**In progress**. Amber means claimed/in progress/review; green means done;
red means an explicit blocked status. Status text carries the same meaning
without color. The secondary **Task details** page retains the existing
native task decks and D-pad selection, OK expansion and Back collapse.

The initial scope is the `elmers-shared-workspace` cohort across five explicit
trackers. This is not a fleet-wide task search. Capacity is 16 records; an
overflow produces a visible stale/error state instead of silently hiding work.
List row activation, interactive filtering and a synchronized side-by-side
detail pane remain work in `dock-workspace-marbles`; the dense list is read-only.

## Build and validate

The producer has its own Cargo workspace and lockfile so platform GUI/JNI
dependencies are not required:

```sh
cargo check --manifest-path crates/canvas-marbles/Cargo.toml
cargo test --manifest-path crates/canvas-marbles/Cargo.toml
cargo clippy --manifest-path crates/canvas-marbles/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path crates/canvas-marbles/Cargo.toml -- --check
cargo build --manifest-path crates/canvas-marbles/Cargo.toml
```

Configure absolute installed paths in a private JSON file. Display labels are
presentation copy; source tracker and task identities remain unchanged.
`viewer_principal` is optional and must be the verified viewer identity before
the view can label an assignment “You”. No token belongs in this file.

```json
{
  "root": "/Users/operator/src/fpl",
  "marbles": "/Users/operator/.local/bin/mb",
  "trackers": ["elmers", "dock", "research-wing", "unibus", "wiki"],
  "cohort": "elmers-shared-workspace",
  "workspace": "work-inbox",
  "project_labels": {
    "dock": "Canvas", "elmers": "Workflows", "research-wing": "Research",
    "unibus": "Transport", "wiki": "Notes"
  }
}
```

`--once` emits one newline-delimited frame per declared surface and exits
unsuccessfully if the owner read fails or capacity is exceeded. `--watch`
refreshes every 15 seconds and recovers on the next successful read. Reads
are bounded to ten seconds and 4 MiB per tracker. Owner errors never enter
display content; safe diagnostics go to stderr. Cross-tracker responses and
duplicate task identities are rejected before filtering.

## Install one workspace without replacing the receiver's projects

1. Use the receiver's supported authenticated control tool. Inspect its owner,
   active workspace and capabilities. On Fire TV, use an explicit authorized
   ADB serial with `canvas-tvctl`; keep the app-private token inside that helper.
2. Save the receiver's current private checkpoint and retain a rollback copy.
   Fire TV stores it at `files/workspaces.json` under the app's `run-as` identity.
   Checkpoints are private runtime state, not repository fixtures.
3. Generate a revision-checked install request:

   ```sh
   canvas-marbles CONFIG.json --install-request CHECKPOINT.json firetv-omni > REQUEST.json
   canvas-tvctl SERIAL < REQUEST.json
   ```

   This preserves every other project/document and the current selection.
   It adds or updates only `work-inbox`, derives its next document revision,
   and refuses mismatched ownership or an unexpected existing document ID.
   If admission rejects a stale revision, obtain a fresh checkpoint and
   regenerate. Never retry a stale full-catalog write unchanged.
4. Inspect again. Confirm other project documents are unchanged. Select
   **Work inbox** through native navigation, or a revision-checked
   `workspace_switch` request.
5. Connect the producer to the existing transient ingress:

   ```sh
   canvas-marbles CONFIG.json --watch | canvas-tvctl SERIAL signal-stream tv-work-inbox
   ```

   The stream argument is the **document ID**, not the project ID. For a
   remote ADB host, pipe through its authenticated SSH connection. Keep
   addresses and commands operator-configured; never interpolate issue text.

6. Supervise the pipeline with the host's existing service manager. Receiver
   expiry remains authoritative when producer or transport disappears.

The document persists empty/loading frames only. Live task readings and
selection are transient. Native status lists additionally receive a UTC
observation timestamp; all frames retain explicit source and freshness.

## Studio / Neo / Fire TV installation

The first installed receiver is `firetv-omni`, serial `192.168.2.97:5555`,
package `dev.fpl.canvas.tv`, through Neo's existing authorized ADB identity.
All 29 pre-existing projects were retained when adding the inbox. No APK,
signing key, app-data reset or renderer replacement was needed.

Studio owns the producer, installed as `~/.local/bin/canvas-marbles`.
The launchd job is `dev.fpl.canvas-marbles`; configuration and the pre-install
checkpoint are under `~/Library/Application Support/Canvas/work-inbox/`.
Logs are under `~/Library/Logs/CanvasWorkInboxNative/`. The job reconnects
through Neo and never selects the workspace just to deliver an update.
The obsolete `dev.fpl.canvas-work-inbox` Mac companion is disabled.

Verify with independent `read_presentation`, then a real device screenshot.
Test D-pad/OK/Back on the details page and selection across an owner refresh.
Stop the producer briefly and confirm receiver staleness, then restore it.
Do not regard successful JSON admission as proof of readable presentation.

## Design reference and remaining scope

The public Macro task/email screenshots informed feature-level reading order:
dense task names, compact status/ownership and adjacent context. No Macro
implementation or assets are included. Canvas consumes its own native widgets
and resolved Coherence theme. The initial tile layout was rejected by the
user as too sparse; it is retained only as a secondary detail surface.

Runtime qualification here applies to the tested Fire TV receiver. The same
wire contract is implemented by the current shared canvas-linux runtime;
this adapter does not prove a Linux compositor deployment or source parity.
