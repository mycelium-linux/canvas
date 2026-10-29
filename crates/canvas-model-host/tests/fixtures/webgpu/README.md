# `soil:webgpu` guest fixture

The fixture is a `wasm32-wasip2` component built from this directory:

```sh
cd crates/canvas-model-host/tests/fixtures/webgpu/guest
cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/soil_webgpu_guest.wasm ../soil-webgpu-guest.wasm
```

The guest imports `soil:webgpu/webgpu@1.0.0`, creates a texture, shader, and
pipeline through Soil-issued handles, draws a triangle, reads the pixels back,
and returns their byte checksum. `tests/soil_webgpu.rs` asserts the draw
succeeds with `soil:webgpu` + `soil:webgpu.shaders`, that shader compilation is
denied without the shader authority, and that an ungranted component has no
imports.
