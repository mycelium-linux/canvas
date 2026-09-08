# Lua and LÖVR adapter boundary

Canvas can use Lua as a fast authoring language without making scripts an
authority boundary. The `lua-scenes` feature embeds Luau through `mlua` in a
fresh VM for each scene generation. A script receives one `canvas` table and
can append groups, text, images, and meshes. Images and meshes refer only to
immutable assets explicitly granted by a trusted caller. The result is a typed
`Scene`; it passes the same Canvas validation as data received from any other
producer.

The VM loads only table, string, math, and UTF-8 libraries. Luau sandboxing
makes builtin globals read-only and removes filesystem, process, package, and
native-module entry points. Canvas also applies source-size, memory, VM-interrupt,
and node-count limits. Scripts receive no network, clock, random, native window,
Canvas control, durable storage, or workspace-patch callback. A generated scene
therefore remains an untrusted proposal; Canvas retains authorization, revision,
placement, persistence, and human-takeover policy.

LÖVR should begin as a separate renderer adapter or process. It consumes a
validated Canvas scene snapshot plus immutable asset bytes and returns input or
pose events through a versioned adapter contract. It does not host the
authoritative workspace document and Canvas does not link to LÖVR internals.
This preserves the option to use iced/wgpu on desktop and LÖVR for XR while
keeping one scene model. Snapshot compilation and validation happen off the
frame path; the renderer applies compact prepared changes at a frame boundary.

Primary-source findings:

- [LÖVR's official repository](https://github.com/bjornbytes/lovr) describes a
  C11 runtime scripted with LuaJIT, single-pass stereo rendering, desktop and
  Android support, common model formats, spatial audio, physics, and compute
  shaders. Its [license](https://github.com/bjornbytes/lovr/blob/master/LICENSE)
  is MIT.
- [`mlua`'s official repository](https://github.com/mlua-rs/mlua) supports
  embedded Luau, Lua 5.x, LuaJIT, and WASM through Emscripten; the selected Luau
  feature is built from source and does not enable module mode. `mlua` is MIT.
- [Luau's official sandbox guide](https://luau.org/sandbox/) states that `io`
  and `package` are removed, dangerous `os` operations and file loaders are
  absent, globals can be made read-only, and hosts should impose memory and CPU
  limits. It also recommends separate VMs for guaranteed isolation, which is
  why Canvas does not reuse a VM between scene producers.

LÖVR and embedded Luau remain optional dependencies. The ordinary Canvas data
model and renderer do not require either runtime.
