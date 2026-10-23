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
//! core wasm module, runs out of fuel, exceeds its epoch deadline, traps,
//! grows past its memory cap, emits invalid JSON, or emits a scene that fails
//! validation is an [`SceneError`] naming the failure kind. There is no
//! fallback to Lua and no partial scene.
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
use std::{
    error::Error,
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use wasmtime::{
    Config, Engine, Memory, Module, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder, Trap,
};

/// Fuel for one scene evaluation when the caller does not supply a budget.
///
/// A scene is a pure JSON transform over a bounded asset list, so its real
/// cost is micro- to low-milliseconds. 50M fuel is roughly a second of
/// straight-line work on today's CPUs: generous enough that no honest scene
/// can hit it, and small enough that a runaway module cannot wedge the 30fps
/// host loop while it waits.
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

/// How often the epoch ticker advances the engine clock.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

/// How many ticker ticks a scene gets before wasmtime interrupts it.
///
/// 100 ticks at 10ms is a one-second wall-clock ceiling. Fuel is the primary
/// deterministic budget; the epoch deadline is the backstop for a module that
/// somehow spends wall time without spending much fuel (and for future
/// hosts that run scenes with fuel accounting disabled).
pub const EPOCH_DEADLINE_TICKS: u64 = 100;

/// One-second scene ceiling, derived rather than written down twice.
pub const EPOCH_BUDGET_MS: u64 =
    (EPOCH_TICK.as_millis() as u64).saturating_mul(EPOCH_DEADLINE_TICKS);

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

/// Every way a wasm scene can fail.
///
/// Variants are distinct so a caller (or a test) can tell a module that lied
/// about its ABI from one that spun too long from one that simply emitted bad
/// JSON. The CLI maps all of them to exit code 1; the host names the exact
/// failure instead of collapsing them into "scene failed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneError {
    /// The file is larger than [`MAX_MODULE_BYTES`].
    ModuleTooLarge { size: usize, limit: usize },
    /// The file does not start with the core wasm magic bytes.
    BadMagic,
    /// The module declares imports, so it asks for a host surface we do not
    /// provide.
    Imports { count: usize },
    /// The module burned its whole fuel budget.
    FuelExhausted { fuel: u64 },
    /// The module tried to grow past [`MAX_MEMORY_BYTES`].
    MemoryLimit { limit: usize },
    /// The module ran past its epoch deadline.
    EpochTimeout { ticks: u64, budget_ms: u64 },
    /// A required export is missing or has the wrong signature.
    WrongAbi { missing: &'static str },
    /// The module trapped for a reason that is not one of the limits above.
    Trap { message: String },
    /// The bytes are not a module this engine can compile.
    Compile { message: String },
    /// Compilation succeeded but instantiation failed for a non-limit reason.
    Instantiate { message: String },
    /// The host-provided asset context is too large.
    ContextTooLarge { size: usize, limit: usize },
    /// The scene returned a result block larger than [`MAX_OUTPUT_BYTES`].
    OutputTooLarge { size: usize, limit: usize },
    /// The scene's result bytes are not a valid scene JSON document.
    InvalidJson { message: String },
    /// The decoded scene failed the shared `SceneState::validate` rules.
    Validation { message: String },
    /// Reading the scene file failed.
    Io { message: String },
    /// The host could not encode its own asset context.
    ContextEncode { message: String },
    /// The returned block lies outside linear memory or is otherwise impossible.
    MalformedResult { message: String },
}

impl SceneError {
    /// Stable, machine-readable code used by tests and by the documentation's
    /// fail-closed table.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ModuleTooLarge { .. } => "module_too_large",
            Self::BadMagic => "bad_magic",
            Self::Imports { .. } => "host_imports",
            Self::FuelExhausted { .. } => "fuel_exhausted",
            Self::MemoryLimit { .. } => "memory_limit",
            Self::EpochTimeout { .. } => "epoch_timeout",
            Self::WrongAbi { .. } => "wrong_abi",
            Self::Trap { .. } => "trap",
            Self::Compile { .. } => "compile",
            Self::Instantiate { .. } => "instantiate",
            Self::ContextTooLarge { .. } => "context_too_large",
            Self::OutputTooLarge { .. } => "output_too_large",
            Self::InvalidJson { .. } => "invalid_json",
            Self::Validation { .. } => "validation",
            Self::Io { .. } => "io",
            Self::ContextEncode { .. } => "context_encode",
            Self::MalformedResult { .. } => "malformed_result",
        }
    }
}

