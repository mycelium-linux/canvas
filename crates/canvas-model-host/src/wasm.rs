//! Bounded WASM scene construction.
//!
//! A WASM scene is the logic-only half of a Canvas app: the same
//! `assets JSON -> scene JSON` contract the Lua scene path implements, run
//! under wasmtime with **no host imports**. It cannot read the filesystem,
//! the clock, the network, or the GPU; the host enumerates assets, hands the
//! result in, and validates the scene that comes back with the same
//! [`SceneState`] type and `validate()` the Lua path uses.
//!
//! Everything here fails closed. A module that imports anything, is not a
//! core wasm module, runs out of fuel, traps, grows past its memory cap, emits
//! invalid JSON, or emits a scene that fails validation is an `Err` naming the
//! scene path. There is no fallback to Lua and no partial scene.
//!
//! The ABI is deliberately small and bump-allocated:
//!
//! - `memory`             exported linear memory
//! - `alloc(len: i32) -> i32`   reserve `len` input bytes, return their offset
//! - `scene(ptr: i32, len: i32) -> i32`  return a result block: `u32` LE length
//!   followed by that many bytes of UTF-8 scene JSON
//!
//! A small ABI is the point: it survives the scene's internals being rewritten
//! (Rust, Zig, or hand-written WAT all look the same from here) and it keeps
//! the host from exposing a richer surface than slice 1 needs.

use crate::{SceneContext, SceneState};
use anyhow::{Context, Result, anyhow, ensure};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use wasmtime::{Config, Engine, Memory, Module, ResourceLimiter, Store, StoreLimitsBuilder};

/// Fuel for one scene evaluation.
///
/// A scene is a pure JSON transform over a bounded asset list, so its real
/// cost is micro- to low-milliseconds. 50M fuel is roughly a second of
/// straight-line work on today's CPUs: generous enough that no honest scene
/// can hit it, and small enough that a runaway module cannot wedge the 30fps
/// host loop while it waits. Clock time is also measured and reported, but
/// fuel is the enforceable limit (`Config::consume_fuel` + a hard store
/// budget), while wall time is only observed.
pub const FUEL_BUDGET: u64 = 50_000_000;

/// Cap on linear memory for one scene instance. Scene input/output are JSON
/// documents measured in kilobytes; 8 MiB leaves room for a normal allocator
/// and stops a module from asking the host for hundreds of megabytes.
pub const MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;

/// A scene module is small: a scene is not an app. Rejecting larger files
/// before compilation keeps a hostile or accidental 100 MiB binary from
/// spending the host's compile time.
pub const MAX_MODULE_BYTES: usize = 2 * 1024 * 1024;

/// The input context is bounded too; it is derived from one directory listing.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// A scene JSON document larger than this is not a scene; it is an attempt to
/// make the host allocate. Keep the output window well inside the 8 MiB cap.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

const WASM_MAGIC: &[u8; 4] = b"\0asm";

/// What a caller can report about one scene evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Fuel charged by wasmtime for this evaluation.
    pub fuel_consumed: u64,
    /// Observed wall time (compile + instantiate + run), for logging only.
    pub elapsed_ms: f64,
}

impl Report {
    pub fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.elapsed_ms / 1_000.0)
    }
}

/// A validated scene plus the run's fuel/time accounting.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneRun {
    pub scene: SceneState,
    pub report: Report,
}

/// Evaluate a scene module from disk.
///
/// Every failure is annotated with the scene path, so a bad scene is
/// attributable without guesswork.
pub fn evaluate_file(path: &Path, context: &SceneContext) -> Result<SceneRun> {
    let source = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    evaluate_bytes(&source, context)
        .with_context(|| format!("evaluate wasm scene {}", path.display()))
}

