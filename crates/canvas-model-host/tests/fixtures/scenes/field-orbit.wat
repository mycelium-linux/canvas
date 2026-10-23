;; Golden parity fixture: field + orbit.
;; Hand-written WAT so the test pins the ABI (memory/alloc/scene and the
;; assets JSON -> scene JSON transform), not one toolchain's output.
;; Regenerate the .wasm with:
;;   cargo run -p canvas-model-host --example build-wasm-fixtures
(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const 8192) "{\"scene_id\":\"bearing\",\"asset\":\"sha256-bearing.stl\",\"camera\":\"iso\",\"orbit\":{\"axis\":[0,1,0],\"degrees_per_second\":12},\"material\":{\"color\":[0,220,255],\"glow\":1.4,\"wireframe\":true,\"opacity\":0.85},\"field\":{\"name\":\"thermal\",\"unit\":\"C\",\"range\":[20,180],\"colormap\":\"inferno\",\"source\":{\"kind\":\"point_heat\",\"at\":[0,0,0],\"power\":160}},\"transition_ms\":700,\"fly_in\":true}")
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $len)))
    (local.get $ptr))
  (func $copy (param $dst i32) (param $src i32) (param $len i32)
    (local $i i32)
    (block $done
      (loop $each
        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
        (i32.store8
          (i32.add (local.get $dst) (local.get $i))
          (i32.load8_u (i32.add (local.get $src) (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $each))))
  (func (export "scene") (param $ptr i32) (param $len i32) (result i32)
    (i32.store (i32.const 4096) (i32.const 357))
    (call $copy (i32.const 4100) (i32.const 8192) (i32.const 357))
    (i32.const 4096)))