impl fmt::Display for SceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModuleTooLarge { size, limit } => write!(
                formatter,
                "wasm scene module is too large: {size} bytes exceeds {limit} bytes"
            ),
            Self::BadMagic => write!(
                formatter,
                "wasm scene has bad magic: not a core wasm module"
            ),
            Self::Imports { count } => write!(
                formatter,
                "wasm scene must import nothing, but it imports {count} item(s)"
            ),
            Self::FuelExhausted { fuel } => {
                write!(formatter, "wasm scene exhausted its {fuel} fuel budget")
            }
            Self::MemoryLimit { limit } => write!(
                formatter,
                "wasm scene exceeded its {limit}-byte memory limit"
            ),
            Self::EpochTimeout { ticks, budget_ms } => write!(
                formatter,
                "wasm scene timed out (epoch deadline: {ticks} ticks, ~{budget_ms} ms)"
            ),
            Self::WrongAbi { missing } => {
                write!(formatter, "wasm scene ABI is incomplete: {missing}")
            }
            Self::Trap { message } => write!(formatter, "wasm scene trapped: {message}"),
            Self::Compile { message } => {
                write!(formatter, "parse wasm scene module: {message}")
            }
            Self::Instantiate { message } => {
                write!(formatter, "instantiate wasm scene module: {message}")
            }
            Self::ContextTooLarge { size, limit } => write!(
                formatter,
                "model scene context is too large: {size} bytes exceeds {limit} bytes"
            ),
            Self::OutputTooLarge { size, limit } => write!(
                formatter,
                "wasm scene result is too large: {size} bytes exceeds {limit} bytes"
            ),
            Self::InvalidJson { message } => {
                write!(formatter, "decode wasm scene JSON result: {message}")
            }
            Self::Validation { message } => write!(formatter, "validate wasm scene: {message}"),
            Self::Io { message } => write!(formatter, "read wasm scene: {message}"),
            Self::ContextEncode { message } => {
                write!(formatter, "encode model scene context: {message}")
            }
            Self::MalformedResult { message } => {
                write!(
                    formatter,
                    "wasm scene returned a malformed result: {message}"
                )
            }
        }
    }
}

impl Error for SceneError {}

/// Evaluate a scene module from disk with the default fuel budget.
pub fn evaluate_file(path: &Path, context: &SceneContext) -> anyhow::Result<SceneRun> {
    evaluate_file_with_fuel(path, context, FUEL_BUDGET)
}

/// Evaluate a scene module from disk with an explicit fuel budget.
///
/// Every failure keeps the scene path in the outer context while preserving the
/// typed [`SceneError`] so callers can tell limit failures apart.
pub fn evaluate_file_with_fuel(
    path: &Path,
    context: &SceneContext,
    fuel: u64,
) -> anyhow::Result<SceneRun> {
    let source = std::fs::read(path).map_err(|error| SceneError::Io {
        message: format!("{}: {error}", path.display()),
    })?;
    evaluate_bytes_with_fuel(&source, context, fuel)
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("evaluate wasm scene {}", path.display())))
}

/// Evaluate a scene module in memory with the default fuel budget.
pub fn evaluate_bytes(source: &[u8], context: &SceneContext) -> anyhow::Result<SceneRun> {
    Ok(evaluate_bytes_with_fuel(source, context, FUEL_BUDGET)?)
}

