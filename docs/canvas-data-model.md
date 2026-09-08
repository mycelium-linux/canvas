# Canvas data model

Canvas treats presentation as a versioned document. The document says what a
project wants to show; native, WebGPU, browser, and future XR adapters decide how
to render it. Switching projects means validating and applying another document,
not reconstructing a desktop with pixel scripts.

Canvas is also the shared visualization plane between a person and agents. A
revisioned joint view records the exact viewport and attention state both sides
observe. See [Canvas shared visualization plane](canvas-shared-visualization.md)
for the source, view, streaming, Lua, and agent-capture boundaries.

`dock-canvas` owns these durable facts:

- the Alfalfa project and resource identities being presented;
- surfaces and their portable split, stack, or normalized freeform layout;
- a reference to the project's Coherence-derived theme;
- typed 3D scene nodes and immutable SHA-256 asset references;
- live input ports and schema-matched external data bindings; and
- an ordered document revision with atomic incremental patches.

The crate deliberately stores no decoded images, video frames, PDF pages,
terminal processes, native window handles, animation clocks, pose samples,
framebuffers, or GPU allocations. Those are bounded, reconstructible runtime
state. A terminal surface can only refer to a managed terminal or agent session
already present in the project catalog; scene data cannot launch a process.

Every renderer consumes the same validated `WorkspaceDocument`. Theme colors
continue to come from `dock-core` and the vendored Coherence tokens, so Canvas
does not create a second palette. A `WorkspacePatch` names its workspace, base
revision, and next revision. Dock applies its operations to a copy and validates
the complete result before publishing it, which keeps a failed patch from
partially changing the visible workspace.

Live bindings also stay data-oriented without coupling Canvas to a producer's
internals. A surface declares a named input and contract, while a binding names
the owning system, stream, and the same contract. JARVIS presence and attention,
for example, can flow into a surface only through an explicitly compatible input.
Dock remains responsible for authorization, freshness policy, placement, and
human takeover.
