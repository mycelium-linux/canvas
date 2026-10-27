# canvas-model-host

Theme-native GPU model renderer and Canvas video producer. This README is
about the **scene** half: the bounded Lua and WebAssembly paths that turn an
asset listing into a validated `SceneState`.

Slice 1 runs a `.wasm` scene as a pure `assets JSON -> scene JSON` function
under wasmtime with **no host imports**. It cannot read the filesystem, the
clock, the network, or the GPU. The host enumerates assets, hands the result
in, and validates the scene with the same `SceneState` type and rules as the
Lua path.

## One runtime

This crate is the single Canvas/Soil wasmtime host: core no-import scene modules
and the component-model Soil capability path (`--features soil-ble`) share the
same `wasmtime 34` dependency. `canvas-widgets` no longer carries a second
wasmtime dependency. The Marbles view host uses the same wasmtime version and
the same no-import core-wasm shape.

## Prerequisites

- A Rust toolchain with `cargo` (the workspace uses edition 2024).
- Nothing else for scene evaluation. The committed wasm fixtures are compiled
  from WAT by the `wat` crate during the build/test cycle, so you do **not**
  need `wat2wasm`, a `wasm32` target, or wasm-bindgen.
- `ffmpeg` only for the streaming (video producer) invocation.

## Build and test

From the repository root:

```sh
cargo build -p canvas-model-host
cargo test -p canvas-model-host
cargo clippy -p canvas-model-host --all-targets -- -D warnings
```

The scene tests are in `tests/wasm_scene.rs`. They take a few seconds because
the epoch-timeout test deliberately lets a runaway module hit its one-second
deadline.

### Build the wasm fixtures from source

The committed `.wat` files are the source of truth; the `.wasm` files next to
them are generated artifacts kept in the tree so tests and CLI examples need no
extra tools. Regenerate both fixtures with:

```sh
cargo run -p canvas-model-host --example build-wasm-fixtures
```

That compiles `tests/fixtures/scenes/*.wat` into `*.wasm` using the `wat`
crate. To add a fixture, write a `.lua` and a `.wat` that express the same
scene, add its name to `examples/build-wasm-fixtures.rs`, regenerate, then
write a golden with the scene bytes (see the parity section).

## CLI

### Scene shape (Lua and wasm)

```sh
canvas-model-host (wasm|lua) scene <scene-path> --assets <asset-dir> [--fuel <n>] [--json]
```

- `--assets <dir>` is required. The host enumerates it and passes the listing
  to the scene as `assets.steps` / `assets.latest`. The scene decides which
  mesh to show; the host never lets it read the directory itself.
- `--fuel <n>` applies to the wasm runtime (default `50_000_000`). It is
  accepted for Lua too so the invocation shape is identical; Lua keeps its
  fixed 64 KiB script cap and instruction budget.
- `--json` prints one-line canonical JSON (the exact bytes the golden tests
  compare). Without it the same schema is printed pretty.
- Exit codes are the same for both runtimes: `0` success, `2` bad invocation,
  `1` runtime or validation failure.

Examples, using the committed fixtures:

```sh
canvas-model-host wasm scene \
  crates/canvas-model-host/tests/fixtures/scenes/field-orbit.wasm \
  --assets crates/canvas-model-host/tests/fixtures/assets --json

canvas-model-host lua scene \
  crates/canvas-model-host/tests/fixtures/scenes/field-orbit.lua \
  --assets crates/canvas-model-host/tests/fixtures/assets --json
```

Both print the bytes in `tests/fixtures/scenes/field-orbit.golden.json`.

The wasm path also logs its accounting to stderr (it is not part of the JSON
schema, so Lua and wasm stdout stay byte-identical):

```text
canvas-model-host: wasm scene ... fuel=6091 elapsed_ms=3.74
```

### Streaming shape (compatibility)

The original eight-argument form is unchanged and still dispatches by scene
extension:

```sh
canvas-model-host <scene.lua|scene.wasm> <asset-root> <endpoint> \
  <width> <height> <fps> <videotoolbox|nvenc|vaapi> <stream-id>
```

## Fail-closed error table

Everything below is an error with a non-zero exit. There is no Lua fallback
and no partial scene.

