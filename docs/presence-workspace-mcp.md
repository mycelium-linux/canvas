# Presence-aware workspace MCP

Dock exposes one orchestration catalog through stdio and through an authenticated
loopback Streamable HTTP listener. FPL Desktop Bridge owns device identity,
remote routing, and capability advertisement. Its connector capability ID is
`dock.workspace-mcp`, which the hosted FPL MCP bridge publishes as
`desktop.dock.workspace-mcp` for the authenticated owner and device.

The MCP tools are deliberately narrower than general desktop control:

| Tool | Effect |
| --- | --- |
| `projects_list` | Lists safe Alfalfa-owned project identities. |
| `project_resources` | Lists safe resource metadata for an exact project. |
| `workspaces_list` | Lists exact projects and saved local window counts, without titles. |
| `displays_list` | Lists displays attached to this Dock host. |
| `presence_route` | Reports whether current identified presence maps to one display. |
| `workspace_activate` | Arranges an exact saved Mac workspace on `primary` or `presence`. |
| `project_recall` | Presents Agora and saved Mac surfaces for an exact project. |
| `resource_open` | Explicitly opens one exact resource. |

Presentation calls share one process-local mutation gate. A separate host-wide
coordinator remains tracked because GUI and independent MCP processes can still
race. Recall presents existing work; it does not start applications, stop work,
migrate compute, or silently open a project resource.

## JARVIS Presence contract

JARVIS owns sensing and identity fusion. Dock reads its public version 1 atomic
publication files and owns only the route from `(producer_id, zone)` to a stable
Dock Control display ID. The private Dock policy lives at:

```text
~/Library/Application Support/Dock/presence-routing.json
```

Example:

```json
{
  "schema_version": 1,
  "person_id": "ajmwagar",
  "max_age_ms": 5000,
  "min_confidence": 0.7,
  "sources": [
    "/Users/ajmwagar/src/fpl/jarvis/.jarvis/presence/current.json"
  ],
  "routes": [
    {
      "producer_id": "presence-local",
      "zone": "bedroom",
      "display_id": "D330FEB7-0CA7-42F7-9BCF-10EFBC8BEB61"
    }
  ]
}
```

Dock accepts only regular owner-controlled files of at most 1 MiB. It requires
occupied presence, the configured person, policy confidence, and combined
sensor/publication age below the configured maximum. An unknown person, stale
sample, future timestamp, missing route, disconnected display, or simultaneous
presence on different routed displays fails rather than choosing a fallback.
Multiple physical screens in one JARVIS zone need distinct producer/zone
observations before Dock can distinguish them; room presence alone does not
infer gaze or screen attention.

Presence selects placement only. JARVIS must still recognize and authorize the
current speaker before invoking a mutating workspace tool. A face observation
from the room never promotes an unknown voice to an authorized speaker.

## Mac installation and runbook

Build on a volume with adequate headroom, then install the exact binary and
connector:

```sh
CARGO_TARGET_DIR=/Volumes/DockBuild/target \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo build -p dock-mac
python3 scripts/setup-mac-workspace-mcp.py \
  --source-binary /Volumes/DockBuild/target/debug/dock-mac
```

The setup writes a random 64-hex bearer to the owner-only Dock support
directory, installs `dev.fpl.dock-mcp`, updates only the managed connector in
FPL Desktop Bridge's configuration, and restarts the existing bridge. The
connector gives the bridge the token file path; no token is placed in arguments,
logs, or project configuration.

Manual checks:

```sh
dock-mac orchestrate displays
dock-mac orchestrate workspaces
dock-mac orchestrate presence
fpl-desktop-bridge verify --connector dock-workspaces --local
fpl-desktop-bridge verify --connector desktop.dock.workspace-mcp
dock-mac orchestrate activate alfalfa presence
```

To recover manually, stop `dev.fpl.dock-mcp`, remove only the connector whose
ID is `dock-workspaces`, and restart `dev.fpl.desktop-bridge`. Preserve the token
unless intentionally revoking the connector; deleting it causes Dock MCP to
fail loudly on its next start.
