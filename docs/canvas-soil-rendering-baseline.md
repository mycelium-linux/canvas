# Canvas ↔ Soil rendering baseline

Status: **gap analysis and ownership proposal**, 2026-11-06. Written from the
trees attached to this thread; no `neo`-local branch was visible, so anything
stated about unpushed work is inferred from the pins that reference it.

This document answers one question — **what does Canvas own and what does Soil
own on the render path** — and indexes the work that already exists on both
sides so the answer can be enacted instead of re-argued. It supersedes nothing;
where it contradicts a decision record it says so by name.

---

## 1. Baseline: where the code actually is

### 1.1 Repositories on the render path

| Repo | Worktree | Role on the render path |
|---|---|---|
| `FuturePresentLabs/dock` → *Canvas* | `dock` | Compositor, workspace, windowing, iced/wgpu renderer, `canvas-web` |
| `fungOS-linux/soil` | `soil` | Runtime, capability hosts, `soil-webgpu`, scene/layout/input crates (unmerged) |
| `fungOS/spores` | `spores` | Spore packages + one Lua UI library (`sdk/ui/ui.lua`) |
| `fungos-sdk` | `fungos-sdk` | A **second** Lua UI library, chart/graph/map/atlas/live layers, `tend` |
| `records`, `mycelium`, `marbles` | — | Data, grants/identity, issue tracking. No pixels; not analysed further. |

### 1.2 Canvas has three lineages, and two of them diverge

All three share `25121f0` ("ci: resolve sibling deps by repo identity"):

```
origin/master  8fec04f  2026-10-28
   └── 25121f0
        ├── origin/rename/dock-to-canvas  b2e5f50  2026-11-04   merge of feat/spore-gpu-surface
        └── origin/pi/soil-scene          77e69c3  2026-11-06   37 commits, the real frontier
```

- **`origin/pi/soil-scene` is the newest Canvas work in existence** (`2026-11-06`)
  and the only branch that integrates Soil. It adds `crates/canvas-workspace`,
  `spore_catalog.rs`, `spore_present.rs`, `spore_socket.rs`, `docs/surface-backends.md`,
  host text measurement, and `canvas-web` surface windows: 82 files,
  +10,063/−914 from `25121f0`.
- **`origin/rename/dock-to-canvas` is small and additive.** Only two files move
  from `25121f0` (+190/−21): `crates/canvas-workspace/src/lib.rs` gains
  `SurfaceContent::Gpu`, and `crates/canvas/src/main.rs` gains its handling. It
  does **not** uniquely carry crates: `canvas-dcp`, `canvas-tardy-tv`,
  `canvas-tvos`, `canvas-android`, `canvas-widget-host`, `canvas-workspace`,
  `vendor/coherence-native` and `vendor/iced_wgpu` are on **both** frontier refs.
- **The two frontiers merge almost cleanly.** `git merge-tree --write-tree` on
  either direction reports exactly one conflict: `crates/canvas/src/main.rs`.
  `canvas-workspace/src/lib.rs` auto-merges. The remaining work is mechanical
  manifest and lockfile reconciliation plus one semantic resolution.
- **The checkout in this worktree is stale and is on neither frontier.**
  `HEAD` is `f9aa610`, which is `origin/pi/canvas-pre-rename-features`, also
  labelled `rename/dock-to-canvas` locally. It has no `canvas-workspace`, no
  `canvas-dcp`, no vendored `coherence-native`. Anything measured on this tree
  about Spore/Soil integration is measuring zero. (My first read of this
  document claimed the rename branch uniquely carried those crates. That was
  measured against this stale tree, not against `origin` — it was wrong, and
  `crates/canvas/src/main.rs` is the only real conflict.)

### 1.3 Soil has one frontier branch, and it is far behind `main`

`origin/pi/layout` (`714a1b9`, 2026-11-06) adds the crates the render path needs:

