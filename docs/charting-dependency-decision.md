# Charting dependencies: the decision

*Decided 2026-09-10. Produced by a six-agent evaluation (four candidates, one
adversarial challenge pass, one synthesis) run against this machine's
`Cargo.lock` and `~/.cargo/registry`, and against crates.io's API on the day.
Nothing was compiled — a parallel build owned the machine — so every claim
below is either read from a manifest or read from source, and the two places
where that leaves inference rather than verification say so.*


## 1. The decision

**Adopt nothing.** Canvas's chart code stays hand-written, and the quality gap closes by writing axis labels, a heatmap colour scale, and in-plot legends directly against the 11 `impl canvas::Program` blocks that already exist. Every candidate fails on a different axis and none of them fails narrowly: `plotters-iced2` is the only crate whose iced-0.14 compatibility survives reading its published manifest, but it is a single-release, single-maintainer fork of an upstream that has shipped nothing in two years, and it still cannot draw Gauge, StateTimeline, SloBurnRate, or the heatmap colour scale — you would vendor ~900–1,200 lines and hand-roll half the audit anyway. `iced_plot` is disqualified outright on a fact that is invisible until it is on screen (§4). `iced_aw` is cleanly compatible and contains zero charting code. `palette` produces bit-identical output to the arithmetic already in `live.rs`. `taffy` would compete with a tested solver that already exists in `operational_views.rs`. The two changes that would improve the wall most cost **zero new dependencies and are already compiled into the binary**: `canvas::Cache`, which appears nowhere in this tree against 19 bare `Frame::new` call sites, and `iced::Animation`, which arrives via `lilt 0.8.2` — a non-optional dependency of `iced_core 0.14.0`, per `Cargo.lock`. Do those first; they are also the honest test of whether a charting crate is ever needed.

## 2. What it replaces

Nothing gets deleted, because nothing gets added. What changes is what gets *written*:

| Stays hand-rolled, permanently | Why |
|---|---|
| `to_oklch` / `from_oklch` / gamut map, `live.rs:1128-1215` | `palette`'s Ottosson matrices are the same constants at the same precision. `palette` core has no fixed-lightness gamut map at all — its `Clamp` clamps components, which is the channel-clipping this code exists to avoid. Swapping buys zero pixels. |
| `series_colors` (`live.rs:1007`), the status-hue guard, `STALE_CHROMA` drain (`live.rs:760+`) | This is design judgement. No crate has an opinion about avoiding status hues. |
| `GaugeProgram` (1706), `StateTimelineProgram` (1409), `BurnRateProgram` (1470) | Radial and non-Cartesian. Forcing them through a `ChartBuilder` yields worse code than what is there. |
| `operational_views.rs` `solve_axis` (301), `Tracks` (374), `drain` (463) | 814 lines with 10 tests and three real `size_hint` consumers (`operational_views.rs:244`, `:599`, `main.rs:8458`). The "size_hint has no consumer" premise in the brief is stale. |
| `transition.rs` `lerp_rgb` / `lerp_palette` (26, 35) | `lilt-0.8.2/src/traits.rs` implements `FloatRepresentable` for `bool` (:30) and `f32` (:40) only. It cannot interpolate a `ThemePalette`. |

What *does* get replaced by already-present iced: `transition.rs:11 progress_at` and `:17 ease_out_cubic` — about 15 of that file's 191 lines — plus the hardcoded `TRANSITION_DURATION = 280ms`, in favour of `iced::Animation`'s `Easing` presets, delay, repeat, and interruption. That is a real but small deletion; do not let anyone sell it as "deletes hand-rolled easing."

## 3. What gets better on screen

The audit's four findings, resolved honestly:

- **Unlabelled axes — FIXED, and the gap is worse than "no tick values".** `grep` finds exactly six `fill_text` sites in `live.rs`: 1266, 1276, 1330, 1379, 1437, 1490 — all inside Heatmap, RankedBar, StackedBar, StateTimeline and BurnRate. `SparklineProgram` (1668), `GaugeProgram` (1706), `HistogramProgram` (1742), `TimeSeriesProgram` (1787) and `ChartProgram` (1896) contain **no text at all**. The five chart-shaped kinds have no tick values because they have no glyphs. That is nice-range selection plus a formatter plus six `fill_text` calls per program — the same code `HeatmapProgram` already contains.
- **No legends — PARTLY WRONG, and worth correcting.** A legend does exist, as an iced `row` outside the canvas (`live.rs:118`, wired at `:142`, tested at `:2148`). The real finding is narrower: no in-plot legend, and no legend on the kinds that need one most.
- **No heatmap colour scale — FIXED ONLY BY HAND.** No candidate supplies this. `plotters` has no matrix chart type and no colour-scale legend element; adopting it would leave this exactly where it is.
- **No motion — FIXED, by `iced::Animation`, not by any charting crate.** `plotters` is a stateless per-frame renderer with no animation concept; `iced_plot` and `iced_aw` do not address it either. This is the one audit finding a charting dependency cannot touch.

