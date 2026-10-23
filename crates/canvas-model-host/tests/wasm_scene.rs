//! Acceptance tests for the logic-only WASM scene path.
//!
//! The fixtures are hand-written WAT rather than compiled Rust so the tests
//! pin the *contract* (memory/alloc/scene ABI and the `assets JSON -> scene
//! JSON` transform), not one toolchain's code generation. They are tiny and
//! readable on purpose: a scene that follows a build only has to find the
//! `latest` field in the host-provided context and copy it into a template.

use canvas_model_host::{SceneContext, lua, wasm};
use std::path::{Path, PathBuf};

/// A scene that ignores its input and returns a fixed JSON document.
fn constant_scene(json: &str) -> String {
    let length = json.len();
    let json = escape_wat(json);
    format!(
        r#"
(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const 8192) "{json}")
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
    (i32.store (i32.const 4096) (i32.const {length}))
    (call $copy (i32.const 4100) (i32.const 8192) (i32.const {length}))
    (i32.const 4096)))
"#
    )
}

/// A scene that copies `assets.latest` into its asset field, or falls back to
/// a fixed name when the build has produced nothing yet.
fn follows_latest_scene() -> String {
    let prefix = r#"{"scene_id":"follow","asset":""#;
    let needle = r#""latest":""#;
    let suffix = r#"","camera":"iso","transition_ms":0,"fly_in":false}"#;
    let fallback = "fallback.stl";
    let (prefix, needle, suffix, fallback) = (
        escape_wat(prefix),
        escape_wat(needle),
        escape_wat(suffix),
        escape_wat(fallback),
    );
    format!(
        r#"
(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const 8192) "{prefix}")
  (data (i32.const 9000) "{needle}")
  (data (i32.const 9100) "{suffix}")
  (data (i32.const 9200) "{fallback}")
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
      (call $find (local.get $ptr) (local.get $len) (i32.const 9000) (i32.const {needle_len})))
    (call $copy (i32.const 4100) (i32.const 8192) (i32.const {prefix_len}))
    (local.set $at (i32.add (i32.const 4100) (i32.const {prefix_len})))
    (if (i32.lt_s (local.get $found) (i32.const 0))
      (then
        (call $copy (local.get $at) (i32.const 9200) (i32.const {fallback_len}))
        (local.set $at (i32.add (local.get $at) (i32.const {fallback_len}))))
      (else
        (local.set $asset
          (i32.add (local.get $ptr) (i32.add (local.get $found) (i32.const {needle_len}))))
        (block $stop
          (loop $scan
            (br_if $stop (i32.ge_s (local.get $n) (i32.const 96)))
            (br_if $stop
              (i32.eq (i32.load8_u (i32.add (local.get $asset) (local.get $n))) (i32.const 34)))
            (local.set $n (i32.add (local.get $n) (i32.const 1)))
            (br $scan)))
        (call $copy (local.get $at) (local.get $asset) (local.get $n))
        (local.set $at (i32.add (local.get $at) (local.get $n)))))
    (call $copy (local.get $at) (i32.const 9100) (i32.const {suffix_len}))
    (local.set $at (i32.add (local.get $at) (i32.const {suffix_len})))
    (i32.store (i32.const 4096) (i32.sub (local.get $at) (i32.const 4100)))
    (i32.const 4096)))
"#,
        prefix = prefix,
        needle = needle,
        suffix = suffix,
        fallback = fallback,
        prefix_len = prefix.replace("\\\"", "\"").len(),
        needle_len = needle.replace("\\\"", "\"").len(),
        suffix_len = suffix.replace("\\\"", "\"").len(),
        fallback_len = fallback.len(),
    )
}