/// Evaluate a scene module in memory.
pub fn evaluate_bytes(source: &[u8], context: &SceneContext) -> Result<SceneRun> {
    let started = Instant::now();
    ensure!(
        source.len() <= MAX_MODULE_BYTES,
        "wasm scene module exceeds {} MiB",
        MAX_MODULE_BYTES / (1024 * 1024)
    );
    ensure!(
        source.starts_with(WASM_MAGIC),
        "wasm scene is not a core wasm module (bad magic)"
    );

    let mut config = Config::new();
    config.consume_fuel(true);
    // Determinism: the same assets must produce the same scene JSON, so
    // canonicalize NaN payloads instead of letting the platform pick one.
    config.cranelift_nan_canonicalization(true);
    let engine = Engine::new(&config).context("configure wasm engine")?;
    let module = Module::new(&engine, source).context("parse wasm scene module")?;
    let imports = module.imports().count();
    ensure!(
        imports == 0,
        "wasm scene must import nothing, but it imports {imports} item(s)"
    );

    let limits = StoreLimitsBuilder::new()
        .memory_size(MAX_MEMORY_BYTES)
        .instances(1)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits as &mut dyn ResourceLimiter);
    store
        .set_fuel(FUEL_BUDGET)
        .context("set wasm fuel budget")?;

    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .context("instantiate wasm scene module")?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| anyhow!("wasm scene does not export `memory`"))?;

    let input = serde_json::to_vec(context).context("encode model scene context")?;
    ensure!(
        input.len() <= MAX_INPUT_BYTES,
        "model scene context exceeds {} KiB",
        MAX_INPUT_BYTES / 1024
    );

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .context("wasm scene does not export `alloc`")?;
    let ptr = alloc
        .call(&mut store, input.len() as i32)
        .context("wasm scene allocator trapped")?;
    let ptr = usize::try_from(ptr).map_err(|_| anyhow!("wasm scene allocator returned {ptr}"))?;
    ensure!(
        ptr.checked_add(input.len())
            .is_some_and(|end| end <= memory.data_size(&store)),
        "wasm scene allocator returned an out-of-bounds block"
    );
    memory
        .write(&mut store, ptr, &input)
        .context("write scene context into wasm memory")?;

    let scene_fn = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "scene")
        .context("wasm scene does not export `scene(ptr, len)`")?;
    let result_ptr = scene_fn
        .call(&mut store, (ptr as i32, input.len() as i32))
        .context("wasm scene trapped")?;
    let output = read_result_block(&mut store, &memory, result_ptr)?;
    let fuel_consumed =
        FUEL_BUDGET.saturating_sub(store.get_fuel().context("read remaining wasm fuel")?);

    let scene: SceneState =
        serde_json::from_slice(&output).context("decode wasm scene JSON result")?;
    scene.validate().context("validate wasm scene")?;
    Ok(SceneRun {
        scene,
        report: Report {
            fuel_consumed,
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        },
    })
}

/// Read the `u32 LE length + UTF-8 bytes` result block the `scene` export
/// returned, refusing to believe the length past the end of linear memory.
fn read_result_block(
    store: &mut Store<wasmtime::StoreLimits>,
    memory: &Memory,
    result_ptr: i32,
) -> Result<Vec<u8>> {
    let start =
        usize::try_from(result_ptr).map_err(|_| anyhow!("wasm scene returned {result_ptr}"))?;
    let memory_size = memory.data_size(&mut *store);
    ensure!(
        start.checked_add(4).is_some_and(|end| end <= memory_size),
        "wasm scene result block header lies outside linear memory"
    );
    let mut header = [0u8; 4];
    memory
        .read(&mut *store, start, &mut header)
        .context("read wasm scene result length")?;
    let length = u32::from_le_bytes(header) as usize;
    ensure!(
        length <= MAX_OUTPUT_BYTES,
        "wasm scene result exceeds {} KiB",
        MAX_OUTPUT_BYTES / 1024
    );
    ensure!(
        start
            .checked_add(4)
            .and_then(|offset| offset.checked_add(length))
            .is_some_and(|end| end <= memory_size),
        "wasm scene result block lies outside linear memory"
    );
    let mut output = vec![0u8; length];
    memory
        .read(&mut *store, start + 4, &mut output)
        .context("read wasm scene result")?;
    Ok(output)
}