And the finding the audit missed: **zero caching.** No `canvas::Cache` anywhere in `crates/canvas-widgets` or `crates/canvas`, against 19 `Frame::new` sites (11 in `canvas-widgets`, 8 in `canvas`). Every widget fully re-tessellates on every repaint of a 5760×1200 wall. This is almost certainly the largest single performance defect in the renderer and it costs nothing to fix.

## 4. The compatibility position

Stated with its evidence, because one candidate's compatibility problem is silent.

- **`plotters-iced2` 0.14.0 is genuinely compatible, and it is the only one.** Its published manifest declares `iced_widget = "0.14" (features = ["canvas"])` and `iced_graphics = "0.14"` against the *sub-crates*, not the umbrella. Verified locally that this is safe: neither `iced_widget-0.14.2` nor `iced_graphics-0.14.0` declares a `default = [...]` list, and `canvas = ["iced_renderer/geometry"]` is a feature Canvas already enables. Residual uncertainty: it was published against 0.14.0/0.14.1 and this lock resolves `iced_widget 0.14.2`. Semver-compatible, APIs present, not compiled — that last word is inference.
- **`iced_plot` 0.5.0 is a live hazard and this is the headline.** crates.io's dependency API, fetched today: `iced ^0.14, kind normal, optional false, **default_features true**, features ["advanced"]`. The umbrella crate. Verified locally in `iced-0.14.0/Cargo.toml`: `default = ["wgpu", "tiny-skia", "crisp", "web-colors", "thread-pool", "linux-theme-detection", "x11", "wayland"]`. Cargo unifies features additively, so Canvas's deliberate `default-features = false` on its own `iced` line would be overridden by this one dependency. Verified in `iced_graphics-0.14.0/src/color.rs:26-47`: `web-colors` flips `GAMMA_CORRECTION` from `true` to `false` and makes `pack()` ship raw non-linear sRGB to the shader instead of `color.into_linear()`. Every colour on the wall would composite differently, on top of the most carefully-reasoned code in the repo, with **no compile error and no failing test**. It also carries a non-optional `getrandom ^0.4 (features = ["wasm_js"])`. Adoption: 901 lifetime downloads, 209 for 0.5.0, five releases since 2025-12-16.
- **`iced_aw` 0.14.1 is compatible** — verified first-hand from the extracted crate on this machine: `iced_core = "0.14.0"`, `iced_widget = "0.14.2"`, exact matches for this lock, no umbrella `iced` outside dev-deps. It is also 33,293 lines of input widgets for a nine-widget read-only surface, with `default = ["full"]` as a footgun and a non-optional `iced_fonts 0.3.0`. Compatible and irrelevant.
- **Upstream `plotters-iced` is dead for this purpose.** crates.io today: max version 0.11.0, published 2024-09-18, targeting iced 0.13. Twenty-four months, no 0.14.
- **`palette` 0.7.7** has no iced dependency, so compatibility is a non-question. Note for the record: it is *not* in `Cargo.lock` (513 packages, no `palette` entry) though `palette-0.7.7` now sits in the local registry — an artifact of this evaluation, not of the build. Net-new either way.

## 5. What it would cost

Cost of the decision itself is roughly 150–250 lines in `live.rs`, against infrastructure that already exists. Cost of the rejected path, for the record:

- **Build time.** `plotters` with `default-features = false` is ~5 net crates and no native deps — genuinely small, and *not* the objection. `iced_aw` is the expensive one: 33k LOC, a `cdylib` linked on every build alongside the rlib, plus the `iced_fonts_macros` proc-macro serialised into the graph. That matters concretely on a machine that has already hit OOM and disk exhaustion during parallel compiles.
- **API lock-in.** Adopting `plotters-iced2` hands Canvas's iced upgrade cadence to a fork maintainer whose crate has one version and has shipped nothing in ~8.5 months. When iced 0.15 lands, Canvas waits — which is exactly what stranded upstream's users on 0.13. Vendoring converts a supply-chain risk into a maintenance obligation; it does not remove it.
- **WASM (dock-8nz).** No hard lockout — all candidates carry wasm32 support. But dock-8nz's premise is that `canvas-widgets` has five dependencies (`serde`, `serde_json`, `canvas-core`, optional `iced`, optional `schemars`) and that only `render.rs` is non-portable. Three of the four proposals add dependencies to precisely that crate. Verified from crates.io today: `plotters 0.3.7` declares `wasm-bindgen ^0.2.89` and `web-sys ^0.3.66` **non-optional** under `cfg(all(target_arch = "wasm32", not(target_os = "wasi")))`, with `HtmlCanvasElement` / `CanvasRenderingContext2d` features. So the "5 crates, no native deps" figure is a native-only figure and the WASM build is materially heavier.
- **Texture rendering (dock-qwv).** Not constrained by anything here. `plotters-iced2`'s backend holds `&mut canvas::Frame` and returns `frame.into_geometry()` — the same tessellated wgpu path `live.rs` uses today. Its single bitmap entry point, `blit_bitmap`, is a no-op that silently returns `Ok(())` (a tenet-12 violation living inside a dependency), unreachable unless you use plotters bitmap elements.
- **Colour precision.** `plotters`' `BackendColor` is `(u8,u8,u8)` + `f64` alpha. `from_oklch` runs an eight-step bisection to land within a percent of the gamut edge in `f32`, then every result would round to 8 bits on the way out. Invisible on an sRGB wall; a hard cap on any P3/HDR ambition, and architecturally backwards — the most precise code in the tree feeding the least precise interface in the proposal.

## 6. First move

**Port `WidgetKind::TimeSeries` — and only that — to the finished bar, by hand, behind the existing `WidgetKind` API.** Three changes inside `TimeSeriesProgram` (`live.rs:1787`), which today draws no text and caches nothing:

1. Give it a `canvas::Cache`, keyed on the data, replacing the bare `Frame::new` at `live.rs:1800`.
2. Add nice-range tick selection and labelled Y and X ticks via `frame.fill_text`, reusing the pattern already at `live.rs:1266`.
3. Move the legend inside the plot area, from the external `row` at `live.rs:118`.

Then look at it on the wall next to the seven widgets that did not change. This is falsifiable in both directions: if it takes more than a day, or reads worse than a `configure_mesh()` chart would, the decision is disproved and `plotters-iced2` earns a vendored spike. If it takes an afternoon — which the six existing `fill_text` sites suggest it will — the remaining nine programs are the same work, and no dependency was ever needed. Revertable as a single commit touching one program.

Do `iced::Animation` in `transition.rs` as a separate, independent commit. It depends on none of this.

## 7. What we would not do

- **Do not adopt `iced_plot`**, at any point, without first re-reading its `default_features` flag on the umbrella `iced` dependency. It arrived recommended in evaluation and it would have silently changed every colour on the wall. The generalisable rule: for any iced-family dependency, read the published `Cargo.toml.orig` from `static.crates.io` and read the `default_features` column, not just the crate names. A CI assertion that `iced_graphics` builds without `web-colors` would be cheap insurance, because the failure mode produces no compile error and no test failure.
- **Do not adopt `palette`.** Bit-identical output, +3 to +7 crates, and the one thing that could have justified it — a fixed-lightness gamut map — is the one thing it does not have. `palette-gamut-mapping` exists but is v0.1.1, June 2024, ~3k lifetime downloads: a worse tenet-11 position than the 20 lines it would replace.
- **Do not adopt `taffy`.** Flexbox is a flow algorithm; Canvas placements are absolute, draggable and overlap-capable (`Placement.layer: i32`, drag writing bounds at `main.rs:3753`). Keeping drag forces `Position::Absolute` on every node, which turns off the constraint solving that was the entire reason to adopt it. And `operational_views.rs` already solves the real problem, with tests.
- **Do not adopt `iced_aw`.** Compatible, well-made, aimed at a different kind of application. It contains no charting code and therefore moves none of the four audit findings.
- **Do not "fix" the OKLCH code — test it.** The claim that its bisection has zero coverage is wrong: `from_oklch` is reached from `series_colors` (`live.rs:1028`) and the stale drain (`:767`), and `series_colors` is exercised by four tests (2184, 2211, 2228, 2242). What is missing is an *assertion about the mapping's behaviour*. Add ~15 lines: construct a deliberately out-of-gamut `Oklch`, assert `from_oklch` holds hue within tolerance while reducing chroma, and assert the naive channel-clip alternative shifts it. That hardens an assertion rather than covering dead code, and it makes the 14° claim in the doc comment at `live.rs:1155` a fact instead of a prose assertion.
- **Do not start any of this on `rename/dock-to-canvas`.** It has uncommitted `canvas-metrics` work and a build repaired two commits ago (`f662fe0`). Entangling a rendering change with a rename bisect is a day nobody gets back.