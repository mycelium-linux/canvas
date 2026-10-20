# Shared design dependencies

The runtime uses Canvas's single `vendor/coherence-native` and
`vendor/coherence-tokens` snapshots. Coherence provenance and hashes live beside
those packages. Dense controls are implemented in the Coherence owner first.

The separate `canvas-workspace-review` package retains the first-party gfx
snapshot for the offscreen QA harness; the UI and Android runtime do not depend
on that review renderer.
