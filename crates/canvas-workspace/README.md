# Shared Canvas workspace model

Renderer-neutral Rust presentation intent for fungOS clients. Depends on
`canvas-core` policy/layout and the non-live widget model, not iced or wgpu.
Desktop's `canvas` type, chart and view paths are compatibility re-exports.
Android's workspace adapter re-exports `session::Workspaces`.

This crate owns:

- Workspace documents, surfaces, backgrounds, theme references and pages.
- Scene nodes, transforms, semantic ink, cameras and lights.
- Bounded chart and widget descriptions.
- Revision-bound document and shared-view patches, validation and JSON compatibility.
- Owner-provided catalog selection and retained per-project document sessions.

Platforms own storage, pairing/authentication, transport, input, decoder lifetimes,
native windows and GPU allocations. Installing a document never fetches a URL,
launches an application or grants an agent control. Sessions must be created from
the selected owner's catalog; replacing owners requires replacing the session.
Document/patch ingestion is for already-admitted owner projections, not an
unauthenticated command endpoint.

Existing JSON keys, defaults and schema versions remain unchanged. Unknown
future surface/background kinds are retained verbatim; invalid known kinds
still fail. Stale or conflicting complete generations and invalid patches leave
the last valid document intact. The `schema` feature enables protocol schemas.

Check with `cargo check -p canvas-workspace --features schema`, then test with
`cargo test -p canvas-workspace --features schema`. Mobile platforms can depend
on this crate without enabling `canvas-widgets/live`.
