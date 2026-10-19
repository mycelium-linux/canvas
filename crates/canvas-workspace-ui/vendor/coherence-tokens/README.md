# coherence-tokens

Generated FPL Coherence design tokens for native UI, with no dependencies.

Values are generated from `config/product-theme.css` during Coherence's build.
Do not hand-edit `src/generated.rs` — regenerate it with `npm run build` in the
Coherence repository, which also asserts the contrast floors every theme must
clear.

```rust
use coherence_tokens::{light, dark, sepia};

let ground = sepia::COLOR_BACKGROUND;   // [u8; 3]
let text   = sepia::COLOR_TEXT;
```

Modules: `light`, `dark`, `sepia`, `compact`.
