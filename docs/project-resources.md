# Notes and dashboards within reach

The default Dock window groups Alfalfa projects into company folders. A business
can use the same project model: its notes, dashboards and other tools remain
owned by their existing applications. Dock is the shortcut to them.

Run `dock-mac`, summon it with Ctrl+Option+Space, and choose a company folder, then a project (type to filter each level). Enter switches into that project's resource context. Notes and dashboards
can then be chosen directly or filtered by title. Included notes preview as
formatted Markdown. Enter opens web resources, project homes and supported local
documents. Escape returns to projects, then companies, then hides the launcher. Command-R
refreshes the owner catalog, and summoning after 30 seconds refreshes it in the
background. Every summon returns to company selection. This switches resource
context; it does not yet recall every native/remote window or attach agent sessions.

My Agents observations are merged automatically from the same local Alfalfa owner.
Registered repositories match using their full owner/repository identity, including
SSH/HTTPS normalization; local repository resources match the collector's canonical
path hash. Same names alone never merge. Unregistered workspaces appear as Observed
with agent counts and a link to My Agents. These are replaceable personal observations,
not new shared project definitions. Unavailable or stale activity is shown explicitly.

Paused and archived projects are hidden by default. Command-Shift-A includes them
without deleting their notes or associations. An observed agent never reactivates
an archived project. The current Alfalfa writer supports active/paused/archived;
the proposed Idea/Building/Live/Maintaining/Retired phases and durable promotion of
observed workspaces are owner work tracked in `dock-96j.29`, not implemented status
controls in this launcher.

Resources are read from the existing local Alfalfa service. Add or edit their
titles, links and included note content in Alfalfa's project editor; Dock does not
create a second resource catalog. Each project also has a result for its Alfalfa
home. Notes are rendered only from the owner's explicitly included context
sources. Other file paths and custom schemes open the project home when no safe
document/web target is available. This first pass does not control individual
browser tabs or guarantee that reopening a URL reuses an existing tab.

The registered catalog includes Alfalfa and Dock. Other currently observed workspaces
come from the changing My Agents feed. Declared business dashboard URLs become
available inside their project as soon as they are added in Alfalfa.

```sh
dock-mac resources
dock-mac resources dock notes
dock-mac --conversation
dock-mac --connect-project studio
```

The last two commands retain the earlier conversation/desktop workflow. Project
colors come from the existing Coherence catalog when the identities match;
unmapped owner projects use Coherence's dark brand theme.

`DOCK_ALFALFA_PORT` selects another local owner port. `DOCK_RESOURCE_QUERY` seeds
the initial query, and `DOCK_SCREENSHOT=/absolute/path.png` captures the actual
iced window after owner loading, then exits. No fixture data or model calls are
used by screenshot mode.

Company folders use Alfalfa's `company_id` and the authenticated FPL company
membership list. Personal companies use their existing canonical `personal-*`
IDs. Projects without a company and unregistered observations stay in Unassigned;
Dock never derives ownership from a login name, GitHub account, or billing account.
Equal company names remain separate folders, distinguished by their canonical IDs.
Folders omit inactive projects unless Command-Shift-A is enabled. A removed
membership closes the corresponding folder; it never reclassifies its projects as
Unassigned. Grouping does not itself grant project sharing permissions.

Authenticated reads use the installed FPL Desktop Bridge credential owner:
`fpl-desktop-bridge project-read companies --port 8791` (also `list`, `activity`,
and `context PROJECT`). Set `DOCK_DESKTOP_BRIDGE_BIN` if the owner executable is
not on PATH. The owner reads its OS credential store and sends the bearer directly
to the local Alfalfa service. Dock receives bounded JSON only. Requests use fixed
read routes on loopback, bypass proxy settings, and never follow redirects.

If the owner executable or enrollment is absent, legacy local projects remain
available in Unassigned and company membership shows an explicit unavailable
state. Authorization denial never falls back to unauthenticated reads and clears
the launcher cache. Once authenticated reads have succeeded in the process,
credential loss also fails closed. Retry with Command-R after repairing enrollment.
No browser cookies, fabricated identity headers, or tokens are copied into Dock.