| Condition | Error (stderr contains) | Exit | What to do |
| --- | --- | --- | --- |
| Fuel exhaustion | `exhausted its <n> fuel budget` | 1 | The scene loops or does more work than a scene should. Fix it or raise `--fuel` deliberately. |
| Memory cap | `exceeded its 8388608-byte memory limit` | 1 | A scene may use at most 8 MiB of linear memory. Stop growing the heap. |
| Epoch timeout | `timed out (epoch deadline:` | 1 | Wall-clock backstop (~1 s). A scene must finish far faster than a frame. |
| Bad magic | `has bad magic` | 1 | The file is not a core wasm module. Rebuild it from `.wat`/Rust. |
| Wrong ABI | `ABI is incomplete: ... export` | 1 | Export `memory`, `alloc(i32)->i32`, and `scene(i32,i32)->i32`. |
| Oversized module | `module is too large` | 1 | Scene modules are capped at 2 MiB. |
| Host imports | `must import nothing` | 1 | Scenes get no host surface. Remove the import. |
| Invalid JSON | `decode wasm scene JSON result` | 1 | The `scene` export must return the `u32 LE length + UTF-8 JSON` block. |
| Invalid scene | `validate wasm scene` | 1 | Same `SceneState` rules as Lua (safe ids, `.stl` asset, bounded values). |
| Malformed result block | `returned a malformed result` | 1 | The returned pointer/length must lie inside linear memory. |
| Bad invocation | usage text | 2 | Fix the arguments; this is not a scene failure. |

## Parity and the golden files

`tests/fixtures/scenes/{field-orbit,follow-latest}.{lua,wat,wasm}` are the
fixture pairs. `tests/fixtures/assets/step-{0,1,2}.stl` are the shared asset
listing. For each pair the test runs both runtimes, serializes each
`SceneState` with `serde_json::to_vec`, asserts the two byte strings are equal,
and asserts they equal the committed golden:

- `tests/fixtures/scenes/field-orbit.golden.json`
- `tests/fixtures/scenes/follow-latest.golden.json`

A behavioural drift therefore shows up as a diff, not as "looks the same".
Regenerate a golden only when the scene contract intentionally changes, and
review the diff.

## Performance

Re-runnable measurement (median of 21 warm runs on this machine, macOS, debug
`cargo test`; wasm includes engine creation and module compilation per
evaluation, Lua includes VM creation per evaluation):

```sh
cargo test -p canvas-model-host --offline --test wasm_scene \
  reports_performance -- --nocapture
```

```text
perf fixture=field-orbit runs=21
  lua_median_ms=0.732
  wasm_median_ms=15.416   (wasm report elapsed_ms=3.739, fuel=6091)
  ratio=21.1x
```

**Wasm is more than 2x the Lua scene time on this fixture** (about 21x
end-to-end; about 5x if you compare the wasm report's own elapsed time). The
gap is dominated by creating an engine and compiling the module for every
evaluation, not by scene execution: the scene itself burns ~6k fuel. This is
acceptable for a change-time/evaluation path, but do not put per-frame wasm
evaluation on the render loop without caching the compiled module first.

## Worktrees: no sibling checkouts, no hand-made symlinks

A fresh worktree used to need `<repo>/.worktrees/unibus` and
`<repo>/.worktrees/gfx` (or sibling checkouts) because several manifests used
`../../../gfx` / `../../../unibus/...` path dependencies, and the workspace
patched `fpl-gfx` to `../gfx`. That made "clean worktree" mean "someone ran
`ln -s` first".

Now every ecosystem dependency is pinned by git revision:

- `fpl-gfx` comes from `https://github.com/ajmwagar/gfx.git` at
  `08cacbe245867d487f5ac1b44da9ae49d96e1890` (the rev that provides the
  `schema` feature `canvas-workspace` asks for). The `[patch]` block is gone;
  the workspace dependency has `default-features = false`, and crates opt in
  to `wgpu` explicitly.
- `stream-descriptors` and the `unibus-*` crates come from
  `ssh://git@github.com/FuturePresentLabs/unibus.git` at
  `318424cdf91c6eebdd2024b779771c225e83025c` (the first rev that carries
  `crates/stream-descriptors`). The existing `unibus` pin stays at
  `b47a6cc...`, so canvas-core's wire contract is unchanged.
- The vendored `coherence-native` names the same `fpl-gfx` rev so the graph
  has one gfx identity, not two incompatible copies. Its `provenance.json`
  checksum is updated to match the dependency-only manifest edit.

`fpl-gfx` moved from per-crate path dependencies to `workspace = true` in the
crates that used `../../../gfx`. The only non-model-host manifest changes are
those dependency declarations plus the workspace root: no crate was renamed,
moved, or restructured. A clean worktree now builds with no scaffolding:

```sh
git worktree add .worktrees/clean-check -b tmp/clean-check <base>
cd .worktrees/clean-check
cargo test -p canvas-model-host --offline
```
