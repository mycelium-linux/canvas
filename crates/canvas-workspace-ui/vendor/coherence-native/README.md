# coherence-native

One Coherence token snapshot for independent native renderers. No Canvas,
windowing, service, credentials, or application-state dependency.

```rust
use coherence_native::{Density, Mode, Tokens};
let tokens = Tokens::new(Mode::Dark, Density::Compact);
tokens.validate()?;
# Ok::<(), coherence_native::InvalidMetrics>(())
```

Enable only what your app uses: `gfx`, `egui`, or both. Default features have no
renderer dependencies. The gfx adapter uses gfx with GPU features disabled;
theme-only clients do not pull in wGPU. The egui adapter targets 0.33.3. The gfx
dependency is pinned to a reviewed revision. No package release is claimed;
consume this crate by a pinned Coherence Git revision or a local path.

```rust,ignore
// On appearance changes, not in the draw loop:
coherence_native::gfx::apply(&tokens, &mut host_gpu_theme)?;
coherence_native::egui::apply(&tokens, &mut host_gui_style)?;
// Or ctx.set_style(coherence_native::egui::style(&tokens)?);
```

`Tokens::new` resolves generated `coherence-tokens` values from the canonical
product profile. It supports light, dark, sepia and both densities. Public typed
fields permit host overrides (including a Canvas/Lua-provided palette) without
I/O in the adapter. Both adapters validate before mutation; bad overrides leave
the previous renderer state intact. This is a Rust presentation view, not a new
wire schema. External GPU apps can keep using gfx's existing versioned serialized
`Theme` contract, validated before upload.

## Mapping

| Meaning | gfx | egui |
| --- | --- | --- |
| Ground/surface/raised/well | Semantic color roles | Panel/window/widget fills |
| Text/muted/status/accent | Semantic color roles | Text, links, warning/error colors |
| Hover/active/focus | Host interaction chooses roles | Explicit widget states and focus outlines |
| Corners | Small/large shape tokens | Widget/menu/shell radii |
| Typography/density/spacing | Host reads `Tokens.metrics` | Text styles, padding, margins, target sizes |

gfx colors are converted from authored sRGB to linear light exactly once using
`Color::from_srgb8`. egui receives authored sRGB bytes via `Color32::from_rgb`.
Coherence's product profile is flat: no shadows or global gloss/grain/emission.
Explicit per-primitive material overrides remain owned by the rendering app. GPU
motion and outline width remain host-owned because the product profile does not
export those tokens. egui behavior, animation timing and custom text styles are
preserved. The egui state adapter maps visual fields rather than replacing the
host's whole behavior configuration.

The font stack is metadata, **not font installation**. egui uses its registered
proportional/monospace families; hosts must load/register Geist if required.
`row_height`, `line_height`, large/display text sizes remain explicit metrics for
custom layouts because neither gfx's theme nor egui's global style has a full
equivalent. A palette adapter is not a complete native component library.

## Verify / preview

```sh
cargo check --manifest-path crates/coherence-native/Cargo.toml --all-features --all-targets
cargo test --manifest-path crates/coherence-native/Cargo.toml --all-features
cargo clippy --manifest-path crates/coherence-native/Cargo.toml --all-features --all-targets -- -D warnings
cargo run --manifest-path crates/coherence-native/Cargo.toml --features gfx,egui --example shared-theme
```

For actual egui and GPU rendering side by side, use the isolated preview package:

```sh
cargo run --manifest-path crates/coherence-native/preview/Cargo.toml -- /tmp/coherence-native.png
```

The preview requires a working Vulkan/Metal GPU (software Vulkan also works),
does not open a window or touch a running Canvas, and writes an opaque PNG.
Its dependencies are isolated so normal adapter builds stay small.
The preview checks ground/panel pixel values against canonical sRGB and checks
every output pixel is opaque before saving. On Agora's RTX 3060, each themed
instrument batch contains 69 retained primitives in one draw call; this is a
rendering-contract proof, not an FPS benchmark.