fn escape_wat(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_module(dir: &Path, name: &str, wat_source: &str) -> PathBuf {
    let module = wat::parse_str(wat_source).expect("fixture WAT parses");
    let path = dir.join(name);
    std::fs::write(&path, module).expect("write fixture wasm");
    path
}

fn three_steps() -> SceneContext {
    SceneContext {
        steps: vec![
            "step-0.stl".into(),
            "step-1.stl".into(),
            "step-2.stl".into(),
        ],
        latest: Some("step-2.stl".into()),
    }
}

/// The parity fixture: orbit, material and a scalar field declared once as
/// JSON and once as Lua. Both go through the same serde types and
/// `validate()`; the scene values must match exactly.
#[test]
fn wasm_and_lua_scenes_agree_on_field_and_orbit() {
    let json = r#"{"scene_id":"bearing","asset":"sha256-bearing.stl","camera":"iso","orbit":{"axis":[0,1,0],"degrees_per_second":12},"material":{"color":[0,220,255],"glow":1.4,"wireframe":true,"opacity":0.85},"field":{"name":"thermal","unit":"C","range":[20,180],"colormap":"inferno","source":{"kind":"point_heat","at":[0,0,0],"power":160}},"transition_ms":700,"fly_in":true}"#;
    let lua_source = r#"
        return {
          scene_id = "bearing",
          asset = "sha256-bearing.stl",
          camera = "iso",
          orbit = { axis = {0, 1, 0}, degrees_per_second = 12 },
          material = { color = {0, 220, 255}, glow = 1.4, wireframe = true, opacity = 0.85 },
          field = {
            name = "thermal", unit = "C", range = {20, 180}, colormap = "inferno",
            source = { kind = "point_heat", at = {0, 0, 0}, power = 160 },
          },
          transition_ms = 700,
          fly_in = true,
        }
    "#;
    let context = SceneContext::scan(&std::env::temp_dir());
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "field-orbit.wasm", &constant_scene(json));

    let wasm_scene = wasm::evaluate_file(&path, &context).expect("wasm scene evaluates");
    let lua_scene = lua::evaluate_with(lua_source, &context).expect("lua scene evaluates");

    assert_eq!(wasm_scene.scene, lua_scene);
    assert!(wasm_scene.scene.validate().is_ok());
    assert_eq!(
        wasm_scene.scene.validate().is_ok(),
        lua_scene.validate().is_ok()
    );
    assert_eq!(
        wasm_scene.scene.field.expect("field").source,
        lua_scene.field.expect("field").source
    );
    assert!(wasm_scene.report.fuel_consumed > 0, "fuel is metered");
    assert!(wasm_scene.report.elapsed_ms >= 0.0);
}

/// The whole point of the slice: a WASM scene follows a build the same way
/// the Lua scene does, including the empty-build fallback.
#[test]
fn wasm_and_lua_scenes_agree_when_following_assets_latest() {
    let lua_source = r#"
        return {
          scene_id = "follow",
          asset = assets.latest or "fallback.stl",
          camera = "iso",
          transition_ms = 0,
          fly_in = false,
        }
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "follow.wasm", &follows_latest_scene());

    for context in [
        three_steps(),
        SceneContext {
            steps: vec!["step-0.stl".into(), "step-1.stl".into()],
            latest: Some("step-1.stl".into()),
        },
        SceneContext::default(),
    ] {
        let wasm_scene = wasm::evaluate_file(&path, &context).expect("wasm scene evaluates");
        let lua_scene = lua::evaluate_with(lua_source, &context).expect("lua scene evaluates");
        assert_eq!(wasm_scene.scene, lua_scene, "context: {context:?}");
        assert!(wasm_scene.scene.validate().is_ok());
    }
}

/// A scene that imports *anything* is rejected before it runs: with no host
/// imports there is no filesystem, clock, network or GPU surface to reach.
#[test]
fn a_module_with_host_imports_is_rejected() {
    let wat_source = r#"
    (module
      (import "env" "now" (func (result i64)))
      (memory (export "memory") 1)
      (func (export "alloc") (param $len i32) (result i32) (i32.const 0))
      (func (export "scene") (param $ptr i32) (param $len i32) (result i32) (i32.const 0)))
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "imports.wasm", wat_source);
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must reject");
    let message = format!("{error:#}");
    assert!(
        message.contains("import"),
        "error should name the imports: {message}"
    );
    assert!(
        message.contains(&path.display().to_string()),
        "error should name the scene path: {message}"
    );
}

