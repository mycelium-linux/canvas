# Canvas Customization How-To Briefing

## What is Canvas

`canvas` is a Rust + iced (WebGPU/Metal) GUI that renders **workspace documents** — versioned JSON specs describing surfaces, layouts, backgrounds, themes, and live data bindings. It's controlled via an owner-local **Unix domain socket** (`~/Library/Application Support/Dock/canvas/control.sock`).

Canvas runs as a borderless, multi-display daemon. It is the visualization/presentation layer, not the project-selector GUI.

---

## Starting Canvas

```sh
cargo run -p canvas --features lua-scenes
# Or use the pre-built binary:
./target/debug/canvas
```

### Prerequisites on macOS

- `scripts/install-mac-control.sh` for display geometry and window control
- Global hotkeys: Ctrl+Option+C (toggle), Ctrl+Option+J (JARVIS), Ctrl+Option+? (help)
- Stream socket for live data: `~/Library/Application Support/Dock/canvas/stream.sock`

---

## Controlling Canvas

### Text mode (canvasctl)

```sh
echo 'inspect'           | canvasctl    # quick status
echo 'displays'          | canvasctl    # list connected displays
echo 'hide'              | canvasctl    # hide all windows
echo 'show'              | canvasctl    # show windows
echo 'shutdown'          | canvasctl    # exit daemon
echo 'bg-shader studio smoke-rings 0.5' | canvasctl  # set background shader
echo 'shader 3'          | canvasctl    # set screensaver shader
```

### JSON mode

```sh
echo '{"schema_version":1,"request_id":"doc","command":{"command":"read_document"}}' | canvasctl
```

The `command` field is internally tagged — nest the command tag inside.

---

## Customization Reference

### 1. Backgrounds (5 kinds)

| Kind | JSON op | Description |
|---|---|---|
| Theme | `set_background: {kind:"theme"}` | Coherence palette (default) |
| Solid | `set_background: {kind:"solid", color:[r,g,b,a]}` | Flat RGBA |
| Gradient | `set_background: {kind:"gradient", ...}` | 2-8 ordered stops |
| Image | `bg-image <path>` text command | Cover/contain/stretch/center |
| Shader | `bg-shader <project> <shader> [speed]` | 7 animated WGSL shaders |

**Shader IDs:** `tidepool`, `aurora-drift`, `dusk-ember`, `slow-ripple`, `spirales`, `floating-rectangles`, `smoke-rings`. Speed: 0.0 (frozen) to 4.0.

Reset: `echo 'bg-clear' | canvasctl`

### 2. Chrome

```sh
echo 'titles hidden'     | canvasctl   # Hidden / Focused / Always
echo 'spacing 24 12'     | canvasctl   # canvas_padding surface_gap (px of air)
```

`spacing` sets how much breathing room sits at the wall edge and between
neighbouring surfaces. New documents default to a humane `24 / 12`; an
explicit `0 0` returns to flush, welded edges.

### 3. Layouts

```sh
echo 'tile grid 2 2'     | canvasctl   # explicit rows x cols grid
echo 'tile golden'       | canvasctl   # phi:1 mosaic, prime cell first
echo 'tile monocle'      | canvasctl   # stack all surfaces, one fills the wall
echo 'tile next'         | canvasctl   # cycle focus through the stack
```

`tile golden` and the fresh-stack default share one deterministic algorithm
(`canvas::compose`): repeatedly split the largest cell at the golden ratio, so
the first surface claims the prime region and every surface gets a distinct,
non-overlapping part of the wall. Composition (`tile`) sets *where* surfaces
land; spacing (`spacing`) sets *how much air* surrounds them — independent axes.

### 4. Surface Content Types

| Kind | How to create |
|---|---|
| Widget | `open_widget <spec.json> <surface_id>` |
| Image/PDF/Markdown | `open <surface_id> <file_path>` |
| DataViz | `open_dataviz <type> <surface_id> <file>` |
| Scene | Via document (3D node graph) |
| Stream | `open_stream <url> <surface_id>` |

### 5. Themes

```sh
echo 'bg dock'         | canvasctl    # dark / info accent
echo 'bg studio'       | canvasctl    # light / brand accent
```

### 6. Overlays (6 targets)

```json
{"command":{"command":"set_overlays","overlays":{
  "presence_rail":"leftmost", "presence_preview":"leftmost",
  "presence_orb":"hidden", "system_hud":"rightmost",
  "fpl_mark":"center", "shortcut_help":"all"
}}}
```

### 7. Status Indicators (16 max)

```sh
echo 'indicator cpu ready "CPU: 23%"' | canvasctl
echo 'indicator agent off' | canvasctl
```

### 8. Mesh (8 nodes max)

```json
{"command":{"command":"set_mesh","mesh":{"nodes":[
  {"id":"agora","label":"Agora GPU","state":"active"},
  {"id":"cft","label":"CFT Worker","state":"paused"}
]}}
```

### 9. Presence

```json
{"command":{"command":"set_presence","presence":{"zone":"desk-north",...}}}
```

