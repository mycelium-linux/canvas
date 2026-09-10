# The Power On demo: script and honest status

*Written 2026-09-10. Every beat below carries a status, because a script that
cannot be delivered is worse than no script. Status is one of:*

- **REAL** — exists and works today, driving real data.
- **DEMO DATA** — the code path is real; the data is synthetic and marked
  `Provenance::Demo` on screen. This is a legitimate state, not a shortcut.
- **NOT BUILT** — does not exist. Named here so it is scheduled rather than
  discovered on stage.

The rule the whole demo rests on: **it runs the real path.** No `--demo` flag
taking a different code route. Synthetic *data* is marked; synthetic *paths* are
forbidden, because they rot within a month and lie before that.

---

## Act 1 — Power On

| # | Beat | Driven by | Status |
|---|---|---|---|
| 1 | `fpl login`, SSO opens in the browser, binary downloads | fpl-auth | REAL, unmeasured |
| 2 | FPL logo spins, SFX, horizontal line opens out like an old TV | — | **NOT BUILT** |
| 3 | Canvas workspace opens with a transition | `transition.rs` — 280 ms ease-out, palette interpolation per surface | REAL |
| 4 | JARVIS boots, establishes presence and identity | `SetPresence` over the control socket; `presence.rs` renders the occupant rail with face and voice signals | REAL — drivable today |
| 5 | "Good afternoon, Avery." | — | **NOT BUILT** (and see the gate below) |
| 6 | Most recent project / home screen | `Command::WorkspaceSwitch` | REAL |

> **The gate on beat 5.** Greeting by name is an identity claim and it is the
> first thing on screen. `PresenceSignalState` already distinguishes
> `Unavailable`, `Unknown`, `Detected`, `Verified`. The greeting must require
> `Verified` and degrade to a nameless "Good afternoon" otherwise. A system that
> confidently calls someone the wrong name once is never trusted again, and this
> is the most visible possible place for it to happen.

> **The risk on beat 2, and it is the demo's biggest.** The intro sets an
> expectation. If what follows takes forty seconds, the animation makes the demo
> *worse* — Iron Man promised, loading screen delivered. Beat 1 and beat 6 are
> not currently measured. `boot_budget` (`canvas-core`) now exists to measure
> them; wire it before designing the animation, and spend the animation budget
> *covering* real latency rather than adding to it. `dock-ybc.4`.

---

## Act 2 — What Canvas is

Shown as a working surface, not a slide.

| # | Beat | Driven by | Status |
|---|---|---|---|
| 7 | Switch project, watch the workspace re-form | `Command::WorkspaceSwitch`, `project_recall` | REAL |
| 8 | Open a PDF; zoom to one region of one page | `canvas_open_file`; `MediaVariant::PdfPage` | REAL — region crop is **NOT BUILT** (see note) |
| 9 | Open an image, a Markdown doc, a rendered Mermaid diagram | `canvas_open_file` | REAL |
| 10 | Live metrics wall — the fleet view | `canvas-metrics` → Prometheus → `canvas.widget.snapshot.v1` | REAL, live data |
| 11 | Video surface | `SurfaceContent::Stream { url }` | REAL |
| 12 | Snap the layout back with a chord; reflow across displays | layout solver, `solve_axis`/`Tracks` | REAL |

> **Beat 8's crop.** Region-level display is not built as a widget feature — and
> under the artifact-broker design it never needs to be, because a crop is a
> *derived artifact* with a parent and a rectangle in its lineage rather than a
> view parameter. So the demo either shows a whole page, or waits for
> `canvas_crop_artifact` to be served. Do not fake a zoom.

---

## Act 3 — Presence

The part that is genuinely differentiating, and the part with the most careful
line between real and not.

| # | Beat | Driven by | Status |
|---|---|---|---|
| 13 | The occupant rail appears — who is here, face signal, voice signal | `presence.rs`, `PresenceSignal`, `MAX_VISIBLE_OCCUPANTS = 4` | REAL |
| 14 | Verification lands: state moves `Detected` → `Verified`, preview shows | `ShowPresencePreview` | REAL — drivable over the control socket |
| 15 | Step away; sensitive values degrade in place rather than the screen locking | — | **NOT BUILT** — `dock-7pa` |
| 16 | Step back; values restore | — | **NOT BUILT** — `dock-7pa` |

> **Beats 15 and 16 are the ones Avery most wants and they do not exist.** The
> honest position: the *identity axis* is already there — `Detected` means
> someone is present, `Verified` means it is the owner, which is exactly the
> distinction this needs and is why JARVIS does identity at all. What is missing
> is only the *degradation*: a sensitivity property on widgets and artifacts,
> following `Provenance`'s shape, plus the render treatment. That is a
> well-scoped piece rather than a research project, and it is the single highest
> value thing to build for this demo.
>
> Until it exists, beat 15 can *honestly* be shown as the presence state
> changing and narrated as what it will drive — but it must not be shown as
> working.

---

## Act 4 — The swarm at work

| # | Beat | Driven by | Status |
|---|---|---|---|
| 17 | State an idea. A spec is drafted, a bead DAG materialises | Alfalfa | REAL |
| 18 | Agents start; the roster fills — who is working, who is blocked | `canvas-agents` → `working_set` → `roster` → `NodeSummary` | DEMO DATA, marked |
| 19 | One agent goes quiet: it greys, says "last seen 40s ago", is dropped two minutes later | `Freshness::Stale`, `EVICT_AFTER_MS` | DEMO DATA, marked |
| 20 | The roster says "Agents (6 of 30)" — it never pretends to show the fleet | `Roster::not_shown` | DEMO DATA, marked |
| 21 | Watch what an agent is reading — the artifact appears on the wall | `canvas_get_artifact` | **NOT SERVED** — schema defined, broker not connected |
| 22 | Build completes, deploys | Alfalfa | REAL |

> **Beat 18-20 is the anti-strobe story and it is worth narrating, not just
> showing.** Ten agents is ten surfaces that change, not a thousand events that
> scroll. Slots never re-sort while occupied, so nothing moves under the
> reader's eye; a newcomer needs to be meaningfully more salient than an
> incumbent, not merely newer. Those are the reasons the wall is readable rather
> than impressive-looking-but-unreadable, and a two-hour demo makes the
> difference visible where a two-minute one would not.

> **Beat 21 is the milestone.** Retrieval *is* the attention signal — because the
> swarm reads through `canvas_get_artifact`, Canvas sees what an agent pulled by
> construction, with no second channel to emit on and nothing for an agent to
> forget. It needs the Alfalfa manifest and a store behind it.

---

## What to build first, in order

1. **`dock-7pa` — presence-gated sensitivity.** Beats 15-16. The identity axis
   exists; only the degradation is missing. Highest value per hour of the lot.
2. **Wire `boot_budget` into startup.** Beats 1-6 are unmeasured, and the
   animation cannot be budgeted against a number nobody has.
3. **The local-disk artifact store.** Beat 21, and it makes beats 8 and 21 real
   rather than narrated.
4. **The intro animation.** Last, deliberately — after the numbers it has to
   cover are known.

## What not to do

- Do not script a beat that does not exist and narrate it as working. Every
  **NOT BUILT** above is a thing to schedule, not a thing to mime.
- Do not add a demo-only code path. Mark data, never fork the path.
- Do not fake the crop, the blur, or the greeting-by-name.
- Do not compile anything during the demo. A cold build on this machine is
  12–15 minutes under contention; ship binaries.