| Crate | What it is |
|---|---|
| `soil-scene` | the presentation payload: `Present::{Scene, Rgba, GpuLease}`, node vocabulary, validation, `shm` |
| `soil-layout` | deterministic host layout over **Taffy** 0.15 |
| `soil-input` | normalized input types shared by Canvas, terminal and imported surfaces |
| `soil-scene-gpu` | reference wgpu renderer + `soil-viz`, `soil-live-frame`, `soil-vt-gpu` |
| `soil-vt`, `soil-terminal` | the terminal emulator Canvas borrows |
| `soil-ui-demo` | the UI kit demo |

`origin/pi/layout` is **45 ahead / 69 behind `origin/main`**. It is missing every
host crate added to `main` since `a6e6a4d`: `soil-notify-host`, `soil-http-host`,
`soil-marbles`, `soil-consent`, `mush`, `soil-command`, `soil-search-host`,
`soil-inference-host`, `soil-principal-host`.

### 1.4 The branch that matters does not build

`crates/canvas/Cargo.toml` on `origin/pi/soil-scene`:

```toml
soil-vt    = { path = "../../../soil/crates/soil-vt" }
soil-scene = { path = "../../../soil/crates/soil-scene" }
soil-input = { path = "../../../soil/crates/soil-input" }
```

None of those three crates exist on `soil@main`. They exist only on
`soil@origin/pi/layout`. So:

> **Canvas `pi/soil-scene` cannot build against any published Soil ref.**

Canvas needs **four** external prerequisites and nothing establishes them:
`../gfx` (root `[patch]`, pinned `83b7d46`), `../soil` (three path deps),
`../unibus` (five path deps, pre-existing), and the git remotes for `dcp` and
`unibus`. There is no setup script; `README.md:40-47` states the `gfx`
precondition in prose and the `soil` one only in a `Cargo.toml` line.

### 1.5 Soil pins a Canvas that no longer exists

`crates/s22-canvas-surface/Cargo.toml` (soil `main`):

```toml
canvas-workspace-ui = { git = ".../FuturePresentLabs/dock.git", rev = "6e3ba64..." }
coherence-native    = { git = ".../FuturePresentLabs/dock.git", rev = "6e3ba64...", features = ["egui"] }
```

`6e3ba64` is `origin/pi/soil-webgpu`. `canvas-workspace-ui` and
`coherence-native` **exist nowhere else in the repository any more** — not on
`master`, not on `pi/soil-scene`, not on `rename/dock-to-canvas`. The pin is the
last surviving copy of a crate that was deleted from every branch. So the S22
phone host is pinned to a dead rev, and runs a second renderer (egui +
`egui_software_backend`) beside iced. Note also that `soil@origin/pi/layout`
**drops `soil-runtime` and `s22-canvas-surface` from its workspace members**
(their `Cargo.toml`s reference unavailable `s22-canvas-surface`,
`fungos-device-hal` and `fpl-gfx` siblings) — so the S22 track is already
effectively dropped by the render branch as well as abandoned by Canvas.

---

## 2. What exists, per repo, as an inventory

### 2.1 Canvas

| Concern | Where |
|---|---|
| Scene type + node vocabulary | `canvas-workspace::NodeContent` = `Group, Rect, Text, Image, Mesh, Camera, Light` (`pi/soil-scene`); on the stale tree it is `crates/canvas/src/lib.rs` |
| Lua scene generation | [`crates/canvas/src/lua_scene.rs`](../crates/canvas/src/lua_scene.rs) — fresh Luau VM per generation, `canvas.group/text/image/mesh` only |
| Lua modules + hot reload | [`crates/canvas/src/lua_module.rs`](../crates/canvas/src/lua_module.rs) — manifest-gated inputs, transactional reload |
| Widgets / charts | `canvas-widgets/src/live.rs`, 11 `impl canvas::Program` blocks |
| wgpu map | `canvas-map-wgpu`, `canvas/src/map/globe.rs` |
| Browser client | `canvas-web` — wasm + `canvas-web-host` loopback bridge |
| Workspace document | `SurfaceContent` (12 kinds incl. `Gpu`), `ContentSource` |
| Spore surface (frontier only) | `spore_socket.rs` (`<runtime>/ui.sock`, line-JSON), `spore_present.rs` (`soil-scene` ↔ Canvas translation), `spore_catalog.rs` (reads installed packages) |
| Capture for agents | view revisions + `soil:view.capture` |

