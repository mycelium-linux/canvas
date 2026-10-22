# Coherence in Canvas GPU widgets

Canvas's iced UI remains the host. `canvas-widgets::live::gfx_theme` translates
the **resolved** `VisualTheme` into Coherence's native token snapshot; the
Coherence-owned gfx adapter maps that snapshot to the GPU theme. Imported
palettes and live Lua overrides are inputs, not replaced by adapter defaults.

```rust
let mut theme = fpl_gfx::Theme::studio_dark();
canvas_widgets::live::gfx_theme::apply(&visual_theme, &mut theme)?;
```

Resolve appearance when it changes and retain the resulting theme with the
renderer. Do not put service discovery, application state, or GPU device
ownership in the appearance adapter. Fonts, motion timing, outline width,
categorical colors, and primitive-specific materials stay host-owned.

The analog clock is the first adopter. Its existing gfx geometry, hands, ticks,
and motion settings are unchanged. Flat fills now use gfx's corrected sRGB
shader path. Invalid geometry produces a visible clock-theme error. The shared
adapter also standardizes recessed surfaces, outlines, highlights, and global
shadow treatment; the clock does not use those changed semantic roles.

Both `vendor/coherence-native` and `vendor/coherence-tokens` are exact artifacts
from Coherence revision `407f86638f5879f687b5fbe4321e897ae67511d3`. Each has a
SHA-256 provenance manifest. They share one token crate, and neither requires
Node or private Git authentication during Canvas builds. Update both artifacts
and their manifests together from an upstream revision, never edit a vendored
palette or adapter locally. The egui adapter is available for independent apps;
Canvas enables only gfx and does not adopt egui as its GUI toolkit.

Verification on Agora:

```sh
cargo check -p canvas-widgets --features live,iced/x11
cargo test -p canvas-widgets --features live,iced/x11 --lib
cargo test -p canvas-core --lib
```

`iced/x11` supplies the platform backend for standalone Linux widget tests;
the Canvas application already enables its platform backends.

Source adoption does not imply fleet deployment. Reconcile the installed
compositor/source revision before replacing a Neo, Studio, or Home Pi binary.
