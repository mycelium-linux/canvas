# Alfalfa project orchestration MCP

Dock exposes a local stdio MCP server for cross-project presentation:

```sh
dock-mac mcp-serve
```

The server reads project and resource identity from the authenticated Alfalfa
owner. It does not copy Alfalfa definitions, return source context, expose
resource URIs, or grant agent control of a project desktop.

The tools are:

- `displays_list`: read live display IDs, names, geometry and primary status
  from Dock Control on this host. Does not infer other machines or user presence.
- `projects_list`: return current project IDs, display names, lifecycle status,
  and purpose.
- `project_resources`: return resource IDs, titles, kinds, and their presentation
  destination for one exact project.
- `project_recall`: recall an exact project on Agora, focus the viewer on the
  output that owns it, and activate any saved local Mac window bindings through
  the same layout operation as the GUI and CLI. Recall is presentation-only: it
  may open a viewer, but never seeds a browser, opens a project resource, stops
  applications, migrates compute, or changes MCP bindings. Empty workspaces stay
  empty. The result reports `presentation_only: true`, `browser_seeded: false`,
  `local_workspace_presented` and `local_window_count`; an unbound
  project remains a valid remote-only recall.
- `resource_open`: open an exact HTTPS resource in the Agora project browser,
  or delegate a local document and owner-only resource to the existing safe Mac
  launcher.

Spoken-name resolution stays deterministic: the caller lists projects, matches
one returned name, and passes its exact `project_id`. Dock rejects unknown IDs,
resources from another project, extra arguments, embedded URL credentials, and
ambiguous guesses.

The manual runbook uses the same implementation:

```sh
dock-mac orchestrate list
dock-mac orchestrate displays
dock-mac orchestrate resources pedalkernel
dock-mac orchestrate recall pedalkernel
dock-mac orchestrate open pedalkernel code
```

An MCP client should register the installed executable with `mcp-serve` as its
only argument. The process inherits `DOCK_DESKTOP_BRIDGE_BIN` when the owner CLI
is not already on `PATH`. JARVIS Sound can expose these operations in its
tier-one tool menu; that adapter should call this MCP contract and keep JARVIS's
existing voice and presence authorization rather than adding another identity
system to Dock.

Concurrent presentation mutations within one MCP server process fail busy rather
than interleave. This is not a cross-process transaction or rollback guarantee:
independent Dock instances and GUI operations still need shared coordination.
A remote presentation can succeed before a later local placement fails; an error
must not be interpreted as "nothing changed".

## Desktop Bridge Boundary

The bridge now supports owner-local `mcp-stdio` connectors as well as HTTP MCP.
Dock can use this transport without an unauthenticated HTTP wrapper. Each client
gets a lifecycle-managed child with configured executable/arguments and no
inherited credential environment. Deployment and connector enrollment remain
separate. The bridge owns transport adaptation, process lifetime and authorization;
Dock owns presentation and Alfalfa project resolution. Alfalfa remains the owner
of project records and resources, not a catalog duplicated in the bridge.

Alfalfa's server already exposes its bridge contract at `/mcp/tools` and
`/mcp/tools/{tool}`. The existing `fpl-mcp/src/alfalfa.rs` adapter translates that
contract into MCP and forwards verified claims; reuse it rather than treating
these REST endpoints as Streamable HTTP or implementing another Alfalfa proxy.
Dock presentation authority is distinct from Alfalfa repository/tool grants.

Dock invokes the owner's `project-read` CLI for Alfalfa reads. On Unix this now
uses the persistent broker and its configuration-bound credential cache, without
per-CLI Keychain reads or anonymous fallback on broker failure. The broker checks
the configured Alfalfa port and retains exact route allowlists, response bounds,
and authorization denials. Owner tokens never reach Dock. Configuration and
verification live in `shared-infra/docs/desktop-bridge/dock-stdio.md`.

Dock does not accept Firefox terms on the owner's behalf. Mozilla's managed
[`SkipTermsOfUse` policy](https://mozilla.github.io/policy-templates/#skiptermsofuse)
carries an explicit representation of authority to accept for every browser
user, so enabling that policy is a separate owner decision rather than part of
project recall.