### 2.2 Soil

| Concern | Where |
|---|---|
| Guest ABI | soil `wit/soil-canvas.wit` — `present/observe/input/close`; soil `crates/soil-canvas-input` normalizer |
| GPU for guests | soil `crates/soil-webgpu` — `wgpu-core` 29, upstream `wasi:webgpu@0.3.0-rc.2` |
| GPU presentation bridge | `soil:canvas-gpu@0.1.0` — texture by borrow, `present`, host picks extent |
| S22 host | `s22-canvas-surface`, `soil-runtime/src/canvas.rs`, `soil-runtime/src/evdev.rs` |
| Synthetic guest frame | `soil-runtime` 32-byte records, rect + disc only |
| Scene/layout (unmerged) | `soil-scene`, `soil-layout`, `soil-input`, `soil-vt` on `origin/pi/layout` |
| Capability map | soil `docs/abi-map.md` |

### 2.3 Spores / SDK

| Concern | Where |
|---|---|
| Lua UI library A | `spores/sdk/ui/ui.lua` (680 lines) — **self-measuring**: lays out in one pass and emits positioned primitives |
| Lua UI library B | `fungos-sdk/lua/fungos/ui.lua` (1278 lines) — **host-measuring**: emits a semantic tree |
| Composition | `fungos-sdk/lua/fungos/{compose,config,action,validate}.lua` |
| Viz layers | `fungos.chart`, `fungos.graph`, `fungos.map`, `fungos.atlas`, `fungos.viz`, `fungos.live` |
| Scene model (Rust) | `fungos-sdk/rust/crates/fungos-sdk/src/scene.rs` — `rects` + `texts`, coordinates in `-4..4` |
| Contracts | `spores/docs/{host-contract,ui-model,surface-windowing,window-policy,content-rendering}.md` |
| Tooling | `tend` (preflight + compose + preview), `tend --scene`, `soil-viz` |

---

## 3. The fragmentation, numbered

This is the list the question "what should Canvas own" is really about. Each is
a place where two things claim the same job.

1. **Four node vocabularies.** `canvas-workspace::NodeContent` (7 kinds),
   `soil-scene::Content` (11 kinds + 3 `Present` variants), `fungos-sdk::scene`
   (`rects`/`texts`, `-4..4` coords), and the S22 guest frame (rect/disc,
   32-byte records). Only two are bridged, by a 301-line
   `crates/canvas/src/spore_present.rs` whose translation is ~180 of those lines
   and whose tests are the rest. **Both directions silently degrade**: seven
   Soil kinds (`Polyline, Area, Band, Points, Heatmap, Candles, Shader`) become
   `Group` going into Canvas, and three Canvas kinds (`Mesh, Camera, Light`)
   become `Group` going out. The same file converts quaternion↔Euler, so the
   transform representation is duplicated too.
2. **Three layout owners.** Canvas/iced places windows (absolute, draggable,
   overlapping); `soil-layout` runs Taffy flow; `spores/sdk/ui.lua` lays itself
   out in a single pass before any host sees it.
3. **Two Lua UI libraries** with *opposite* layout philosophies (2.3 above),
   both shipped, both documented as the answer.
4. **Two Canvas renderers.** iced/wgpu on desktop; egui + `egui_software_backend`
   on S22 — from a pinned old rev, with a vendored `coherence-native` that the
   frontier tree does not carry.
5. **Two contribution models** for a Spore. `soil:canvas@1.0.0` says a Spore
   calls `present()` and the *host* renders a host-owned owner view; Spores/`ui-model.md`
   says a Spore returns a UI tree; `surface-windowing.md` says a Spore gets a
   *lease* and presents pixels or a texture. All three are written down as
   decisions.
