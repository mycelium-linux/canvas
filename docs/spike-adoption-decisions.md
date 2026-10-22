# Four spikes, four verdicts

*Decided 2026-09-10, after the fact-finding this time. Four spikes were built and
run — `spikes/plotters-iced`, `spikes/palette-colour`, `spikes/iced-aw`,
`spikes/blitz-html` — and this is the adoption call across them. It supersedes
nothing in `docs/charting-dependency-decision.md`; it discharges that doc's one
stated inference. It closes the open item in
`docs/servo-and-web-surfaces-decision.md` line 14 ("Blitz — after the spike that
already exists is run once"): the spike has now been run.*

*What I verified first-hand today, because two of these turn on compatibility and
a confident wrong answer there is the most expensive error available: every
version and dependency edge from the crates.io API; every source defect from the
extracted crate in `~/.cargo/registry`; the resolved graph in each spike's own
`Cargo.lock`; and `spikes/blitz-html/out/blitz.png` with my own eyes. What I did
not do is compile anything — every build time, microsecond timing, ΔE2000 figure
and primitive count below is the spike's word, and §7 says so item by item.*

---

## 1. The decision

| Spike | Verdict | Deciding reason |
|---|---|---|
| `plotters-iced2` 0.14.0 | **DEFER** | It really does render on iced 0.14 — that is now proven rather than inferred — but the zero-dependency alternative it competes with has still never been written, and adopting means vendoring a one-release fork *and* shipping two patches of your own. |
| `palette` 0.7.7 | **REJECT** | It agrees with `live.rs` to ΔE2000 ≤ 0.0001 and has no gamut-mapping API at all, so it changes no pixel and cannot supply the only part that is hard. |
| `iced_aw` 0.14.1 | **REJECT** | Current, compatible, well made, and uniquely supplies none of the five interactions it was evaluated for. Its defaults half-follow the palette, which is worse than ignoring it. |
| Blitz 0.3.0-beta.2 | **DEFER** for a doc-forge preview; **REJECT, permanently, for deliverables** | No `@page`, no pagination, and no vector backend anywhere in `anyrender`, so it can never produce the PDF. And the preview it *could* produce is broken in the one region a reviewer looks at first. |

Nothing is adopted. Three of the four spikes reached that conclusion themselves;
the fourth (Blitz) recommended adopting for prose surfaces, and §6 says why that
one does not survive contact with what is already compiled into the binary.

**On iced 0.14 compatibility: only one spike failed on it, and it is not an
ecosystem lag.** `plotters-iced` is a single dead crate — max version 0.11.0,
published 2024-09-18, targeting iced 0.13 — with a live fork. `iced_aw` 0.14.1
(2026-04-27) declares `iced_core 0.14.0` / `iced_widget 0.14.2`, exact matches
for this lock, under the `iced-rs` org. `palette` and Blitz have no iced
dependency at all. There is nothing here to wait out. The rule that does
generalise is narrower and was already written down once: for any iced-family
dependency, read the published manifest's `default_features` column, not just
the version. §6 has today's live example of why.

---

## 2. What the spikes changed, and what they did not

Two decision documents were written this morning from manifests and source with
nothing compiled. Here is what running the code actually moved.

**Discharged.** `charting-dependency-decision.md` §4 ended with *"Semver-compatible,
APIs present, not compiled — that last word is inference."* It is no longer
inference. `spikes/plotters-iced/Cargo.lock` resolves `plotters-iced2 0.14.0`
against `iced 0.14.0`, `iced_widget 0.14.2`, `iced_wgpu 0.14.0`, `wgpu 27.0.1` —
this lock's exact versions — and the app rendered and read back its own wgpu
surface. The fallback is real. That is worth knowing precisely because it means
the hand-rolled path can be tried without betting the outcome.

**Strengthened, in the same direction.** The spike measured 172 frame operations
at 900×420 *and* at 5760×1200 — identical, which is the cleanest possible proof
that nothing rasterises and that the wgpu geometry path is the one being used.
It also confirmed the dependency delta is 3 crates (`plotters 0.3.7`,
`plotters-backend 0.3.7`, `plotters-iced2 0.14.0`), 208 of the other 211 being
already present via iced and mlua.

**Newly disqualifying, and self-inflicted.** Three defects in `plotters-iced2`,
all verified by me in `~/.cargo/registry`:

> `backend/mod.rs:233` — the rotation branch of `draw_text` is commented out
> wholesale, above a `//TODO: fix rotation until text rotation is supported by
> Iced`. That excuse is stale: `iced_wgpu-0.14.0/src/geometry.rs` takes the glyph
> atlas only when `is_scale_translation()` and otherwise falls through to
> `text.draw_with(|path, color| self.fill(&path, color))`. iced 0.14 does support
> it. So `y_desc` renders horizontally and clipped.
>
> `backend/mod.rs:279` — `estimate_text_size` measures with
> `self.backend.default_size()` instead of `style.size()`. Every caption area is
> reserved at 16px no matter what was asked for, and the error scales with
> requested-size ÷ 16 — worst at wall type sizes.
>
> `backend/mod.rs:298` — `blit_bitmap` draws nothing and returns `Ok(())`. A
> tenet-12 violation living inside a dependency.
>
> `backend/mod.rs:305` — `#[allow(static_mut_refs)]` over a
> `static mut Lazy<HashSet<String>>` reached through `unsafe`, to leak font
> family names as `&'static str`. Unsynchronised, and it retains every distinct
> name forever.

Two of those you would ship patches for. That is the difference between a
dependency and a fork you are maintaining.

**One spike claim that is half wrong, on exactly the widget it proposes to
convert.** The plotters spike says the crate "supplies the tick values and legend
the hand-rolled programs lack." The tick values, yes — `live.rs` has six
`fill_text` sites and not one is a tick value. The legend, no:
`live.rs:117-133` already builds a `TimeSeries` legend as an iced `row` of status
dots and muted labels, themed through the palette. Converting `TimeSeries` to
plotters would *replace* a working legend with a boxed one. The delta is smaller
than advertised.

**Unmoved.** `palette` and `iced_aw` came back exactly where the morning's
manifest reading put them, with better evidence. `mod ok_utils;` is private
(`palette-0.7.7/src/lib.rs:429`) and there is no gamut module in the crate.
`iced_aw`'s `card::primary` resolves through `colors::PRIMARY`, which is
`DODGER_BLUE`, while its background comes from `theme.extended_palette()` —
hybrid, so a card looks broadly right and is wrong in the one place that carries
meaning. `badge::warning(_theme: &Theme, ...)` discards the theme outright.
`card::Style::default()` is white on `border_radius: 10.0`; Coherence's
`RADIUS_MD` is 6.0.

---

## 3. Order, and what unblocks what

Nothing is adopted, so what follows is the order for the work the spikes *earned*
— all of it against code this tree already owns.

1. **The `from_oklch` guard test.** First, because it is the precondition for
   touching colour at all, and because the load-bearing claim at `live.rs:1155`
   is currently protected by a prose comment. It blocks nothing and protects
   everything downstream of it, including any future conversion that would route
   series colours through an 8-bit `BackendColor`.
2. **`canvas::Cache` plus hand-rolled ticks on `TimeSeries`.** Second, because it
   is the only thing that can decide the plotters question. `canvas::Cache`
   appears zero times against 10 bare `Frame::new` sites in `live.rs`; the
   morning's decision named that the largest single renderer defect, and the
   plotters spike independently found the same thing from the other side
   (`Chart::draw` exposing `draw_cache` was listed as a *bonus* of adopting — it
   is available today for free). This step gates step 4.
3. **`Intent` / sealed `Action` / `authorize()` in `canvas-core`.** An
   independent track, blocked by nothing here, gating `dock-8n5`, `dock-4s7` and
   `dock-aqz`. Run it in parallel if approvals are actually near-term. It is the
   part of the `iced_aw` spike that should survive its own rejection: the widget
   question and the reply-path question turned out to be independent, and only
   the second one matters.
4. **Reopen `plotters-iced2` only if step 2 fails** — takes more than a day, or
   reads worse on the wall than a `configure_mesh()` chart would. That test was
   set this morning and it is still unrun. Failing it is the trigger; nothing
   else is.

Blitz has no step. It has a trigger (§5).

---

## 4. The smallest revertable first move

Each of these is one commit, and the revert is deleting it.

**1 — colour guard.** Apply
`spikes/palette-colour/PROPOSED-test-for-live.rs.txt` into the test module of
`crates/canvas-widgets/src/live.rs`. ~60 lines, test-module only, no new crates,
no shipped behaviour change. It builds the coherence-dark ramp through
`from_oklch` and through a test-local channel-clipping variant, cross-checks the
first against `series_colors` so it cannot pass by measuring the wrong thing, and
asserts the clipping variant *breaks* the 25° status guard. Revert: delete one
function.

**2 — one program.** Inside `TimeSeriesProgram` (`live.rs:1787`) only: a
`canvas::Cache` keyed on the data replacing the bare `Frame::new`, nice-range
tick selection, and labelled X/Y ticks via `frame.fill_text` reusing the pattern
already at `live.rs:1266`. Then look at it on the wall next to the seven that did
not change. Revert: one commit touching one program.

**3 — one call site.** Add `Intent`, `Origin`, sealed `Action`, `Denied` and
`authorize()` to `canvas-core` (~80 lines; `canvas-core` is right because it owns
the contracts both sides already depend on and has no iced dependency), then
change `main.rs:4301-4305` — `button(text(initial)).on_press(Message::RestoreSurface(..))`,
the only actuating widget in Canvas today — to `Message::Ask(Intent, Origin)`
with a single new arm in `update()`. The load-bearing detail is the private
`Seal` field: without it the type is documentation rather than a guarantee.
Revert: one module, one call site, one arm.

---

## 5. What each would cost

| | Build time | API lock-in | WASM (`dock-8nz`) | Texture / remote (`dock-qwv`) |
|---|---|---|---|---|
| `plotters-iced2` + `plotters` | +3 crates, no native deps; spike measured 1m41s to rebuild and relink | High. Canvas's iced upgrade cadence handed to a fork with one release and one maintainer, plus two patches you now own, plus `BackendColor` truncating to `(u8,u8,u8)` on the way out of the most precise code in the tree | Not blocked, but pointed at the wrong crate. `plotters 0.3.7` declares `wasm-bindgen` and `web-sys` **non-optional** under `cfg(target_arch = "wasm32")` — verified on crates.io today — and `canvas-widgets` is precisely the crate `dock-8nz`'s portability premise rests on | **Not constrained; mildly helped.** The backend holds `&mut canvas::Frame` and returns `into_geometry()`, the same tessellated path `live.rs` uses. 172 ops at both 900×420 and 5760×1200 — resolution-independent, so no pixels cross the wire |
| `palette` | +7 crates including a proc-macro | Low — and irrelevant, because it changes no output | Fine | Irrelevant |
| `iced_aw` | +3 crates (`iced_aw`, `iced_fonts`, `iced_fonts_macros` proc-macro); a `cdylib` in the graph, rlib-only as a dependency; `iced_fonts` `default = []` keeps the megabyte icon fonts out | Moderate and pointless: you write a per-widget style closure resolving every field from `VisualTheme` by hand, which is the same work as styling a plain `container` minus the freedom to shape it | Neutral (shell-side; `canvas-widgets` untouched) | Neutral |
| Blitz | +312 crates, 11m45s cold, 882 MB target dir, on a machine that has hit OOM and disk exhaustion during parallel compiles | High: beta pre-releases requiring exact pins (`=0.3.0-beta.2`, or `cargo add` silently resolves 0.2.x), on `stylo 0.20` | **Unverified in either direction.** Nobody has built stylo for wasm32 and nobody should assume it. Structurally you would duplicate the engine into the browser or ship its pixels | **Constrained, and on the wrong side of the split.** Blitz emits a full-resolution RGBA8 buffer per render. That is the data plane `dock-qwv` exists to gate — the payload that must not go to an unobserved screen — not the cheap control plane |

`dock-qwv` is the axis that separates the two deferred candidates most cleanly.
Plotters output is geometry, so it goes wherever iced goes and costs the same at
any resolution. Blitz output is a bitmap, so every remote screen either runs the
engine or receives the pixels.

---

## 6. What we would not do

- **Do not adopt `iced_plot`**, which the plotters spike floats as the
  alternative to the single-maintainer fork. Re-verified on crates.io today:
  `iced ^0.14, kind normal, optional false, default_features true`, features
  `["advanced"]` — the *umbrella* crate. Cargo unifies features additively, so it
  would re-enable `web-colors` under Canvas's deliberate
  `default-features = false`, flipping `GAMMA_CORRECTION` and recolouring the
  entire wall with no compile error and no failing test. 901 lifetime downloads,
  five breaking releases in six months. The spike proposed it without reading
  that column; that is the whole reason the column is worth reading.
- **Do not vendor `plotters-iced2` yet.** Vendoring is the right shape *if* it is
  adopted — it matches `vendor/coherence-tokens` and it is where the rotation and
  text-measurement patches would live. Doing it before step 2 converts an option
  into a maintenance obligation for a crate that may never be needed.
- **Do not let a plot script name a font**, in any adoption, ever. `static mut
  FONTS` behind `#[allow(static_mut_refs)]` is unsynchronised and retains every
  distinct name for the life of the process. Expose no font control at the Lua
  surface.
- **Do not use Blitz to make a client-facing document.** This is not a maturity
  judgement that time fixes: zero `@page`, zero pagination, and every backend in
  the `anyrender` ecosystem is a rasteriser. A raster PDF loses selectable,
  searchable, accessible text. WeasyPrint keeps its job.
- **Do not add an HTML engine for prose.** The Blitz spike's "adopt now" case is
  article bodies and briefings — and `crates/canvas/src/main.rs` already renders
  those, through `markdown::parse` and `markdown::view` at `:3952`, `:4031` and
  `:6219`, vector text through iced, zero new crates. There is no HTML anywhere
  in `crates/canvas` today (`roxmltree` parses the bundled SVG mark, nothing
  else). The real defect at that site is `markdown::Settings::from(&iced::Theme::Dark)`
  — a hardcoded theme bypassing the Coherence palette, a rule-7 violation worth
  about ten lines. Fix that instead of buying 312 crates to render the same
  paragraph.
- **Do not relax `element<'a, Message: 'a>`.** `canvas-widgets` is parametric
  over a `Message` it has no way to construct, so by parametricity the library
  provably cannot emit anything today. That is the action-side analogue of rule 5
  and it was apparently never written down. When a widget needs a reply path,
  pass an explicit `on_action: &dyn Fn(Intent) -> Message` so the guarantee
  narrows instead of disappearing.
- **Do not read "the spike compiled" as "`canvas-widgets` compiles."** The
  plotters spike built a standalone binary against umbrella `iced`;
  `canvas-widgets` uses `iced` with `default-features = false` and
  `["wgpu", "canvas", "tokio"]`. Feature unification across that boundary is read,
  not built.
- **Do not start any of this on `rename/dock-to-canvas`.** Still dirty, still
  mid-rename. Entangling a rendering change with a rename bisect costs a day
  nobody gets back.
- **Reclaim 3.7 GB before doing anything.** The four spike `target/` dirs are
  1.9 GB, 995 MB, 884 MB and 63 MB on a disk that was at 96% during the runs.
  `spikes/palette-colour/` can go entirely — its two logs are the deliverable and
  are quoted in this document.

---

## 7. What the spikes claimed that I could not verify

Stated in full, because a synthesis that launders four sets of unverified numbers
into one confident table is worse than the four reports it replaces.

- **I compiled nothing.** Every build time, every microsecond figure (270 µs per
  Lua eval), every ΔE2000 number, every hostile-script tally (10/10 rejected),
  every primitive count (172 ops, 18 strings) and every render latency
  (116–416 ms) is taken on the spike's word. What I verified is the structure
  underneath them: versions, dependency edges, resolved locks, and source.
- **The rotation patch does not exist as a landed patch.** `/tmp/rotation-fix.txt`
  is present (1.5 KB, today 02:08) and the iced 0.14 fallback it relies on is
  real in source — `iced_wgpu-0.14.0/src/geometry.rs`, the `else` branch calling
  `text.draw_with(...)`. Whether ~15 lines actually restores a rotated `y_desc`
  through plotters is a draft's claim, and the tessellated-instead-of-atlased
  cost is untested.
- **Whether `plotters-iced2` compiles inside `canvas-widgets`' feature set** —
  see §6. Unbuilt.
- **`palette`'s 332,640-sample sweep and its ΔE2000 figures** were not re-derived.
  The two structural facts under them I did check: `ok_utils` is private and
  there is no gamut module; `FromColor: FromColorUnclamped + Clamp`, so
  `Srgb::from_color(oklch)` clamps silently and a following `is_within_bounds()`
  is unconditionally true. Those two are enough to carry the rejection on their
  own.
- **The `iced_aw` spike never opened a window.** Its rendering claims are
  compilation plus unit assertions. I verified its two load-bearing source
  claims independently (`card::primary` → `colors::PRIMARY` = `DODGER_BLUE`;
  `badge::warning(_theme, ...)` discarding the theme) and they hold.
- **Blitz on wasm32 is unknown**, and the spike did not claim otherwise. Do not
  let it become assumed.
- **The Blitz table bug I did verify myself**, and it is the one place where
  looking beat reading. `spikes/blitz-html/out/blitz.png` shows a heavy black grid
  over a table whose CSS asks for two 1px hairlines, and the QTY column's values
  — `5` and `2` — clipped at the right edge. `DioxusLabs/blitz#504` is open as of
  today (created 2026-07-10, last updated 2026-08-20). Every doc-forge document
  template is a line-item table, so that single issue is the whole trigger:
  **revisit Blitz when #504 closes, and not before.** The spike is committed and
  re-runnable in minutes.
- **Whether the hand-roll takes an afternoon.** That is the one estimate the
  entire charting decision rests on, it is unmeasured, and step 2 exists to
  measure it.