/// Fuel is the enforceable limit; an endless loop must fail fast instead of
/// hanging the host's frame loop. The clock is not a scheduler.
#[test]
fn an_infinite_loop_runs_out_of_fuel() {
    let wat_source = r#"
    (module
      (memory (export "memory") 1)
      (func (export "alloc") (param $len i32) (result i32) (i32.const 0))
      (func (export "scene") (param $ptr i32) (param $len i32) (result i32)
        (loop $forever (br $forever))
        (i32.const 0)))
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "spin.wasm", wat_source);
    let started = std::time::Instant::now();
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must stop");
    let message = format!("{error:#}");
    assert!(
        message.to_lowercase().contains("fuel"),
        "error should name fuel: {message}"
    );
    assert!(
        message.contains(&path.display().to_string()),
        "error should name the scene path: {message}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "a runaway scene must not hold the host"
    );
}

/// The linear-memory cap is real: a scene that tries to grow past 8 MiB traps
/// at the limiter rather than asking the host for the memory.
#[test]
fn a_memory_hungry_scene_fails_closed() {
    let wat_source = r#"
    (module
      (memory (export "memory") 1)
      (func (export "alloc") (param $len i32) (result i32) (i32.const 0))
      (func (export "scene") (param $ptr i32) (param $len i32) (result i32)
        (local $page i32)
        (block $all
          (loop $grow
            (br_if $all (i32.ge_s (local.get $page) (i32.const 256)))
            (if (i32.eq (memory.grow (i32.const 1)) (i32.const -1))
              (then (unreachable)))
            (local.set $page (i32.add (local.get $page) (i32.const 1)))
            (br $grow)))
        (i32.const 0)))
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "greedy.wasm", wat_source);
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must stop");
    let message = format!("{error:#}");
    assert!(
        message.contains(&path.display().to_string()),
        "error should name the scene path: {message}"
    );
    assert!(
        message.to_lowercase().contains("trap") || message.to_lowercase().contains("unreachable"),
        "error should describe the trap: {message}"
    );
}

/// A trap is an error, never a partial or stale scene.
#[test]
fn a_trapped_scene_is_an_error() {
    let wat_source = r#"
    (module
      (memory (export "memory") 1)
      (func (export "alloc") (param $len i32) (result i32) (i32.const 0))
      (func (export "scene") (param $ptr i32) (param $len i32) (result i32)
        (unreachable)))
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "trap.wasm", wat_source);
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must reject");
    let message = format!("{error:#}");
    assert!(
        message.contains(&path.display().to_string()),
        "error should name the scene path: {message}"
    );
    assert!(
        message.to_lowercase().contains("trap"),
        "error should describe the trap: {message}"
    );
}

/// Arbitrary bytes are rejected with a clear reason, not fed to the parser.
#[test]
fn a_non_wasm_file_is_rejected_with_a_magic_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("garbage.wasm");
    std::fs::write(&path, b"this is not wasm").expect("write garbage");
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must reject");
    let message = format!("{error:#}");
    assert!(
        message.contains(&path.display().to_string()),
        "error should name the scene path: {message}"
    );
    assert!(
        message.to_lowercase().contains("magic"),
        "error should name the bad magic: {message}"
    );
}

/// Valid wasm that emits something other than a scene JSON document fails
/// closed instead of handing the renderer half a scene.
#[test]
fn invalid_json_output_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_module(dir.path(), "not-json.wasm", &constant_scene("not json"));
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must reject");
    let message = format!("{error:#}");
    assert!(message.contains("decode"), "{message}");
    assert!(message.contains(&path.display().to_string()), "{message}");

    let path = write_module(
        dir.path(),
        "bad-scene.wasm",
        &constant_scene(r#"{"scene_id":"bad id","asset":"../escape.stl"}"#),
    );
    let error = wasm::evaluate_file(&path, &SceneContext::default()).expect_err("must reject");
    let message = format!("{error:#}");
    assert!(
        message.contains("validate wasm scene"),
        "validation must be the same as Lua: {message}"
    );
    assert!(message.contains(&path.display().to_string()), "{message}");
}
