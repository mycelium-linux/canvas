;; Golden parity fixture: follow assets.latest.
;; Hand-written WAT so the test pins the ABI and the transform; it finds the
;; `latest` field in the host-provided context and copies it into a template.
;; Regenerate the .wasm with:
;;   cargo run -p canvas-model-host --example build-wasm-fixtures
(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const 8192) "{\"scene_id\":\"follow\",\"asset\":\"")
  (data (i32.const 9000) "\"latest\":\"")
  (data (i32.const 9100) "\",\"camera\":\"iso\",\"transition_ms\":0,\"fly_in\":false}")
  (data (i32.const 9200) "fallback.stl")
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
  (func $find
    (param $hay i32) (param $hay_len i32) (param $needle i32) (param $needle_len i32)
    (result i32)
    (local $i i32)
    (local $j i32)
    (loop $outer
      (if (i32.gt_s (i32.add (local.get $i) (local.get $needle_len)) (local.get $hay_len))
        (then (return (i32.const -1))))
      (local.set $j (i32.const 0))
      (block $mismatch
        (loop $inner
          (if (i32.ge_s (local.get $j) (local.get $needle_len))
            (then (return (local.get $i))))
          (br_if $mismatch
            (i32.ne
              (i32.load8_u (i32.add (local.get $hay) (i32.add (local.get $i) (local.get $j))))
              (i32.load8_u (i32.add (local.get $needle) (local.get $j)))))
          (local.set $j (i32.add (local.get $j) (i32.const 1)))
          (br $inner)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $outer))
    (i32.const -1))
  (func (export "scene") (param $ptr i32) (param $len i32) (result i32)
    (local $found i32)
    (local $at i32)
    (local $asset i32)
    (local $n i32)
    (local.set $found
      (call $find (local.get $ptr) (local.get $len) (i32.const 9000) (i32.const 10)))
    (call $copy (i32.const 4100) (i32.const 8192) (i32.const 30))
    (local.set $at (i32.add (i32.const 4100) (i32.const 30)))
    (if (i32.lt_s (local.get $found) (i32.const 0))
      (then
        (call $copy (local.get $at) (i32.const 9200) (i32.const 12))
        (local.set $at (i32.add (local.get $at) (i32.const 12))))
      (else
        (local.set $asset
          (i32.add (local.get $ptr) (i32.add (local.get $found) (i32.const 10))))
        (block $stop
          (loop $scan
            (br_if $stop (i32.ge_s (local.get $n) (i32.const 96)))
            (br_if $stop
              (i32.eq (i32.load8_u (i32.add (local.get $asset) (local.get $n))) (i32.const 34)))
            (local.set $n (i32.add (local.get $n) (i32.const 1)))
            (br $scan)))
        (call $copy (local.get $at) (local.get $asset) (local.get $n))
        (local.set $at (i32.add (local.get $at) (local.get $n)))))
    (call $copy (local.get $at) (i32.const 9100) (i32.const 50))
    (local.set $at (i32.add (local.get $at) (i32.const 50)))
    (i32.store (i32.const 4096) (i32.sub (local.get $at) (i32.const 4100)))
    (i32.const 4096)))