Toggle JARVIS overlay (face preview, presence rail): **Ctrl+Option+J**

### 10. Data Bindings

```jsonc
"surfaces": [{ "id": "meter", "inputs": [{"id": "cpu-load", "contract": "metrics.cpu-load.v1"}] }]
"bindings": [{
  "id": "cpu-to-meter",
  "source": {"owner": "system", "stream": "cpu", "contract": "metrics.cpu-load.v1"},
  "target": {"surface_id": "meter", "input_id": "cpu-load"}
}]
```

---

## Keyboard Shortcuts

| Key | Action |
|---|---|
| **F2** | Choose background — live shader picker grid |
| **F3** | Toggle diagnostics HUD |
| **F4** | Choose theme — Coherence palette selector |
| Ctrl-P | Find / open a project |
| Ctrl-0 | Show on every display |
| Ctrl-Option-L | Snap layout to saved view |
| Ctrl-Option-F | Fit layout to displays |
| Ctrl-Option+C | Toggle Canvas visibility |
| Ctrl-Option+J | Toggle JARVIS presence overlay |
| ? | Toggle shortcut help (Canvas focused) |
| Ctrl-Option+? | Toggle shortcut help (global) |

### F2: Background Picker

Opens a 4×2 grid of all 7 WGSL shaders running live. Arrow keys navigate, `+`/`-` adjust speed (7 rungs: 0.0-4.0×), Enter applies, Escape cancels. Opens on the current shader.

### F4: Theme Picker

6 Coherence combinations: Dark/Light × Brand/Info/Success. Arrow keys navigate, Enter applies, Escape cancels.

### F2 ≠ face preview

F2 shows **backgrounds**. Face preview (JARVIS presence) is via **Ctrl+Option+J** + `ShowPresencePreview` socket command.

---

## Briefing System (dock-ybc.2, dock-ybc.25.1)

A briefing is a paced, seekable presentation over the whole Canvas — scripted control + TTS, authored as a Lua artifact.

### Architecture

```
Producer (Python) → UDP datagram → StreamInbox (retained log) → Lua binder → BriefingBeat → Canvas patches + TTS
```

### Components

| Component | Path |
|---|---|
| Briefing binder | `config/briefings/intro-v1.lua` |
| Producer script | `scripts/canvas-briefing-run.py` |
| Briefing module | `crates/canvas/src/briefing.rs` |
| Stream binding | `crates/canvas/src/stream_binding.rs` (`apply_briefing_binder`) |
| Lockout widget binder | `config/binders/lockout-v1.lua` |
| Lockout producer | `scripts/canvas-lockout-feed.py` |

### Running

```sh
# Start Canvas, then run the intro briefing (6 beats, 5s apart)
python3 scripts/canvas-briefing-run.py --briefing intro-v1 --every 5

# Single beat
python3 scripts/canvas-briefing-run.py --beat 3

# Lockout table widget
python3 scripts/canvas-lockout-feed.py --every 2
```

### BriefingBeat format

```jsonc
{
  "beat": 2,
  "narrate": "What would you like to work on today?",
  "wait_seconds": 3.0,
  "patches": [
    {"op": "set_background", "background": {"kind": "shader", "shader_id": "smoke-rings", "speed": 0.6}}
  ],
  "done": false
}
```

---

## Theme Switcher (dockctl)

```sh
cargo run -p dock-core --bin dockctl -- theme dock     # dark / info
cargo run -p dock-core --bin dockctl -- theme studio   # light / brand
cargo run -p dock-core --bin dockctl -- projects       # list
```

Coherence tokens (20 colors per theme) in `design/coherence/coherence-{dark,light}.tokens.json`.

---

## Building

```sh
cargo build -p canvas --features lua-scenes --bin canvas --bin canvasctl
cargo test -p canvas --features lua-scenes
cargo test -p canvas-widgets --all-features
cargo clippy -p canvas -p canvas-widgets --all-features --all-targets -- -D warnings
```

### Test results (refactor branch)
- canvas-widgets: 57 tests
- canvas: 179 lib + 72 main + 5 briefing = 256 tests

---

## File Locations

| Path | Purpose |
|---|---|
| `~/Library/Application Support/Dock/canvas/control.sock` | Owner-local control socket |
| `~/Library/Application Support/Dock/canvas/stream.sock` | UDP stream socket (DGRO) |
| `~/Library/Application Support/Dock/canvas/workspaces.json` | Persisted workspace documents |
| `~/Library/Application Support/Dock/canvas/runtime.lock` | Pid-based lock |
| `config/projects.json` | Project catalog |
| `config/widgets/` | Widget specs |
| `config/binders/` | Lua stream binders (lockout-v1, etc.) |
| `config/briefings/` | Lua briefing scripts (intro-v1) |
| `scripts/canvas-briefing-run.py` | Briefing producer |
| `scripts/canvas-lockout-feed.py` | Lockout feed producer |
| `design/coherence/` | Coherence theme tokens |