6. **Two theme systems.** Vendored `coherence-tokens` (coherence at
   `rename`), `coherence-native` (egui, pinned), and semantic roles duplicated
   in `spores` and `soil-scene::Ink`.
7. **Three renderers of the same scene.** `soil-scene-gpu` (wgpu, native
   windows), Canvas/iced, `canvas-web` (browser). They will drift unless one is
   declared reference.
8. **No build that ties them together.** §1.4 — the only branch that imports
   Soil cannot resolve Soil.

---

## 4. Proposal: the split

### 4.1 The rule

> **Canvas owns the surface. Soil owns the payload.**

Restated precisely, because "Canvas owns the pixels" has been read three ways:

- **Canvas owns everything with a native handle**: window, compositor, seat,
  focus, occlusion, z-order, the final wgpu surface/swapchain, PTYs, video
  decoders, and the capture buffer an agent reads.
- **Soil owns everything that is data or a sandbox**: the scene payload, the
  layout algorithm for trees a Spore describes, normalized input *types*,
  the Lua VM, the WebGPU device, and the grants.
- **One implementation per fact.** Where two repos both own a thing today, §4.3
  picks the survivor.

### 4.2 Which side owns which artifact

| Artifact | Owner | Why |
|---|---|---|
| Window, workspace doc, tiling, focus, z-order, occlusion | **Canvas** | Not representable in a sandbox; needs the native surface |
| Native GPU surface / swapchain / final present | **Canvas** | Only place a native handle may live |
| PTY, video decode, image decode, external surface ingress | **Canvas** (`Native` backend) | Needs devices and transports |
| Capture for agent parity | **Canvas** | Must be the same revision the person saw |
| **Scene vocabulary** (`Present`, `Node`, `Content`) | **Soil** (`soil-scene`) | Versioned data, not WIT; must outlive any host |
| **Flow layout for Spore trees** | **Soil** (`soil-layout`) | Deterministic, no font handle, testable once |
| **Input normalization** (keycodes → semantic keys) | **Soil** (`soil-input`) | Already stated; each surface keeps its own grant |
| **Lua sandbox + scene validation** | **Soil** (`soil-scene::validate`, limits) | One set of ceilings |
| **GPU device for guests** | **Soil** (`soil-webgpu`) | wgpu-core in the host, `wasi:webgpu` to the guest |
| **Terminal emulation** | **Soil** (`soil-vt`) | Already borrowed by Canvas — this split is working |
| **Theme tokens** | **Coherence** (one copy) | Neither repo may fork it |
| **Theme → material** (fonts, glyph atlas, rasterization) | **Canvas** per host | iced and wgpu hosts differ |

### 4.3 Survivors (the de-duplication)

| Loser | Survivor | Note |
|---|---|---|
| `fungos-sdk::scene` (`-4..4` coords) | `soil-scene` | Delete; coordinates belong to layout, not the wire |
| `spores/sdk/ui/ui.lua` one-pass layout | `fungos-sdk` semantic tree + `soil-layout` | **Needs your call** — see D1 |
| `soil:canvas@1.0.0` `present()` owner-view model | `soil-scene::Present` on the socket | `soil:canvas` keeps `observe`/`input`/`close`; `present` becomes "here is my payload" |
| `SurfaceContent::Widget` / `DataViz` / `Prompt` | `Spore` backend | Already written in `docs/surface-backends.md` (`pi/soil-scene`) |
| `coherence-native` (egui, pinned rev) | `coherence-tokens` + the host's own material layer | It is vendored on *both* frontier refs; the problem is only the soil pin at a rev that predates them — D4 |
| `s22-canvas-surface` @ `6e3ba64` | a host that consumes `soil-scene` | D5 |

---

## 5. Decisions, with a recommendation each

