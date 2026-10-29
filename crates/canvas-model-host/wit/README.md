# Soil contracts: `soil:gpu@1.0.0`, `soil:audio@1.0.0`, and `soil:ble@1.0.0`

These WIT packages are **defined, not shipped**. They describe the slice-2
boundary between a wasm app and the native Canvas host; no import is linked
today. The host is the PDP: an import becomes visible only when the app
manifest requests the capability, the grant exists, and host policy allows the
intersection (see `WASM-APP-ECOSYSTEM` and `HOST-CAPABILITIES`).

Canvas remains native T0. wgpu, VideoToolbox, and the audio sink stay in the
host; wasm only supplies a bounded draw list or bounded PCM. There is no
filesystem, network, clock, or device-enumeration surface expressible here:
meshes are content identifiers (CIDs), the audio sink is host policy, and every
error is a per-interface capability result.

`soil:ble@1.0.0` adds the BLE transport seam. The host owns the radio and the
lease; a Spore sees eight verbs, Soil-issued device handles, typed errors, and
no MAC, path, socket, clock, or ambient filesystem/network authority. The
capability strings are `soil:ble.observe`, `soil:ble.connect`, and
`soil:ble.write`; the ring protocol stays in the guest.

Component fixtures are generated from WIT, never hand-written. The
`tests/fixtures/ble/soil-ble.wasm` fixture is built by the guest crate in
`tests/fixtures/ble/guest` and `wasm-tools component new`; the example
`build-wasm-fixtures` runs both.

`conformance.json` is the machine-checked drift guard: the tests resolve the WIT
surface and compare function signatures structurally (parameter names and
resolved type names, plus the `result` ok/err types), so a type drift fails even
when the function name survives. It also pins the typed error enum and the
`event` variant cases for `soil:ble`.

Deliberate omissions:

- No `world`: slice 2 grants interfaces per manifest, and no instantiable app
  world is defined yet. Adding one here would imply a composition contract the
  host does not implement.
- No `deps/` directory: each package is self-contained so neither capability
  depends on the other.
- No shader bytes, raw GPU buffers, callbacks, device enumeration, or paths.

## `soil:webgpu@1.0.0`

`soil:webgpu` is the WebGPU-shaped capability. The host owns the wgpu adapter,
device, queue, and presentation surface; a Spore receives typed handles and
descriptors, never raw GPU objects or a swapchain. The first linked slice is
`adapter` plus `render-probe`, which renders an offscreen triangle and returns
bounded RGBA8. It proves WIT -> wasmtime -> wgpu before the resource API
(buffers, textures, pipelines, render passes) is added.

Like the other Soil packages this file is defined, not shipped: it is linked
only when the manifest requests `soil:webgpu`, the principal grants it, and
host policy allows the intersection. Shader bytes will be a separate
`soil:webgpu.shaders` authority; the probe uses a host-owned shader.

Host-side implementation and feature gate: `src/webgpu.rs` and the
`soil-webgpu` feature in `Cargo.toml`.
