---
name: canvas-live
description: Create and maintain live project workspaces, dashboards, and widgets on Canvas; show ongoing agent work on selected TV or desktop screens and synchronize portable scenes. Use for Canvas presentation requests, not unrelated web dashboards or provider control.
---

# Canvas Live

Make useful work visible without making presentation the main task. Automatic
discovery of this skill is not permission to replace a user's screen or expose
private work.

## Choose the real endpoints

Run `canvas-fleet status --json`. Target by registered endpoint ID, not the
host where the agent happens to run. Neo's catalog does not describe the TV.
Inspect installed command support and the owning repository before using new
APIs. Default checkout here is `~/Documents/src/fpl/dock`; Unibus is its sibling.
Follow their `AGENTS.md` files and their actual schemas/fixtures.

A request such as “show your work on the TV” scopes the screen and content. If
no screen was chosen, retain the scene without activation or ask once. Preserve
unrelated workspaces and menus. Read [authoring.md](references/authoring.md) when
creating/changing scenes, and [sync.md](references/sync.md) before copying them.

## Fast path for a new dashboard

Use the checked-in scaffolds instead of hand-writing the document envelope:

```sh
skills/canvas-live/scripts/new-dashboard.sh   --id my-dashboard --project my-project --title "My dashboard"   --template kpi --out /tmp/my-dashboard.json

CANVAS_TVCTL=/path/to/canvas-tvctl   skills/canvas-live/scripts/import-tv-dashboard.sh 10.0.0.44:5555 /tmp/my-dashboard.json
```

Templates are `kpi`, `list`, `series`, and `kanban`; they start in `no_data`
state rather than inventing values. The import script inspects the target and
uses its revision, so it cannot overwrite a newer scene. See
[dashboard-quickstart.md](references/dashboard-quickstart.md) for the schema,
widget shapes, and the remaining data-binding steps. Once the scaffold is
imported, replace the placeholder surface with a real widget specimen, declared
input contract, and source. Scaffolding is not a live feed.

## Live, reusable scenes

Reuse Canvas Lua composition primitives and widget specimens before writing
Rust UI. Keep the existing analog clock and shared GFX/Coherence chrome where
appropriate. Native GPU/external surfaces suit video and heavy instruments; a
small progress widget should not require rebuilding the compositor.

Providers own data/actions; Unibus owns access/transport; Mycelium owns discovery;
Canvas owns layout, validation and presentation. Lua composes declared typed
inputs, not network clients, credentials or polling daemons. Reuse owner
adapters instead of scraping terminals when structured telemetry exists.

Show verified activity, milestones, tests/build outcomes, blockers and pending
human responses. Distinguish active/idle/waiting/disconnected/unknown using
evidence. Missing is not zero. Include source and freshness; align refresh and
expiry so normal polling does not flash stale. Avoid fake motion implying
traffic/progress, tables when symbolic GFX is clearer, and every-session dumps.

## Keep the main work moving

When delegation is available and allowed, delegate one bounded Canvas companion
while the parent continues implementation. Give it the project, authorized
screen IDs, data contracts, exclusive files, and completion condition. It can
publish verified milestones and sync meaningful document edits; it should not
rediscover state in a busy loop or become a second project orchestrator.

Do not let companions concurrently install APKs, build into one target directory,
overwrite another writer's generation or mutate provider state. Prefer existing
telemetry/watch services for continuous data. Companion work ends with the task;
unattended scheduling/new persistent services require a separate scoped request.
On a failure, retain the last good scene, report it and continue unrelated work.
Do not silently switch screens, defeat generation conflicts, loosen ACLs, or
restart a playing TV to recover.

## Verify and hand off

Use owner validation, read back each named destination document and verify
selection preservation plus an actual live input update. A copied definition
does not prove a working feed or matching renderer. Report unsupported surfaces,
unreachable/local assets and missing bindings.

Keep scenes/binders in their project repositories; use small verified commits
under local policy. Report source workspace, screens, refresh source, sync outcome
and live-data gaps. Do not claim continuous replication or Iroh delivery from
owner-CLI results.
