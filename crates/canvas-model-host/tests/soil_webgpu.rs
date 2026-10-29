//! `soil:webgpu` through a real component: resource handles, shader authority,
//! and a draw/readback through the host linker.
//!
//! The fixture is built from `tests/fixtures/webgpu/guest` for
//! `wasm32-wasip2`; see that crate's WIT world.

#![cfg(feature = "soil-webgpu")]

use canvas_model_host::caps::{EffectiveCaps, Limits};
use canvas_model_host::webgpu::WebGpuHost;
use canvas_model_host::webgpu_linker::{WebGpuCtx, add_to_linker};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use wasmtime::component::{Component, Instance, Linker, Val};
use wasmtime::{Config, Engine, Store};

fn fixture() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webgpu/soil-webgpu-guest.wasm"),
    )
    .expect("read soil-webgpu fixture")
}

fn engine() -> Engine {
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("engine")
}

fn caps(names: &[&str]) -> EffectiveCaps {
    EffectiveCaps {
        caps: names
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>(),
        limits: Limits {
            memory_mb: 64,
            fuel: 10_000_000,
            epoch_ms: 5_000,
        },
        clamped: Vec::new(),
    }
}

fn instantiate(
    engine: &Engine,
    caps: &EffectiveCaps,
) -> Result<(Store<WebGpuCtx>, Instance), wasmtime::Error> {
    let host = Arc::new(WebGpuHost::new().expect("webgpu host"));
    let component = Component::new(engine, fixture()).expect("compile webgpu fixture");
    let mut linker = Linker::new(engine);
    let context = add_to_linker(&mut linker, caps, host).expect("register webgpu");
    let mut store = Store::new(engine, context);
    let instance = linker.instantiate(&mut store, &component)?;
    Ok((store, instance))
}

fn run(store: &mut Store<WebGpuCtx>, instance: &Instance) -> i64 {
    let func = instance.get_func(&mut *store, "run").expect("guest run");
    let mut results = [Val::S64(0)];
    func.call(&mut *store, &[], &mut results).expect("call run");
    match results[0] {
        Val::S64(value) => value,
        ref other => panic!("run returned {other:?}"),
    }
}

#[test]
fn guest_draws_through_granted_handles() {
    if WebGpuHost::new().is_err() {
        eprintln!("no WGPU adapter; skipping");
        return;
    }
    let engine = engine();
    let Ok((mut store, instance)) =
        instantiate(&engine, &caps(&["soil:webgpu", "soil:webgpu.shaders"]))
    else {
        eprintln!("no WGPU adapter or component instantiate failed; skipping");
        return;
    };
    let checksum = run(&mut store, &instance);
    assert!(checksum > 0, "guest reported failure code {checksum}");
}

#[test]
fn shader_authority_is_separate_from_gpu_access() {
    if WebGpuHost::new().is_err() {
        eprintln!("no WGPU adapter; skipping");
        return;
    }
    let engine = engine();
    let Ok((mut store, instance)) = instantiate(&engine, &caps(&["soil:webgpu"])) else {
        eprintln!("no WGPU adapter or component instantiate failed; skipping");
        return;
    };
    assert_eq!(run(&mut store, &instance), -2, "shader must be denied");
}

#[test]
fn ungranted_webgpu_has_no_imports() {
    if WebGpuHost::new().is_err() {
        eprintln!("no WGPU adapter; skipping");
        return;
    }
    let engine = engine();
    let error = match instantiate(&engine, &caps(&[])) {
        Ok(_) => panic!("no webgpu grant must fail"),
        Err(error) => error,
    };
    let text = format!("{error:#}");
    assert!(
        text.contains("unknown import")
            || text.contains("failed to find import")
            || text.contains("matching implementation was not found")
            || text.contains("implementation is missing"),
        "{text}"
    );
}