/// Evaluate a scene module in memory with an explicit fuel budget.
pub fn evaluate_bytes_with_fuel(
    source: &[u8],
    context: &SceneContext,
    fuel: u64,
) -> Result<SceneRun, SceneError> {
    let started = Instant::now();
    if source.len() > MAX_MODULE_BYTES {
        return Err(SceneError::ModuleTooLarge {
            size: source.len(),
            limit: MAX_MODULE_BYTES,
        });
    }
    if !source.starts_with(WASM_MAGIC) {
        return Err(SceneError::BadMagic);
    }

    let mut config = Config::new();
    config.consume_fuel(true);
    // Wall-clock backstop. Fuel alone cannot stop a module that spends time in
    // a long-running loop with unusually cheap instructions, and a future
    // caller may not meter fuel at all.
    config.epoch_interruption(true);
    // Determinism: the same assets must produce the same scene JSON, so
    // canonicalize NaN payloads instead of letting the platform pick one.
    config.cranelift_nan_canonicalization(true);
    let engine = Engine::new(&config).map_err(|error| SceneError::Compile {
        message: error.to_string(),
    })?;
    let module = Module::new(&engine, source).map_err(|error| SceneError::Compile {
        message: error.to_string(),
    })?;

    let imports = module.imports().count();
    if imports != 0 {
        return Err(SceneError::Imports { count: imports });
    }
    require_export(&module, "memory")?;
    require_export(&module, "alloc")?;
    require_export(&module, "scene")?;

    let limits = StoreLimitsBuilder::new()
        .memory_size(MAX_MEMORY_BYTES)
        .instances(1)
        .build();
    let mut store = Store::new(&engine, SceneLimits::new(limits));
    store.limiter(|limits| limits as &mut dyn ResourceLimiter);
    store
        .set_fuel(fuel)
        .map_err(|error| SceneError::Instantiate {
            message: error.to_string(),
        })?;
    store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
    let _ticker = EpochTicker::start(engine.clone());

    let instance = wasmtime::Instance::new(&mut store, &module, &[]).map_err(|error| {
        classify_runtime_error(&store, &error, fuel, SceneFailureSite::Instantiate)
    })?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or(SceneError::WrongAbi {
            missing: "`memory` export",
        })?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .map_err(|_| SceneError::WrongAbi {
            missing: "`alloc(i32) -> i32` export",
        })?;

    let input = serde_json::to_vec(context).map_err(|error| SceneError::ContextEncode {
        message: error.to_string(),
    })?;
    if input.len() > MAX_INPUT_BYTES {
        return Err(SceneError::ContextTooLarge {
            size: input.len(),
            limit: MAX_INPUT_BYTES,
        });
    }

    let ptr = alloc
        .call(&mut store, input.len() as i32)
        .map_err(|error| classify_runtime_error(&store, &error, fuel, SceneFailureSite::Call))?;
    let ptr = usize::try_from(ptr).map_err(|_| SceneError::MalformedResult {
        message: format!("allocator returned negative offset {ptr}"),
    })?;
    if ptr
        .checked_add(input.len())
        .is_none_or(|end| end > memory.data_size(&store))
    {
        return Err(SceneError::MalformedResult {
            message: "allocator returned an out-of-bounds block".into(),
        });
    }
    memory
        .write(&mut store, ptr, &input)
        .map_err(|error| SceneError::MalformedResult {
            message: format!("write scene context into wasm memory: {error}"),
        })?;

    let scene_fn = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "scene")
        .map_err(|_| SceneError::WrongAbi {
            missing: "`scene(i32, i32) -> i32` export",
        })?;
    let result_ptr = scene_fn
        .call(&mut store, (ptr as i32, input.len() as i32))
        .map_err(|error| classify_runtime_error(&store, &error, fuel, SceneFailureSite::Call))?;
    let output = read_result_block(&mut store, &memory, result_ptr)?;
    let fuel_consumed =
        fuel.saturating_sub(store.get_fuel().map_err(|error| SceneError::Instantiate {
            message: error.to_string(),
        })?);

    let scene: SceneState =
        serde_json::from_slice(&output).map_err(|error| SceneError::InvalidJson {
            message: error.to_string(),
        })?;
    scene.validate().map_err(|error| SceneError::Validation {
        message: format!("{error:#}"),
    })?;
    Ok(SceneRun {
        scene,
        report: Report {
            fuel_consumed,
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        },
    })
}

#[derive(Clone, Copy)]
enum SceneFailureSite {
    Instantiate,
    Call,
}

