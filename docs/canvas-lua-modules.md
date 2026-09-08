# Canvas Lua modules

Canvas Lua modules turn typed host data into small retained scenes. A module is
a manifest and Luau source. Rust loads it with an input snapshot, the active
project's Coherence tokens, and optional opaque resource references. The source
runs once for `load`, for explicit data or interaction events, and for a
candidate hot reload. Canvas retains the last successful scene between calls.
It does not run Lua implicitly in the render loop.

The manifest explicitly lists readable input IDs, whether theme tokens are
visible, and which declared host resources may be presented. Unknown manifest
fields fail parsing. There are no filesystem, network, process, package, native
module, clock, random, GPU, media decode, or workspace mutation capabilities.
The same Luau sandbox, memory ceiling, interrupt budget, source limit, asset
grants, and combined node limit apply to every lifecycle call.

Inputs are one of number, text, boolean, finite point series, or status. The
host assigns monotonically increasing snapshot revisions and invokes modules
with typed `load`, `data_changed`, `activate`, `pulse`, or `reload` events.
Pulse time is supplied by the scheduler; scripts cannot read a clock. A failed
update or reload leaves the prior retained output and generation untouched.

`module.theme` is derived at invocation time from `canvas_core::ProjectTheme` and
therefore from the vendored Coherence tokens. Lua receives semantic colors and
metrics without owning or duplicating a palette. See the executable
[`graph.lua`](../crates/canvas/examples/lua/graph.lua) and
[`pnl-status.lua`](../crates/canvas/examples/lua/pnl-status.lua) examples.

Video/NVR and maps enter as `video_stream` and `map_viewport` references. A
reference contains only stable owner, ID, kind, and contract data. Lua can place
an explicitly granted reference with `canvas.host_resource`; it never sees an
endpoint, credential, decoded frame, map tile, texture, or GPU handle. A native,
WebGPU, or future LÖVR adapter asks the Rust host to resolve and render it.

Hot reload is transactional: Canvas validates the candidate manifest, renders
the candidate against the current snapshot in a fresh VM, and swaps source,
manifest, generation, and retained scene only after the complete result passes
validation. This keeps the runtime deterministic and lets project/workspace
documents remain authoritative.

Within the [shared visualization plane](canvas-shared-visualization.md), Lua is
an adaptive reducer and presentation layer for typed source contracts. It does
not create a second view state: viewport, attention, navigation, filters, source
position, and revision remain in the Rust-owned shared view.
