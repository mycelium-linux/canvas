# Dock — First Cut (v0 build doc)

*Scope: one night's build. This is deliberately smaller than `computing-mesh-design.md`
— it's the Phase 0/1 slice of that doc, made concrete enough to actually type code
against tonight. Nothing here is meant to survive unchanged once the real
orchestration core exists; the goal is working, disposable-in-a-good-way scaffolding
that proves the pattern.*

**Design language:** all visual styling (dock chrome, colors, spacing) comes from
**Coherence**, the shared design language already running in the design tool.
Both docks should consume Coherence's token output rather than hand-rolled colors —
exact pipeline (which artifact each platform reads) is still being figured out;
see Open Items.

---

## 1. Streaming (Linux → Mac) — do this first

The one piece with external setup/pairing friction, so front-load it.

- [ ] Install/confirm **Sunshine** running on the Linux box.
- [ ] Pair **Moonlight** on the Mac over the **Tailscale IP**, not local LAN —
      proves the real end-state (remote-reachable) immediately, not just
      same-room.
- [ ] Get **one** monitor streaming full-screen-in-a-window first. Confirm it
      feels good before touching multi-monitor.
- [ ] Add the other two monitors, position manually. This is a one-time,
      persisted setup cost — not a bug to chase tonight.

**Non-goal tonight:** auto-arranging all three monitors on connect. Known to not
be turnkey in Moonlight/Parsec; revisit later if it's still annoying.

---

## 2. Mac dock — first cut

**Shape:** `NSStatusItem` menu-bar dropdown (SwiftUI). Not a custom-drawn floating
dock — fastest real thing to ship tonight.

**Contents:**
- **Project switcher** — list of profiles (start with 2: default + PedalKernel).
  Each row built with SwiftUI gesture modifiers (see §5), not a bare button.
- **Colorway picker** — reads Coherence token output; ship with 2 real colorways
  tonight, not placeholders.
- **Streaming status** — manual toggle reflecting the Moonlight session from §1.
  Doesn't need real plumbing yet, just needs to be present and visually correct.
- **JARVIS row — reserved, greyed out, disabled.** JARVIS is already in flight;
  building its slot into the layout now means the dock doesn't get restructured
  when it lands, only un-greyed.

---

## 3. Sway dock — first cut

**Shape:** Waybar custom modules, same pattern as the existing `mediaplayer.py`/`.sh`
scripts.

**Contents:**
- **Project switcher module** — lists profiles, click fires the profile's launch
  script (workspace + apps + placement, per `computing-mesh-design.md` §2/§6).
- **Colorway module** — click swaps the active Waybar CSS variant.
- **JARVIS placeholder icon** — static, disabled, same reserved-slot logic as
  the Mac side.
- Styled from the **same Coherence token output** as the Mac dock, so the two
  read as one system, not two independently-themed tools.

---

## 4. Project profiles & colorways — the glue

Minimal artifacts tonight, not the full schema:

- One YAML/TOML file per profile: workspace/session name, app list + working
  directories + launch placement, colorway name. Two profiles (default,
  PedalKernel) is enough to prove the pattern end-to-end.
- Colorway tokens: a single source (from Coherence) feeding both the Mac
  SwiftUI color scheme and the Waybar CSS. This is the one piece worth getting
  right tonight even if everything else is rough — both docks depend on it,
  and it's the most annoying thing to retrofit later.

---

## 5. Gesture-readiness (not gestures)

Gestures themselves are explicitly out of scope tonight. What's in scope:
building the interaction model so a gesture input source can be dropped in
later without a UI rewrite.

- Mac dock rows use `DragGesture` / `MagnificationGesture` wired to trackpad
  input now — gesture-shaped interactions, not button-shaped ones.
- The point: a future hand-tracking classifier feeding synthetic
  pointer/gesture events is an **input-source swap**, not a UI change, *if*
  the interaction model is already gesture-shaped tonight.

---

## 6. Explicit non-goals tonight

- No gesture recognition (see §5 — readiness only).
- No JARVIS logic — reserved UI slot only, both docks.
- No 3-monitor auto-arrangement.
- No full project-profile schema from the main design doc — 2 profiles, minimal
  fields, enough to prove the shape.
- No custom compositor work — Sway stays stock, driven via existing IPC/module
  patterns.

## 7. Open items (figure out later, not tonight)

- Exact Coherence pipeline: which artifact does each platform actually consume
  (compiled CSS? a raw token JSON? something else per-platform)?
- Where project-profile files live long-term (this doc assumes local files
  tonight; may want them Coherence-managed too).
- How the JARVIS row/icon actually activates once JARVIS lands — likely a
  tier-1 dispatch call per `computing-mesh-design.md` §5, not decided here.
