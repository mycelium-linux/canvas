# Slice-2 contracts: `fleet:gpu@1.0.0` and `fleet:audio@1.0.0`

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

`conformance.json` is the machine-checked drift guard for slice 2: the tests
resolve the WIT surface and compare function signatures structurally (parameter
names and resolved type names, plus the `result` ok/err types), so a type drift
fails even when the function name survives.

Deliberate omissions:

- No `world`: slice 2 grants interfaces per manifest, and no instantiable app
  world is defined yet. Adding one here would imply a composition contract the
  host does not implement.
- No `deps/` directory: each package is self-contained so neither capability
  depends on the other.
- No shader bytes, raw GPU buffers, callbacks, device enumeration, or paths.
