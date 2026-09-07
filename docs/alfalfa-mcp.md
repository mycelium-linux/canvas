# Alfalfa project orchestration MCP

Dock exposes a local stdio MCP server for cross-project presentation:

```sh
dock-mac mcp-serve
```

The server reads project and resource identity from the authenticated Alfalfa
owner. It does not copy Alfalfa definitions, return source context, expose
resource URIs, or grant agent control of a project desktop.

The tools are:

- `projects_list`: return current project IDs, display names, lifecycle status,
  and purpose.
- `project_resources`: return resource IDs, titles, kinds, and their presentation
  destination for one exact project.
- `project_recall`: recall an exact project on Agora and focus the viewer on the
  output that owns it. When the new workspace has no windows, Dock opens its
  first safe HTTPS Alfalfa resource in a project-isolated browser profile. The
  result reports whether Firefox terms have been accepted for that profile and
  sets `attention_required` when the owner must finish that one-time step.
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
dock-mac orchestrate resources pedalkernel
dock-mac orchestrate recall pedalkernel
dock-mac orchestrate open pedalkernel code
```

An MCP client should register the installed executable with `mcp-serve` as its
only argument. The process inherits `DOCK_DESKTOP_BRIDGE_BIN` when the owner CLI
is not already on `PATH`. JARVIS Sound can expose the same four operations in its
tier-one tool menu; that adapter should call this MCP contract and keep JARVIS's
existing voice and presence authorization rather than adding another identity
system to Dock.

Dock does not accept Firefox terms on the owner's behalf. Mozilla's managed
[`SkipTermsOfUse` policy](https://mozilla.github.io/policy-templates/#skiptermsofuse)
carries an explicit representation of authority to accept for every browser
user, so enabling that policy is a separate owner decision rather than part of
project recall.