**D1 — Which Lua UI model wins?**
`spores/sdk/ui/ui.lua` measures and positions in Lua (680 lines, shipping in
`spores/markdown`). `fungos-sdk/lua/fungos/ui.lua` emits a semantic tree and
lets the host measure (1278 lines, has chart/graph/map/atlas/live layers, has
`tend`).

> **Recommendation: host-measuring wins.** It is the only one compatible with
> `soil-layout`, with `window-policy.md`'s "viewport in, variants chosen in Lua",
> and with the rule that a sandbox holds no geometry. Port `spores/markdown` to
> it and delete the one-pass library — otherwise the one-pass library will keep
> winning by being the only one that works.

**D2 — Does Canvas keep iced?**

> **Recommendation: yes, for the desktop compositor, this cycle.** The boundary
> is `soil-scene`, so the renderer is replaceable later. Replacing iced now would
> entangle a renderer rewrite with the Spore migration and the rename bisect.

**D3 — Does `soil-scene-gpu` stay?**
It is a second renderer (wgpu/win it) beside Canvas/iced.

> **Recommendation: keep it, and name it the *reference* renderer.** It is what
> makes "Soil without Canvas" real (headless, S22, `soil-live-frame` captures,
> `tend --scene` previews) and what proves the payload is renderable by a host
> that is not the compositor. Declare Canvas/iced the *production* renderer and
> require a golden-frame test that both draw the same `soil-scene` input.

**D4 — One theme material layer or two?**

> **Recommendation: one token source (`coherence-tokens`), two material layers**
> (iced, and the wgpu/egui hosts), with a contrast test over the palette matrix
> in each — the check `widget-quality.md` rule 7 already asks for.

**D5 — What happens to `s22-canvas-surface`?**
It pins Canvas `6e3ba64` and links `canvas-workspace-ui` + `coherence-native`.
Those two crates no longer exist on **any** branch of Canvas; that rev is their
last surviving copy, and soil's own render branch (`pi/layout`) has already
dropped `soil-runtime` and `s22-canvas-surface` from its workspace.

> **Recommendation: retire the pin.** Either bring the owner UI forward as a
> `soil-scene` producer, or delete the S22 host path and say so. A pin to a rev
> whose crates were deleted is not a supported configuration, and nobody can
> rebuild it from the current repository.

**D6 — Is `soil:surface` (guest scene submission) still deferred?**
`soil-canvas.md` says deferred; `pi/soil-scene` already ships it over
`ui.sock`.

> **Recommendation: it is no longer deferred, and it is not a WIT interface.**
> `soil-scene` is a versioned document on a host-approved socket, which is
> exactly why it can evolve. Update `soil-canvas.md` and `abi-map.md` to say so;
> `soil:canvas` keeps `observe`/`input`/`close` and `soil:canvas-gpu` keeps the
> texture lease.

**D7 — Where does HTTP/transport for a `GpuLease` live?**
Today `spore_socket.rs` carries base64 RGBA frames over line-JSON.

> **Recommendation: keep line-JSON for `Rgba`, and move `GpuLease` to the
> shared-memory path `soil-scene::shm` already models.** A 4096² RGBA frame is
> 64 MB of base64 before it is a bug report.

---

## 6. Visual fidelity: ChartGPU / GraphGPU / MapGPU / deck.gl / Embedding Atlas

All of those are WebGPU draw pipelines. The port target is **`soil-scene` + a
wgpu layer library**, not Canvas widgets and not `live.rs`, for one structural
reason: a chart that a Spore composes at runtime through Lua cannot be a
compile-time `WidgetKind`. `soil-scene-gpu` already draws polylines, areas,
bands, points, heatmaps and candles.

Where the pieces already exist:

| Capability | Exists today | Missing |
|---|---|---|
| Line/area/band/point/heatmap/candle **data** primitives | `soil-scene::Content` = `Polyline, Area, Band, Points, Heatmap, Candles` — the vocabulary is already ahead of every host | instancing, LOD, per-mark shaders |
| GPU rendering of those | `soil-scene-gpu` (`src/live.rs`, `src/lib.rs`) | density/aggregation, picking |
| Custom shader nodes | `soil-scene::Content::Shader` (WGSL + uniforms) | a safety policy — an untrusted Spore's WGSL is a denial-of-service surface |
| GPU-instanced points / density | `fungos.atlas` (Lua spec only) | the pipeline |
| Map primitives | Canvas `canvas-map-wgpu` and `map/globe.rs`; `fungos.map` (Lua spec) | one map pipeline, not two — a decision of the same kind as D1 |
| Graph/edge rendering | `fungos.graph` (Lua), Canvas `activity-graph.lua` | a GPU edge pipeline |
| Text in a GPU scene | `soil-scene-gpu` glyph atlas | shaping (complex scripts), subpixel |
| **3D: mesh, camera, light** | **Canvas only** (`NodeContent::{Mesh, Camera, Light}`) | they are **silently downcast to `Group`** on the Canvas→Soil path |

The asymmetry is the finding, and it is the opposite of what I first wrote:
`soil-scene` has *more* 2D drawing vocabulary than Canvas does (Canvas has zero
path/area/band/point primitives), while Canvas has *all* of the 3D vocabulary and
`soil-scene` has none. `crates/canvas/src/spore_present.rs` therefore degrades
**seven** Soil content kinds to `Group` and **three** Canvas kinds to `Group` —
round-tripping a chart through Canvas and back loses the chart. See §3.1.

The real gap is not "add a Path node"; it is **reconcile two vocabularies that
each grew in a different direction**. That is the `soil-scene` schema epic, and
it must land before any GPU library is ported, or the port gets done twice.

---

## 7. The loop you actually want (agent tweaks Lua, you promote upstream)

The loop is: *app running → agent edits Lua → live surface changes → you like it
→ promote to the Spore repo → maybe RiiR.*

What exists:

- fresh-VM-per-generation Luau sandbox with limits — [`lua_scene.rs`](../crates/canvas/src/lua_scene.rs)
- transactional hot reload that keeps the last good scene — [`lua_module.rs`](../crates/canvas/src/lua_module.rs)
- `tend` preflight (`tend doctor`, `tend check`) before the host ever sees it
- Spore scene socket (`spore_socket.rs`) so a producer in any language can push
- dotfiles-as-projection design: `fungos-sdk/docs/user-repos.md`

What is missing, in dependency order:

1. **One UI library** (D1). An agent cannot reliably mangle two dialects.
2. **Host text measurement**, so a Lua tree stops guessing widths — landed on
   `pi/soil-scene` (`feat(lua): host text measurement and local module hit targets`).
3. **Id-keyed scene merge on the host**, so a reload patches instead of
   rebuilding the page (already listed as host work in `spores/docs/ui-model.md`).
4. **A `~/.config` VFS projection with the same reload semantics in both
   views** — `soil-vfs` exists; the dotfiles projection is design only.
5. **A promote path**: `tend` knows how to preview and compose, not how to take
   the live `~/.config/fungos/apps/inbox.lua` override and land it in the Spore
   repo as a versioned `ui/` change. That is a small, missing script.

Without (1) and (3), the loop works on one app the agent has been taught and
fails on the next.

---

## 8. Baseline actions, in order

1. **Land soil `origin/pi/layout`'s render crates onto `main`.** This is the
   only real blocker. The branch is 45 ahead / 69 behind and is *three* concerns,
   not one: the render foundation (`soil-scene`, `soil-layout`, `soil-input`,
   `soil-vt`, `soil-terminal`, `soil-scene-gpu`, `soil-ui-demo`), the live-viz
   series (14 commits, all editing `soil-scene-gpu/src/live.rs`, unsignable as
   one review), and shell/chrome integration. `git merge-tree --write-tree
   origin/main origin/pi/layout` gives five content conflicts: `Cargo.lock`,
   `Cargo.toml`, `crates/soil-runtime/Cargo.toml`, `crates/soil-spore/src/lib.rs`,
   `wit/soil-canvas.wit`. Transplant the render crates onto a fresh branch from
   `origin/main` rather than rebasing the whole branch; the 69 commits of host
   crates must not be reverted.
