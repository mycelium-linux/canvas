# Project startup through existing owners

Alfalfa owns project identity, shared context and bulletin-board resources.
Edgerunner/OpenCode owns agent conversations, model selection and run-scoped
Bifrost/MCP configuration. The FPL desktop bridge and fpl-auth own enrollment and
credentials. Canvas recalls the desktop and consumes those interfaces.

The Rust Alfalfa adapter reads the existing local service at
`http://127.0.0.1:8791` (override its port with `DOCK_ALFALFA_PORT`). It neither
starts an Alfalfa daemon nor creates a project. Project recall fetches the exact
matching project briefing alongside desktop recall. Missing context does not
prevent access to the existing desktop. The diagnostic view reports whether
context was attached; the normal interface gains no setup buttons.

```sh
canvas-mac alfalfa list
canvas-mac alfalfa context alfalfa
```

Cross-project callers use the owner-backed orchestration tools documented in
[`alfalfa-mcp.md`](alfalfa-mcp.md). These tools list exact Alfalfa identities,
recall a project workspace, and open a selected resource without returning the
owner's source context or credentials.

The context response preserves Alfalfa's source content and resource links,
including board links when the owner supplies them. It is not a new messaging
API, and Canvas does not automatically post the briefing into an agent conversation.
The local running owner returned the actual `alfalfa` project, seven resources
and four context sources during validation. Canvas was then registered through the
owner's project-editor API with its source workspace, Beads tracker and startup
ownership note. A fresh read returned project `dock`, three resources and two
context sources. Studio remains a demo with no Alfalfa project; its identity is
not silently mapped to another project. No agent run or board message was created.

Hosted project access requires the owner's authenticated client interface; the
local adapter deliberately accepts only a loopback port and does not construct
identity headers. Browser SSO and bridge enrollment are separate credentials.
Completing the bridge's legitimate pairing flow in the persistent Agora browser
can establish that browser's SSO session while enrolling the bridge. Approving
on the Mac alone does not sign the Agora browser in.

Edgerunner already has run-scoped Bifrost leases and MCP configuration. A model
name in session metadata is not an authenticated readiness probe. Local desktop
Bifrost SSO issuance and full owner-backed terminal attachment remain incomplete;
neither is replaced by copying static keys into Canvas.