/// Turn a wasmtime error into the most specific [`SceneError`] the host knows.
fn classify_runtime_error(
    store: &Store<SceneLimits>,
    error: &wasmtime::Error,
    fuel: u64,
    site: SceneFailureSite,
) -> SceneError {
    // The limiter flag is the only reliable way to tell a denied `memory.grow`
    // from any other trap: wasmtime returns `-1` to the guest by default, so
    // the guest's own follow-on trap is not the memory failure.
    if store.data().memory_denied {
        return SceneError::MemoryLimit {
            limit: MAX_MEMORY_BYTES,
        };
    }
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => SceneError::FuelExhausted { fuel },
            Trap::Interrupt => SceneError::EpochTimeout {
                ticks: EPOCH_DEADLINE_TICKS,
                budget_ms: EPOCH_BUDGET_MS,
            },
            _ => SceneError::Trap {
                message: error.to_string(),
            },
        };
    }
    match site {
        SceneFailureSite::Instantiate => SceneError::Instantiate {
            message: format!("{error:#}"),
        },
        SceneFailureSite::Call => SceneError::Trap {
            message: format!("{error:#}"),
        },
    }
}

/// Refuse a module before instantiation if it does not even name the exports
/// the ABI requires. Signature mismatches are caught by `get_typed_func`.
fn require_export(module: &Module, name: &'static str) -> Result<(), SceneError> {
    if module.exports().any(|export| export.name() == name) {
        Ok(())
    } else {
        Err(SceneError::WrongAbi {
            missing: match name {
                "memory" => "`memory` export",
                "alloc" => "`alloc(ptr) -> i32` export",
                "scene" => "`scene(ptr, len) -> i32` export",
                _ => "required export",
            },
        })
    }
}

/// Read the `u32 LE length + UTF-8 bytes` result block the `scene` export
/// returned, refusing to believe the length past the end of linear memory.
fn read_result_block(
    store: &mut Store<SceneLimits>,
    memory: &Memory,
    result_ptr: i32,
) -> Result<Vec<u8>, SceneError> {
    let start = usize::try_from(result_ptr).map_err(|_| SceneError::MalformedResult {
        message: format!("scene returned negative offset {result_ptr}"),
    })?;
    let memory_size = memory.data_size(&mut *store);
    if start.checked_add(4).is_none_or(|end| end > memory_size) {
        return Err(SceneError::MalformedResult {
            message: "result block header lies outside linear memory".into(),
        });
    }
    let mut header = [0u8; 4];
    memory
        .read(&mut *store, start, &mut header)
        .map_err(|error| SceneError::MalformedResult {
            message: format!("read result length: {error}"),
        })?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_OUTPUT_BYTES {
        return Err(SceneError::OutputTooLarge {
            size: length,
            limit: MAX_OUTPUT_BYTES,
        });
    }
    if start
        .checked_add(4)
        .and_then(|offset| offset.checked_add(length))
        .is_none_or(|end| end > memory_size)
    {
        return Err(SceneError::MalformedResult {
            message: "result block lies outside linear memory".into(),
        });
    }
    let mut output = vec![0u8; length];
    memory
        .read(&mut *store, start + 4, &mut output)
        .map_err(|error| SceneError::MalformedResult {
            message: format!("read result: {error}"),
        })?;
    Ok(output)
}

/// Store data that remembers whether a memory growth was refused.
///
/// `StoreLimits` alone returns `false`/`-1` to the guest; keeping the fact on
/// the store lets the host report [`SceneError::MemoryLimit`] even when the
/// guest reacts by trapping for some unrelated reason.
struct SceneLimits {
    inner: StoreLimits,
    memory_denied: bool,
}

impl SceneLimits {
    fn new(inner: StoreLimits) -> Self {
        Self {
            inner,
            memory_denied: false,
        }
    }
}

impl ResourceLimiter for SceneLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        let allowed = self.inner.memory_growing(current, desired, maximum)?;
        if !allowed {
            self.memory_denied = true;
        }
        Ok(allowed)
    }

    fn memory_grow_failed(&mut self, error: anyhow::Error) -> anyhow::Result<()> {
        self.inner.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: u32,
        desired: u32,
        maximum: Option<u32>,
    ) -> anyhow::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }

    fn table_grow_failed(&mut self, error: anyhow::Error) -> anyhow::Result<()> {
        self.inner.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// Advance the engine's epoch clock until dropped.
///
/// A separate thread rather than a per-call timer is what wasmtime's epoch
/// mechanism expects: the guest checks the deadline at loop backedges and
/// calls, and the ticker makes the deadline actually pass.
struct EpochTicker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("canvas-wasm-epoch".into())
            .spawn(move || {
                while !stop_for_thread.load(Ordering::Relaxed) {
                    thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })
            .expect("spawn wasm epoch ticker");
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