2. **Merge the two Canvas lines.** Small: fresh branch from `25121f0`, merge
   `rename/dock-to-canvas` first, then `pi/soil-scene`, resolve
   `crates/canvas/src/main.rs`. Keep `pi/soil-scene`'s `fpl-gfx` pin (`83b7d46`)
   and its `soil-*` path deps. Do not do it in the current checkout — it is
   stale and dirty.
3. **Delete the stale local `rename/dock-to-canvas` label** or re-point it at
   `b2e5f50`. The current checkout misleads anyone who runs `cargo` in it.
4. **Make the sibling checkouts explicit**: one script that establishes
   `../gfx` at `83b7d46`, `../soil` and `../unibus`, plus the vendored patches,
   and fails loudly naming whichever is missing.
5. **Decide D1 and D5**, which are the two that stop further duplication.
6. **Reconcile the two node vocabularies** (§6) before porting any GPU library.
7. **Decide `soil-input` vs `soil-canvas-input`.** They are not duplicates:
   `soil-canvas-input` is a *platform→Canvas key* adapter, `soil-input` is a
   *host-neutral event model* (pointer, scroll, focus, IME, terminal encoding).
   `wit/soil-input.wit` describes neither — it is the canonical data type. Three
   things, one name. Pick the boundary before they drift.

Do not start a rendering refactor on a rename branch — the same warning
`docs/spike-adoption-decisions.md` already carries. And do not rebase the whole
of `pi/layout`: 30 of its 45 commits carry no `Signed-off-by`, and the viz series
is 14 commits of small edits to one file.

---

## 9. Index

**Canvas decision records**
`charting-dependency-decision.md`, `spike-adoption-decisions.md`,
`canvas-shared-visualization.md`, `widget-quality.md`, `lua-lovr-adapter.md`,
`canvas-lua-modules.md`, `computing-mesh-design.md`,
`servo-and-web-surfaces-decision.md`; plus `surface-backends.md` (on
`origin/pi/soil-scene` only).

**Soil records**
`soil-canvas.md`, `soil-webgpu.md`, `abi-map.md`, `s22-guest-frame.md`,
`soil-shell.md`, `soil-vfs-index.md`, `soil-config.md`,
`soil-canvas-s22.md`.

**Spores records**
`docs/host-contract.md`, `docs/ui-model.md`, `docs/surface-windowing.md`,
`docs/window-policy.md`, `docs/content-rendering.md`, `docs/design-tokens.md`,
`docs/components.md`.

**fungos-sdk records**
`docs/viz.md`, `docs/live.md`, `docs/composition.md`, `docs/chart.md`,
`docs/user-repos.md`, `docs/lua-api-contract.md`, `docs/action-bar.md`.

**Code that matters**
`crates/canvas/src/{lua_scene,lua_module,spore_present,spore_socket,spore_catalog}.rs`,
soil `crates/soil-scene/src/lib.rs`, `crates/soil-layout/src/lib.rs`,
`crates/soil-scene-gpu/src/lib.rs`, `crates/soil-webgpu`,
`fungos-sdk/lua/fungos/`, `spores/sdk/ui/ui.lua`.

**Known contradictions to resolve**
- `soil-canvas.md` "Spores do not submit UI trees" vs `spores/docs/ui-model.md`
  "a view returns a tree" vs `surface-windowing.md` "a Spore gets a lease".
- `charting-dependency-decision.md` "do not adopt taffy" vs `soil-layout`
  adopting Taffy. Both are right: Canvas places *windows* absolutely, Soil lays
  out *inside* a surface in flow. Say that out loud in both documents.
- `abi-map.md` `soil:surface@1` "deferred" vs `pi/soil-scene` shipping it.
