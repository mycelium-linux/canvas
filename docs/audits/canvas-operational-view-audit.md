# Canvas operational view audit

Date: 2026-09-08

This is a current working-tree product audit, not a revision-certified maintenance receipt. Dock, Elmer's, and shared-infra all had uncommitted changes during inspection, so an immutable baseline was unavailable. The source paths below make every finding reproducible without pretending the trees were clean.

## Decision

Canvas should become the default read-only operational surface for FPL systems. Grafana remains useful as a query/debugging fallback while Canvas gains coverage. Source applications continue to own mutations such as approvals, billing, device enrollment, CRM edits, workflow execution, and incident acknowledgement.

A saved operational view is a named composition of widgets, source bindings, layout, theme, and freshness policy. It belongs to a project but is not a workspace. This audit does not change workspace selection, MCP activation, or persistence contracts.

## Existing dashboard inventory

| Owner | Evidence | Current content | Canvas direction |
|---|---|---|---|
| Elmer's | `../elmers/deploy/grafana/dashboards/elmers-production-health.json` | 17 panels covering sidecar/scrape health, in-flight runs, durable workflow and capability outcomes, loop errors, model/token usage, budget headroom, repo drift, approvals, schedules, and timers | Replace the routine read-only view with `elmers/now`, `elmers/workflows`, and `elmers/agent-economics`; keep Prometheus/Grafana for query diagnosis during migration. |
| Elmer's application | `../elmers/src/ui.html` | Now, approvals, machines, live Brain, capabilities, workflows, campaigns, policy, and ops | Canvas owns Now, Brain visualization, and read-only workflow health. Elmer's keeps approvals, tick/action controls, workflow execution, campaign mutation, and policy editing. |
| Bifrost runtime | `../shared-infra/batty/mon/grafana-dashboards/bifrost-runtime.json` | 12 panels for backend health, advertised models, queues, completions, token rates, KV cache, TTFT, end-to-end latency, HTTP traffic, prefix cache, paid calls, and fallback | Make `bifrost/runtime` the first production-grade Canvas operations view. It exercises stats, time series, thresholds, costs, and latency distributions without domain-specific UI. |
| FPL Auth | `../shared-infra/batty/mon/grafana-dashboards/fpl-auth.json` | 6 panels for SSO health, sessions, OIDC tokens, magic links, login/vouch outcomes, and OIDC endpoint outcomes | Add `auth/access-health`; keep account, company, connection, billing, and skill mutations in fpl-auth. Never put secrets or token bodies in Canvas state. |
| Invoker | `../invoker/ops/monitoring/grafana/invoker.json` | 8 panels for scrape health, configured functions, request/error rates, traffic, duration, cold starts, and function count | Add `invoker/traffic` and an alert-focused compact variant. |
| ntfy | `../ntfy/examples/grafana-dashboard/ntfy-grafana.json` | Example dashboard with publish/cache/visitor/topic/subscriber, HTTP, attachment, Firebase, UnifiedPush, Matrix, email, latency, goroutine, file descriptor, and memory panels | Treat as reference until an FPL deployment is confirmed. If adopted, expose `ntfy/service-health`; notification presentation itself is a separate Canvas capability. |
| Bifrost repository | `../bifrost/grafana/provisioning/datasources/prometheus.yml` | Prometheus datasource provisioning only | Reuse the owner endpoint through the Canvas Prometheus connector. Do not duplicate query transport in widgets or Lua. |
| ohmOS | `../ohmos` (`axum-prometheus` and `/metrics`) | Provisioning and device operations with a Prometheus endpoint, no audited Grafana dashboard | Add `ohmos/fleet` after exact metrics are catalogued; preserve onboarding, passkey, and device mutations in ohmOS. |
| The Lab Tracker | `../the-lab-tracker/frontend/src/components/Dashboard.tsx` | Active leads, follow-ups, jobs, monthly revenue, setter cuts, funnel, industry mix, revenue breakdown, overdue follow-ups, and today's tasks | Add `lab/morning-brief`, `lab/pipeline`, `lab/production`, and `lab/cash`; keep lead/task edits in the tracker. |
| Panopticon | `../panopticon/strategic-ui/src/App.svelte` and its map components | Map/screening modes, domains, entities, radius, filters, freshness, saved operator view, overlays, and command-region shortcuts | Panopticon owns intelligence, geography, and query state. Canvas presents `panopticon/world-watch`, `panopticon/watchlist`, and trip/navigation views through the existing native map bead. |
| JARVIS | `../jarvis/src/sound/status.rs` | Voice status already publishes into Canvas; presence and announcements are owner concerns | Add `jarvis/presence` for diagnosis and a global notification/announcement surface. JARVIS decides what to announce; Canvas owns visual delivery and history. |

## Named view catalog

The adjacent `canvas-operational-view-inventory.json` is the machine-readable audit proposal. The highest-value first views are:

1. `bifrost/runtime`: health, queues, throughput, latency, cache, token rate, fallbacks, and cost.
2. `elmers/now`: current work, attention, recent outcomes, loop health, and budget.
3. `elmers/brain`: live topology and firing from the Elmer's and MCP SSE contracts.
4. `auth/access-health`: authentication availability and outcome rates without personal or secret data.
5. `lab/morning-brief`: leads, follow-ups, production, cash, and tasks in one glance.
6. `jarvis/presence`: microphone, camera/presence, voice loop, last event, and connector freshness.
7. `panopticon/world-watch`: map, active domains, selected entities, freshness, legend, and alert rail.

Names should be stable project-local identifiers. Display names can change with the theme. A preset refers to connector and query identities; it does not contain endpoints, credentials, or copied source data.

## Widget coverage and gaps

Canvas currently implements quant/candlestick, stat, time series, clock, ticker, gauge, LED/VU meter, histogram, table, event log, node summary, status, text, external chart, and Lua module kinds in `crates/canvas-widgets/src/lib.rs`. That is enough to prove transport and layout, but not enough to replace mature operational dashboards.

| Family | Required behavior | Current gap |
|---|---|---|
| Shared widget frame | Optional title/subtitle, source, time window, freshness, health, units, quiet drag target, density, and loading/no-data/stale/partial/error states | Each renderer builds a slightly different sparse header; source and freshness are absent. |
| Stat / KPI | Value, unit, semantic state, delta with domain-aware polarity, sparkline, threshold, window, and comparison label | Delta color assumes positive is good; history has no time/window context. |
| Time series | Axes, ticks, grid, legend, units, time window, thresholds, annotations, hover/crosshair, missing-data gaps, and downsampling | Current line plot has a minimal legend and grid but no readable axes or interaction. |
| Distribution | Histogram, heatmap, quantiles, bucket labels, threshold markers, and exemplar/detail handoff | Current histogram drops bucket bounds and lacks axes; heatmap is absent. |
| Categorical comparison | Ranked bars, grouped/stacked bars and area, compact donut only when part-to-whole is the actual question | Missing; tables or time series currently carry the load. |
| State over time | State timeline, uptime/SLO, error-budget and burn-rate treatments | Missing. |
| Events and notifications | Severity rail, timestamp, source, grouping, unread/announced state, deduplication, actions/deep links, and bounded history | Event log is visual only and lacks delivery semantics. |
| Table | Column types/alignment, status cells, sorting, density, truncation, scrolling, and row selection/deep links | Current table gives every cell equal width and truncates after eight rows. |
| Topology / DAG | Edges, grouping, state/activity animation, latency, selection, legend, replay/hold, and provenance | Node summary is only a list. Retained graph work already has dedicated beads. |
| Map / navigation | Native GPU map, layers, legend, route, waypoints, weather/tide/current overlays, scale, bearing, GPS/entity follow, and trip summary | Typed viewport backend exists; native renderer and trip widgets remain. |
| Contact / personnel file | Portrait, preferred name/pronouns, roles, organizations, safe contact methods, relationship context, recent verified facts, provenance, freshness, and deep links | New widget family. Sensitive fields need explicit source policy and redaction. |
| Media / engineering | Image/PDF viewport, video, terminal, 3D, oscilloscope/spectrum, and generated explanatory animation | Artifact groundwork exists; oscilloscope/music and Maxim need explicit future work. |

## Visual quality bar

Every operational widget should render through one Coherence-owned `WidgetFrame` and one plot/layout toolkit rather than inventing its own card. The frame has compact and comfortable density modes, optional header and chrome, a stable inner plot rectangle, tabular numerals, and semantic surface/text/border/data-series roles. Series colors derive deterministically from the active project palette and retain accessible contrast in light and dark themes.

Layouts should answer an operational question at a glance. A large panel earns its space with a plot, map, topology, or dense table. Single numbers stay compact. Legends identify color and shape, axes show units and time, and every live view exposes source freshness. Motion communicates a new event or state transition; it does not decorate idle dashboards.

All primitives must show loading, no-data, stale, partial, disconnected, and error states explicitly. Live inputs are bounded and time-aware. A renderer may downsample for pixels, but retained source state and the receipt must make that decision visible. Hover and selection can expose detail, while the default screen remains useful from several feet away.

## Migration sequence

1. Finish the shared frame, plot scaffolding, typed display states, and responsive layout behavior across the existing widget suite.
2. Add the missing high-signal primitives: ranked/stacked bars, heatmap/distribution, state timeline/SLO, alert list, and topology/DAG.
3. Implement `bifrost/runtime` and `elmers/now` as real source-backed presets. Run them beside Grafana until coverage, freshness, and failure behavior match.
4. Add Auth, Invoker, Lab, JARVIS, and Panopticon view packs. Keep mutation workflows as deep links to their owning apps.
5. Retire a Grafana dashboard only after its Canvas preset has owner-source parity, visible stale/error behavior, bounded history, restart restoration, and a documented Grafana fallback/query runbook.

## Audit limits

This pass inspected explicit Grafana JSON/provisioning and representative application dashboards under `/Users/ajmwagar/src/fpl`. It did not claim every page in every vendored or third-party UI. ntfy and EspoCRM contain substantial upstream UI material; only FPL-owned operational questions should drive Canvas work. Exact PromQL, access policy, retention, and alert ownership must be verified per source while implementing each preset.
