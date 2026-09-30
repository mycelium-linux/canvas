//! Bounded owner-local stream ingress and sandboxed event-to-widget adapters.

use crate::{Error, Result};
use canvas_widgets::{BriefingBeat, WidgetMutation, WidgetSpec};
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, VmState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt, PermissionsExt},
    net::UnixDatagram,
};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_STREAM_MESSAGE: usize = 64 * 1024;
const MAX_PENDING_EVENTS: usize = 512;
pub const MAX_ADAPTER_BYTES: usize = 32 * 1024;

/// An artifact manifest packages a query, a binder, and the widget kind it
/// produces into one addressable unit. Per docs/widget-quality.md, these
/// three must agree — and today they live in three unrelated files with no
/// resolution order or manifest. The resolution order is:
///
/// 1. Project-local: `runtime_config::root()/artifacts/<id>.json`
/// 2. Global: `~/.local/state/dock/canvas/artifacts/<id>.json`
/// 3. Built-in: `config/artifacts/<id>.json` (compiled in via include_str!)
///
/// A project can override a shared artifact without forking the binder itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    /// The stream source this artifact consumes (e.g. "prometheus:bifrost/health").
    pub source: String,
    /// The binder Lua file name (without path) that transforms events into
    /// widget snapshots. Loaded via the same runtime_config or built-in path
    /// as `apply_named_adapter`.
    pub binder: String,
    /// The widget kind this artifact produces — enforced at the boundary so
    /// a mismatched spec fails before it reaches a surface.
    pub widget_kind: String,
    /// Whether this artifact is marked as demo/sample (non-live data).
    #[serde(default)]
    pub provenance: String,
    /// Minimum refresh interval in milliseconds. Widgets fed by this artifact
    /// are not polled more frequently than this.
    #[serde(default = "default_refresh_ms")]
    pub refresh_ms: u64,
}

fn default_refresh_ms() -> u64 {
    5_000
}

impl ArtifactManifest {
    /// The manifest schema version. Bumped when fields change in a
    /// non-backward-compatible way.
    pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

    /// Load an artifact manifest by ID, searching in resolution order:
    /// project local → global → built-in.
    pub fn load(id: &str) -> Result<Self> {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error(format!("invalid artifact id: {id}")));
        }
        // 1. Project-local: runtime config directory
        let relative = PathBuf::from("artifacts").join(format!("{id}.json"));
        if let Some(bytes) = crate::runtime_config::read(&relative, 64 * 1024)? {
            return Self::parse(&bytes, id);
        }
        // 2. Global user state directory
        if let Some(global_path) = global_artifact_path(id) {
            if let Ok(bytes) = std::fs::read(&global_path) {
                return Self::parse(&bytes, id);
            }
        }
        // 3. Built-in: compiled into the binary
        let built_in = match id {
            "fpl-health" => include_str!("../../../config/artifacts/fpl-health.json"),
            "fpl-gpu" => include_str!("../../../config/artifacts/fpl-gpu.json"),
            "fpl-gpu-power" => include_str!("../../../config/artifacts/fpl-gpu-power.json"),
            "music-nowplaying" => include_str!("../../../config/artifacts/music-nowplaying.json"),
            "agent-activity" => include_str!("../../../config/artifacts/agent-activity.json"),
            _ => return Err(Error(format!("unknown artifact: {id}"))),
        };
        Self::parse(built_in.as_bytes(), id)
    }

    fn parse(bytes: &[u8], id: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error(format!("invalid artifact manifest {id}: {e}")))?;
        if manifest.id != id {
            return Err(Error(format!(
                "artifact manifest id mismatch: expected {id}, got {}",
                manifest.id
            )));
        }
        if manifest.schema_version != Self::MANIFEST_SCHEMA_VERSION {
            return Err(Error(format!(
                "artifact manifest {} has unsupported schema_version {} (expected {})",
                id,
                manifest.schema_version,
                Self::MANIFEST_SCHEMA_VERSION
            )));
        }
        // Validate the binder name matches the existing apply_named_adapter rules.
        if manifest.binder.is_empty()
            || manifest.binder.len() > 64
            || !manifest
                .binder
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error(format!(
                "artifact manifest {id} has invalid binder name: {}",
                manifest.binder
            )));
        }
        // Validate the widget kind is a known Canvas widget kind.
        if !canvas_widgets::is_known_widget_kind(&manifest.widget_kind) {
            return Err(Error(format!(
                "artifact manifest {} declares unknown widget kind: {}",
                id, manifest.widget_kind
            )));
        }
        Ok(manifest)
    }
}

/// Path to the global user-level artifact directory.

/// Ask the kernel for a larger receive queue on the stream socket.
///
/// Four games deciding at once burst far faster than one frame drains, and the
/// default queue is small enough that producers were getting ENOBUFS and
/// dropping snapshots. Best effort on purpose: a kernel that refuses the size
/// leaves the default in place, which is the behaviour we already had.
fn widen_receive_buffer(socket: &UnixDatagram) {
    use std::os::fd::AsRawFd;

    let wanted: libc::c_int = 4 * 1024 * 1024;
    // SAFETY: a valid fd owned by `socket`, and a correctly sized option value.
    let applied = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            std::ptr::from_ref(&wanted).cast(),
            std::mem::size_of_val(&wanted) as libc::socklen_t,
        )
    };
    if applied != 0 {
        eprintln!(
            "Canvas stream socket kept its default receive buffer: {}",
            std::io::Error::last_os_error()
        );
    }
}

fn global_artifact_path(id: &str) -> Option<PathBuf> {
    let state_root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })?;
    Some(
        state_root
            .join("dock/canvas/artifacts")
            .join(format!("{id}.json")),
    )
}

/// Re-exported from canvas-core so stream_binding callers can validate
/// without a separate import. See DECLARED_CONTRACTS and is_declared_contract
/// in canvas-core for the canonical registry.
pub use canvas_core::{DECLARED_CONTRACTS, is_declared_contract};

pub use crate::stream_contract::{STREAM_SCHEMA_VERSION, StreamEnvelope};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Every event ever accepted. Also the next log position to be assigned.
    pub received: u64,
    /// Events that fell out of the retention window. Eviction is normal — the
    /// window is finite by design — so this is not by itself a fault. A reader
    /// only loses something when `InboxRead::missed` is non-zero.
    pub evicted: u64,
    /// Events currently inside the window.
    pub retained: usize,
}

/// One read of the log, and what it cost the reader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InboxRead {
    pub events: Vec<StreamEnvelope>,
    /// Where the reader should ask from next.
    pub position: u64,
    /// Events this reader will never see, because they were evicted before it
    /// asked. Non-zero means a real gap: the reader fell behind the window, and
    /// whatever it renders next is discontinuous with what it rendered last.
    pub missed: u64,
}

/// A bounded retained log of stream events.
///
/// Readers hold a position rather than consuming, so a reader that reconnects
/// or falls behind can tell the difference between "nothing new" and "I missed
/// something" — which a drain queue cannot express, because a dropped event and
/// an idle producer look identical to it.
#[derive(Clone, Default)]
pub struct StreamInbox(Arc<Mutex<InboxState>>);

#[derive(Default)]
struct InboxState {
    retained: VecDeque<StreamEnvelope>,
    /// Log position of `retained.front()`. Everything below this is gone.
    base: u64,
    /// Total accepted; `base + retained.len() == received` always holds.
    received: u64,
    evicted: u64,
}

impl StreamInbox {
    pub fn push(&self, event: StreamEnvelope) {
        let mut state = self.0.lock().expect("stream inbox lock poisoned");
        state.received = state.received.saturating_add(1);
        if state.retained.len() == MAX_PENDING_EVENTS {
            state.retained.pop_front();
            state.base = state.base.saturating_add(1);
            state.evicted = state.evicted.saturating_add(1);
        }
        state.retained.push_back(event);
    }

    /// Read up to `limit` events starting at `position`.
    ///
    /// A position below the retained window is not an error — it is a gap, and
    /// it is reported rather than silently skipped.
    pub fn read_from(&self, position: u64, limit: usize) -> InboxRead {
        let state = self.0.lock().expect("stream inbox lock poisoned");
        let missed = state.base.saturating_sub(position);
        let start = position.max(state.base);
        // A position above the head means the caller is ahead of the log, which
        // only happens if the inbox was replaced underneath it. Treat it as
        // caught up rather than panicking on the subtraction.
        let offset = usize::try_from(start.saturating_sub(state.base)).unwrap_or(usize::MAX);
        let events: Vec<StreamEnvelope> = state
            .retained
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        InboxRead {
            position: start.saturating_add(events.len() as u64),
            missed,
            events,
        }
    }

    /// The next position that will be assigned. A reader starting here sees only
    /// what arrives after it asked.
    pub fn head(&self) -> u64 {
        self.0.lock().expect("stream inbox lock poisoned").received
    }

    /// Start close enough to the retained head to replay current state after a
    /// workspace switch without walking the entire history first.
    pub fn replay_tail(&self, limit: usize) -> u64 {
        let state = self.0.lock().expect("stream inbox lock poisoned");
        state.received.saturating_sub(limit as u64).max(state.base)
    }

    pub fn stats(&self) -> StreamStats {
        let state = self.0.lock().expect("stream inbox lock poisoned");
        StreamStats {
            received: state.received,
            evicted: state.evicted,
            retained: state.retained.len(),
        }
    }
}

pub struct StreamSocketGuard {
    path: PathBuf,
}

impl Drop for StreamSocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() }
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn socket_path() -> Result<PathBuf> {
    canvas_core::stream_socket_path()
        .ok_or_else(|| Error("Canvas stream socket home is not configured".into()))
}

pub fn start_server() -> Result<(StreamInbox, StreamSocketGuard)> {
    let path = socket_path()?;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(Error(
                "refusing to replace unsafe Canvas stream socket".into(),
            ));
        }
        fs::remove_file(&path).map_err(|error| Error(error.to_string()))?;
    }
    let socket = UnixDatagram::bind(&path).map_err(|error| Error(error.to_string()))?;
    widen_receive_buffer(&socket);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| Error(error.to_string()))?;
    let inbox = StreamInbox::default();
    let receiver = inbox.clone();
    std::thread::Builder::new()
        .name("canvas-stream".into())
        .spawn(move || receive(socket, receiver))
        .map_err(|error| Error(error.to_string()))?;
    Ok((inbox, StreamSocketGuard { path }))
}

fn receive(socket: UnixDatagram, inbox: StreamInbox) {
    let mut bytes = vec![0; MAX_STREAM_MESSAGE + 1];
    let mut consecutive_failures: u32 = 0;
    loop {
        let size = match socket.recv(&mut bytes) {
            Ok(size) => {
                consecutive_failures = 0;
                size
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                // Retiring this thread wedges Canvas silently: it is the only
                // reader of the stream socket, so every edge then blocks
                // forever on a full kernel buffer (ENOBUFS) while the socket
                // still exists. One transient error cost us the whole RLCD
                // feed until the app was restarted. Back off and keep serving.
                consecutive_failures = consecutive_failures.saturating_add(1);
                eprintln!(
                    "Canvas stream receive failed ({consecutive_failures} in a row): {error}"
                );
                std::thread::sleep(Duration::from_millis(
                    (50 * u64::from(consecutive_failures)).min(1_000),
                ));
                continue;
            }
        };
        if size > MAX_STREAM_MESSAGE {
            eprintln!("Canvas rejected oversized stream event");
            continue;
        }
        match serde_json::from_slice::<StreamEnvelope>(&bytes[..size])
            .map_err(|error| Error(format!("invalid stream event: {error}")))
            .and_then(|event| {
                event.validate()?;
                Ok(event)
            }) {
            Ok(event) => inbox.push(event),
            Err(error) => eprintln!("Canvas rejected stream event: {error}"),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn apply_named_adapter(
    adapter: &str,
    event: &StreamEnvelope,
    target: &mut WidgetSpec,
) -> Result<()> {
    apply_named_adapter_batch(adapter, &[event], target)
}

pub fn apply_named_adapter_batch(
    adapter: &str,
    events: &[&StreamEnvelope],
    target: &mut WidgetSpec,
) -> Result<()> {
    if adapter.is_empty()
        || !adapter
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(Error(format!("invalid stream adapter: {adapter}")));
    }
    let relative = PathBuf::from("binders").join(format!("{adapter}.lua"));
    if let Some(bytes) = crate::runtime_config::read(&relative, MAX_ADAPTER_BYTES as u64)? {
        let source = std::str::from_utf8(&bytes)
            .map_err(|error| Error(format!("Canvas adapter is not UTF-8: {error}")))?;
        return apply_lua_adapter_batch(source, events, target);
    }
    let source = built_in_adapter_source(adapter)
        .ok_or_else(|| Error(format!("unknown stream adapter: {adapter}")))?;
    apply_lua_adapter_batch(source, events, target)
}

/// Compiled-in binder sources. `binders_are_all_registered` asserts this
/// table matches `config/binders/`, so a new binder file can never be
/// added without registering it here too.
const BUILT_IN_BINDERS: &[(&str, &str)] = &[
    (
        "mev-opportunity-scene-v1",
        include_str!("../../../config/binders/mev-opportunity-scene-v1.lua"),
    ),
    (
        "mev-opportunity-profit-v1",
        include_str!("../../../config/binders/mev-opportunity-profit-v1.lua"),
    ),
    (
        "mev-opportunity-route-v1",
        include_str!("../../../config/binders/mev-opportunity-route-v1.lua"),
    ),
    (
        "mev-opportunity-evidence-v1",
        include_str!("../../../config/binders/mev-opportunity-evidence-v1.lua"),
    ),
    (
        "trader-backtest-v1",
        include_str!("../../../config/binders/trader-backtest-v1.lua"),
    ),
    (
        "activity-grid-v1",
        include_str!("../../../config/binders/activity-grid-v1.lua"),
    ),
    (
        "correlation-matrix-v1",
        include_str!("../../../config/binders/correlation-matrix-v1.lua"),
    ),
    (
        "hierarchy-v1",
        include_str!("../../../config/binders/hierarchy-v1.lua"),
    ),
    (
        "metric-board-v1",
        include_str!("../../../config/binders/metric-board-v1.lua"),
    ),
    (
        "distribution-v1",
        include_str!("../../../config/binders/distribution-v1.lua"),
    ),
    (
        "rich-card-v1",
        include_str!("../../../config/binders/rich-card-v1.lua"),
    ),
    (
        "approval-card-v1",
        include_str!("../../../config/binders/approval-card-v1.lua"),
    ),
    (
        "command-palette-v1",
        include_str!("../../../config/binders/command-palette-v1.lua"),
    ),
    (
        "media-transport-v1",
        include_str!("../../../config/binders/media-transport-v1.lua"),
    ),
    (
        "model-telemetry-v1",
        include_str!("../../../config/binders/model-telemetry-v1.lua"),
    ),
    (
        "log-stream-v1",
        include_str!("../../../config/binders/log-stream-v1.lua"),
    ),
    (
        "sankey-v1",
        include_str!("../../../config/binders/sankey-v1.lua"),
    ),
    (
        "service-topology-v1",
        include_str!("../../../config/binders/service-topology-v1.lua"),
    ),
    (
        "mycelium-topology-v1",
        include_str!("../../../config/binders/mycelium-topology-v1.lua"),
    ),
    (
        "span-timeline-v1",
        include_str!("../../../config/binders/span-timeline-v1.lua"),
    ),
    (
        "spectral-frame-v1",
        include_str!("../../../config/binders/spectral-frame-v1.lua"),
    ),
    (
        "spatial-tracks-v1",
        include_str!("../../../config/binders/spatial-tracks-v1.lua"),
    ),
    (
        "bifrost-requests-v1",
        include_str!("../../../config/binders/bifrost-requests-v1.lua"),
    ),
    (
        "bifrost-tokens-v1",
        include_str!("../../../config/binders/bifrost-tokens-v1.lua"),
    ),
    (
        "bifrost-cost-v1",
        include_str!("../../../config/binders/bifrost-cost-v1.lua"),
    ),
    (
        "bifrost-health-v1",
        include_str!("../../../config/binders/bifrost-health-v1.lua"),
    ),
    (
        "bifrost-backends-v1",
        include_str!("../../../config/binders/bifrost-backends-v1.lua"),
    ),
    (
        "bifrost-mesh-v1",
        include_str!("../../../config/binders/bifrost-mesh-v1.lua"),
    ),
    (
        "bifrost-models-v1",
        include_str!("../../../config/binders/bifrost-models-v1.lua"),
    ),
    (
        "shroud-regions-v1",
        include_str!("../../../config/binders/shroud-regions-v1.lua"),
    ),
    (
        "shroud-summary-v1",
        include_str!("../../../config/binders/shroud-summary-v1.lua"),
    ),
    (
        "shroud-fleet-stat-v1",
        include_str!("../../../config/binders/shroud-fleet-stat-v1.lua"),
    ),
    (
        "shroud-memory-gauge-v1",
        include_str!("../../../config/binders/shroud-memory-gauge-v1.lua"),
    ),
    (
        "shroud-disk-gauge-v1",
        include_str!("../../../config/binders/shroud-disk-gauge-v1.lua"),
    ),
    (
        "shroud-load-stat-v1",
        include_str!("../../../config/binders/shroud-load-stat-v1.lua"),
    ),
    (
        "shroud-vms-v1",
        include_str!("../../../config/binders/shroud-vms-v1.lua"),
    ),
    (
        "shroud-network-v1",
        include_str!("../../../config/binders/shroud-network-v1.lua"),
    ),
    (
        "fab-projects-v1",
        include_str!("../../../config/binders/fab-projects-v1.lua"),
    ),
    (
        "fab-runs-v1",
        include_str!("../../../config/binders/fab-runs-v1.lua"),
    ),
    (
        "fab-deliveries-v1",
        include_str!("../../../config/binders/fab-deliveries-v1.lua"),
    ),
    (
        "fab-metrics-v1",
        include_str!("../../../config/binders/fab-metrics-v1.lua"),
    ),
    (
        "fab-trends-v1",
        include_str!("../../../config/binders/fab-trends-v1.lua"),
    ),
    (
        "lockout-v1",
        include_str!("../../../config/binders/lockout-v1.lua"),
    ),
    (
        "market-ohlcv-v1",
        include_str!("../../../config/binders/market-ohlcv-v1.lua"),
    ),
    (
        "market-order-book-v1",
        include_str!("../../../config/binders/market-order-book-v1.lua"),
    ),
    (
        "market-trade-v1",
        include_str!("../../../config/binders/market-trade-v1.lua"),
    ),
    (
        "alfalfa-activity-v1",
        include_str!("../../../config/binders/alfalfa-activity-v1.lua"),
    ),
    (
        "alfalfa-fleet-heatmap-v1",
        include_str!("../../../config/binders/alfalfa-fleet-heatmap-v1.lua"),
    ),
    (
        "alfalfa-overview-v1",
        include_str!("../../../config/binders/alfalfa-overview-v1.lua"),
    ),
    (
        "alfalfa-pulse-v1",
        include_str!("../../../config/binders/alfalfa-pulse-v1.lua"),
    ),
    (
        "alfalfa-project-goals-v1",
        include_str!("../../../config/binders/alfalfa-project-goals-v1.lua"),
    ),
    (
        "alfalfa-project-summary-v1",
        include_str!("../../../config/binders/alfalfa-project-summary-v1.lua"),
    ),
    (
        "alfalfa-project-work-v1",
        include_str!("../../../config/binders/alfalfa-project-work-v1.lua"),
    ),
    (
        "alfalfa-project-agents-v1",
        include_str!("../../../config/binders/alfalfa-project-agents-v1.lua"),
    ),
    (
        "alfalfa-agent-dag-v1",
        include_str!("../../../config/binders/alfalfa-agent-dag-v1.lua"),
    ),
    (
        "alfalfa-project-reviewers-v1",
        include_str!("../../../config/binders/alfalfa-project-reviewers-v1.lua"),
    ),
    (
        "alfalfa-manifest-search-v1",
        include_str!("../../../config/binders/alfalfa-manifest-search-v1.lua"),
    ),
    (
        "agent-attention-v1",
        include_str!("../../../config/binders/agent-attention-v1.lua"),
    ),
    (
        "agent-context-v1",
        include_str!("../../../config/binders/agent-context-v1.lua"),
    ),
    (
        "feed-headlines-v1",
        include_str!("../../../config/binders/feed-headlines-v1.lua"),
    ),
    (
        "widget-snapshot-v1",
        include_str!("../../../config/binders/widget-snapshot-v1.lua"),
    ),
    (
        "transmog-build-v1",
        include_str!("../../../config/binders/transmog-build-v1.lua"),
    ),
    (
        "eval-watch-v1",
        include_str!("../../../config/binders/eval-watch-v1.lua"),
    ),
    (
        "run-tree-v1",
        include_str!("../../../config/binders/run-tree-v1.lua"),
    ),
    (
        "lob-pipeline-v1",
        include_str!("../../../config/binders/lob-pipeline-v1.lua"),
    ),
    (
        "music-nowplaying-v1",
        // Retained construction: Lua runs only when media state changes; the
        // resulting media card is rendered by Rust until the next snapshot.
        include_str!("../../../config/binders/music-nowplaying-v1.lua"),
    ),
    (
        "mpd-lyrics-v1",
        include_str!("../../../config/binders/mpd-lyrics-v1.lua"),
    ),
    (
        "jarvis-notifications-v1",
        include_str!("../../../config/binders/jarvis-notifications-v1.lua"),
    ),
];

fn built_in_adapter_source(adapter: &str) -> Option<&'static str> {
    BUILT_IN_BINDERS
        .iter()
        .find(|(name, _)| *name == adapter)
        .map(|(_, source)| *source)
}

pub fn apply_lua_adapter(
    source: &str,
    event: &StreamEnvelope,
    target: &mut WidgetSpec,
) -> Result<()> {
    apply_lua_adapter_batch(source, &[event], target)
}

/// Run one bounded delivery batch in one sandbox and swap state atomically.
pub fn apply_lua_adapter_batch(
    source: &str,
    events: &[&StreamEnvelope],
    target: &mut WidgetSpec,
) -> Result<()> {
    let values = run_lua_adapter_values(source, events)?;
    let mut next = target.clone();
    for value in values {
        let mutation: WidgetMutation = serde_json::from_value(value.clone()).map_err(|error| {
            Error(format!(
                "Canvas Lua adapter output is invalid: {error}; output={value}"
            ))
        })?;
        next.apply_mutation(mutation)
            .map_err(|error| Error(error.to_string()))?;
    }
    *target = next;
    Ok(())
}

/// Run a named Lua binder for a model timeline. The Lua owns tool semantics
/// (operation names, glyphs and component grouping); Rust receives only the
/// generic viewer contract.
pub fn apply_named_timeline_adapter(
    adapter: &str,
    event: &StreamEnvelope,
) -> Result<canvas_model_viewer::state::Timeline> {
    let relative = PathBuf::from("binders").join(format!("{adapter}.lua"));
    let owned;
    let source =
        if let Some(bytes) = crate::runtime_config::read(&relative, MAX_ADAPTER_BYTES as u64)? {
            owned = String::from_utf8(bytes)
                .map_err(|error| Error(format!("Canvas adapter is not UTF-8: {error}")))?;
            owned.as_str()
        } else {
            built_in_adapter_source(adapter)
                .ok_or_else(|| Error(format!("unknown stream adapter: {adapter}")))?
        };
    apply_lua_timeline_adapter(source, event)
}

pub fn apply_lua_timeline_adapter(
    source: &str,
    event: &StreamEnvelope,
) -> Result<canvas_model_viewer::state::Timeline> {
    let value = run_lua_adapter_values(source, &[event])?
        .pop()
        .expect("one input produces one output");
    serde_json::from_value(value.clone()).map_err(|error| {
        Error(format!(
            "Canvas Lua timeline output is invalid: {error}; output={value}"
        ))
    })
}

fn run_lua_adapter_values(
    source: &str,
    events: &[&StreamEnvelope],
) -> Result<Vec<serde_json::Value>> {
    if source.is_empty() || source.len() > MAX_ADAPTER_BYTES {
        return Err(Error("Lua adapter must contain 1-32768 bytes".into()));
    }
    if events.is_empty() || events.len() > 256 {
        return Err(Error("Lua adapter batch must contain 1-256 events".into()));
    }
    for event in events {
        event.validate()?;
    }
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(lua_error)?;
    lua.set_memory_limit(4 * 1024 * 1024).map_err(lua_error)?;
    let interrupts = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&interrupts);
    lua.set_interrupt(move |_| {
        if count.fetch_add(1, Ordering::Relaxed) >= 50_000 {
            Err(mlua::Error::RuntimeError(
                "Canvas adapter budget exceeded".into(),
            ))
        } else {
            Ok(VmState::Continue)
        }
    });
    let globals = lua.globals();
    for name in [
        "collectgarbage",
        "debug",
        "dofile",
        "getfenv",
        "io",
        "load",
        "loadfile",
        "loadstring",
        "os",
        "package",
        "require",
        "setfenv",
    ] {
        globals.raw_set(name, mlua::Value::Nil).map_err(lua_error)?;
    }
    let canvas = lua.create_table().map_err(lua_error)?;
    canvas
        .raw_set(
            "array",
            lua.create_function(|lua, table: Option<mlua::Table>| {
                let table = match table {
                    Some(table) => table,
                    None => lua.create_table()?,
                };
                table.set_metatable(Some(lua.array_metatable()))?;
                Ok(table)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    canvas
        .raw_set(
            "object",
            lua.create_function(|lua, table: Option<mlua::Table>| match table {
                Some(table) => Ok(table),
                None => lua.create_table(),
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    globals.raw_set("canvas", canvas).map_err(lua_error)?;
    lua.sandbox(true).map_err(lua_error)?;
    let wrapped = format!("return function(event)\n{source}\nend");
    let binder: Function = lua
        .load(&wrapped)
        .set_name("canvas-stream-adapter")
        .eval()
        .map_err(lua_error)?;
    let mut outputs = Vec::with_capacity(events.len());
    for event in events {
        let value: mlua::Value = binder
            .call(lua.to_value(*event).map_err(lua_error)?)
            .map_err(lua_error)?;
        outputs.push(lua.from_value(value).map_err(lua_error)?);
    }
    Ok(outputs)
}

fn lua_error(error: mlua::Error) -> Error {
    Error(format!("Canvas Lua adapter failed: {error}"))
}

/// The stream contract a briefing producer emits. Each envelope's payload
/// carries `{ beat: <index>, briefing: "<name>" }`; the binder renders the
/// beat into a `BriefingBeat` with workspace patches and optional TTS text.
pub use crate::stream_contract::BRIEFING_CONTRACT;

/// Run a briefing Lua binder against one beat event.
///
/// The binder source is sandboxed identically to widget adapters (TABLE,
/// STRING, MATH only; 4 MB memory; 50 000 instruction budget; no io/debug/os).
/// It receives the beat event and returns a [`BriefingBeat`] containing
/// workspace patches and optional narration text.
pub fn apply_briefing_binder(source: &str, event: &StreamEnvelope) -> Result<BriefingBeat> {
    if source.is_empty() || source.len() > MAX_ADAPTER_BYTES {
        return Err(Error("Lua briefing must contain 1-32768 bytes".into()));
    }
    event.validate()?;

    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(lua_error)?;
    lua.set_memory_limit(4 * 1024 * 1024).map_err(lua_error)?;
    let interrupts = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&interrupts);
    lua.set_interrupt(move |_| {
        if count.fetch_add(1, Ordering::Relaxed) >= 50_000 {
            Err(mlua::Error::RuntimeError(
                "Canvas briefing budget exceeded".into(),
            ))
        } else {
            Ok(VmState::Continue)
        }
    });
    let globals = lua.globals();
    for name in [
        "collectgarbage",
        "debug",
        "dofile",
        "getfenv",
        "io",
        "load",
        "loadfile",
        "loadstring",
        "os",
        "package",
        "require",
        "setfenv",
    ] {
        globals.raw_set(name, mlua::Value::Nil).map_err(lua_error)?;
    }
    lua.sandbox(true).map_err(lua_error)?;

    let wrapped = format!("return function(event)\n{source}\nend");
    let binder: Function = lua
        .load(&wrapped)
        .set_name("canvas-briefing-binder")
        .eval()
        .map_err(lua_error)?;

    let value = binder
        .call(lua.to_value(event).map_err(lua_error)?)
        .map_err(lua_error)?;

    let beat: BriefingBeat = lua.from_value(value).map_err(|error| {
        Error(format!(
            "Canvas briefing output is not a valid beat: {error}"
        ))
    })?;
    beat.validate().map_err(|error| Error(error.0))?;
    Ok(beat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_widgets::{
        WIDGET_SCHEMA_VERSION, WidgetChrome, WidgetKind,
        quant::{
            BacktestEvidence, BacktestPoint, BacktestSeries, BacktestSeriesRole, Candle,
            QuantWidget,
        },
    };

    #[test]
    fn replay_tail_starts_at_the_newest_bounded_window() {
        let inbox = StreamInbox::default();
        numbered(&inbox, 20);
        assert_eq!(inbox.replay_tail(6), 14);
        assert_eq!(inbox.read_from(inbox.replay_tail(6), 6).events.len(), 6);
    }

    fn test_spec(kind: WidgetKind) -> WidgetSpec {
        WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "probe".into(),
            kind,
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData::default(),
        }
    }

    fn spec() -> WidgetSpec {
        WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "live-es".into(),
            kind: WidgetKind::Quant {
                widget: QuantWidget::Candlestick {
                    symbol: "ESU6".into(),
                    interval: "5m".into(),
                    candles: vec![Candle {
                        time: 1,
                        open: 100.0,
                        high: 101.0,
                        low: 99.0,
                        close: 100.0,
                        volume: 1.0,
                    }],
                    show_volume: true,
                },
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData::default(),
        }
    }

    #[test]
    fn binders_are_all_registered() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/binders");
        let mut files = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".lua"))
            .collect::<Vec<_>>();
        files.sort();
        let mut registered: Vec<String> = BUILT_IN_BINDERS
            .iter()
            .map(|(name, _)| format!("{name}.lua"))
            .collect();
        registered.sort();
        assert_eq!(
            files, registered,
            "config/binders and BUILT_IN_BINDERS drifted"
        );
    }

    #[test]
    fn eval_watch_binder_projects_one_run_into_cohesive_views() {
        let payload = serde_json::json!({
            "schema":"canvas.run.watch.v1", "revision":4, "state":"running",
            "run":{"model":"Jev","harness":"Transmog","benchmark":"cad"},
            "artifactCount":2, "omittedEvents":0,
            "events":[
                {"id":"d1","kind":"decision","title":"wall strategy","confidence":0.8},
                {"id":"b1","kind":"build","title":"Extrude","detail":"400 triangles"},
                {"id":"v1","kind":"verification","title":"bounds","verdict":"pass"}
            ]
        });
        for (stream, kind) in [
            ("eval-summary", "status_list"),
            ("eval-decisions", "dag_graph"),
            ("eval-events", "event_log"),
            ("eval-verification", "status_list"),
        ] {
            let initial = match kind {
                "dag_graph" => WidgetKind::DagGraph {
                    title: "waiting".into(),
                    nodes: vec![],
                    edges: vec![],
                },
                "event_log" => WidgetKind::EventLog {
                    title: "waiting".into(),
                    entries: vec![],
                    query: None,
                    paused: false,
                    follow: true,
                },
                _ => WidgetKind::StatusList {
                    title: "waiting".into(),
                    nodes: vec![],
                },
            };
            let mut target = test_spec(initial);
            let event = StreamEnvelope {
                schema_version: STREAM_SCHEMA_VERSION,
                owner: "eval-studio".into(),
                stream: stream.into(),
                contract: "canvas.run.watch.v1".into(),
                sequence: 4,
                emitted_at_ms: now_ms(),
                project_id: Some("eval".into()),
                payload: payload.clone(),
            };
            apply_named_adapter("eval-watch-v1", &event, &mut target)
                .expect("eval run view renders");
            assert!(match (kind, &target.kind) {
                ("status_list", WidgetKind::StatusList { .. })
                | ("dag_graph", WidgetKind::DagGraph { .. })
                | ("event_log", WidgetKind::EventLog { .. }) => true,
                _ => false,
            });
        }
    }

    #[test]
    fn activity_grid_binder_builds_a_retained_themed_heatmap() {
        let mut target = test_spec(WidgetKind::Heatmap {
            title: "Waiting".into(),
            rows: vec!["Mon".into()],
            columns: vec!["W1".into()],
            values: vec![vec![0.0]],
            style: canvas_widgets::HeatmapStyle::Activity,
            unit: None,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "github".into(),
            stream: "activity".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "title": "Project activity",
                "rows": ["Mon", "Tue"],
                "columns": ["W1", "W2"],
                "values": [[0, 3], [1, 8]],
                "unit": "events"
            }),
        };
        apply_named_adapter("activity-grid-v1", &event, &mut target).expect("activity renders");
        let WidgetKind::Heatmap { style, values, .. } = target.kind else {
            panic!("activity binder changed widget family")
        };
        assert_eq!(style, canvas_widgets::HeatmapStyle::Activity);
        assert_eq!(values[1][1], 8.0);
    }

    #[test]
    fn mev_profit_binder_reports_net_not_gross_profit() {
        let mut target = test_spec(WidgetKind::Stat {
            label: "Waiting".into(),
            value: 0.0,
            unit: None,
            change: None,
            state: None,
            history: vec![],
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mev-observer".into(),
            stream: "opportunities".into(),
            contract: "mev.opportunity.v1".into(),
            project_id: Some("mev-observer".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "schema":"mev.opportunity.v1", "token":"WETH",
                "gross_profit":2740, "gas_cost":100, "builder_payment":50,
                "net_profit":2590, "profitable":true
            }),
        };
        apply_named_adapter("mev-opportunity-profit-v1", &event, &mut target)
            .expect("MEV net profit renders");
        let WidgetKind::Stat { label, value, .. } = target.kind else {
            panic!("MEV binder changed widget family")
        };
        assert_eq!(label, "NET OPPORTUNITY");
        assert_eq!(value, 0.000_000_000_000_002_59);
    }

    #[test]
    fn mev_scene_binder_builds_the_full_bleed_quant_instrument() {
        let mut target = test_spec(WidgetKind::Stat {
            label: "Waiting".into(),
            value: 0.0,
            unit: None,
            change: None,
            state: None,
            history: vec![],
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mev-observer".into(),
            stream: "opportunities".into(),
            contract: "mev.opportunity.v1".into(),
            project_id: Some("home".into()),
            sequence: 2,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "schema":"mev.opportunity.v1", "chain_id":1, "block_number":26088562,
                "block_hash":"0x28d50943000000000000000000000000000000000000000000000000ca1c9cd5",
                "token":"WETH", "route":["uniswap-v2","sushiswap-v2"],
                "amount_in":10000000000000000_u64, "amount_after_first":26603579,
                "amount_out":9936000000000000_u64, "gross_profit":-64000000000000_i64,
                "gas_cost":374000000000000_u64, "builder_payment":0, "net_profit":-438000000000000_i64,
                "profitable":false, "evaluated_candidates":500
            }),
        };
        apply_named_adapter("mev-opportunity-scene-v1", &event, &mut target)
            .expect("MEV scene renders");
        let WidgetKind::Quant {
            widget:
                QuantWidget::MevOpportunity {
                    route, net_profit, ..
                },
        } = target.kind
        else {
            panic!("MEV scene binder did not build its quant instrument")
        };
        assert_eq!(route, vec!["uniswap-v2", "sushiswap-v2"]);
        assert!((net_profit - -0.000438).abs() < f64::EPSILON);
    }

    #[test]
    fn trader_backtest_binder_preserves_evidence_and_research_metrics() {
        let mut target = test_spec(WidgetKind::Quant {
            widget: QuantWidget::Backtest {
                title: "Waiting".into(),
                evidence: BacktestEvidence::SyntheticProof,
                market: vec![],
                regimes: vec![],
                series: vec![BacktestSeries {
                    label: "Waiting".into(),
                    role: BacktestSeriesRole::Strategy,
                    points: vec![BacktestPoint {
                        time: 0,
                        value: 0.0,
                    }],
                }],
                trades: vec![],
                stats: vec![],
            },
        });
        let fixed = |mantissa| serde_json::json!({"mantissa":mantissa,"scale":4});
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "trader".into(),
            stream: "backtest".into(),
            contract: "fpl.trader.backtest-visualization/v1".into(),
            sequence: 1,
            emitted_at_ms: now_ms(),
            project_id: Some("trader".into()),
            payload: serde_json::json!({
                "schema":"fpl.trader.backtest-visualization/v1", "strategy_name":"SMA crossover",
                "equity_curve":[{"sequence":1,"equity":10000.0,"cash":0,"market_value":10000},{"sequence":2,"equity":10125.0,"cash":0,"market_value":10125}],
                "summary":{"run_id":"sha256:research-proof","execution_count":4,"report":{"evidence":"synthetic_proof","strategy":{"total_return":fixed(125),"maximum_drawdown":fixed(-75),"sharpe_ratio":fixed(15100),"sortino_ratio":null,"closed_trades":2},"benchmark":{"total_return":fixed(50)},"strategy_minus_benchmark":{"total_return":fixed(75)}}}
            }),
        };
        apply_named_adapter("trader-backtest-v1", &event, &mut target)
            .expect("Trader projection renders");
        let WidgetKind::Quant {
            widget:
                QuantWidget::Backtest {
                    evidence,
                    series,
                    stats,
                    ..
                },
        } = target.kind
        else {
            panic!("backtest binder changed widget family")
        };
        assert_eq!(
            evidence,
            canvas_widgets::quant::BacktestEvidence::SyntheticProof
        );
        assert_eq!(series[0].points.len(), 2);
        assert_eq!(stats[0].value, "+1.25%");
    }

    #[test]
    fn correlation_binder_builds_a_square_hoverable_matrix() {
        let mut target = test_spec(WidgetKind::Heatmap {
            title: "Waiting".into(),
            rows: vec!["waiting".into()],
            columns: vec!["waiting".into()],
            values: vec![vec![1.0]],
            style: canvas_widgets::HeatmapStyle::Matrix,
            unit: None,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "metrics".into(),
            stream: "correlation".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "labels":["latency","errors"],
                "values":[[1.0,0.82],[0.82,1.0]]
            }),
        };
        apply_named_adapter("correlation-matrix-v1", &event, &mut target)
            .expect("correlation renders");
        let WidgetKind::Heatmap {
            rows,
            columns,
            values,
            ..
        } = target.kind
        else {
            panic!("correlation binder changed widget family")
        };
        assert_eq!(rows, columns);
        assert_eq!(values[0][1], 0.82);
    }

    #[test]
    fn hierarchy_binder_switches_view_without_changing_the_data_contract() {
        for style in ["treemap", "sunburst"] {
            let mut target = test_spec(WidgetKind::Hierarchy {
                title: "Waiting".into(),
                style: canvas_widgets::HierarchyStyle::Treemap,
                nodes: vec![canvas_widgets::HierarchyNode {
                    id: "root".into(),
                    label: "Root".into(),
                    parent: None,
                    value: 1.0,
                }],
                unit: None,
                focus: None,
            });
            let event = StreamEnvelope {
                schema_version: STREAM_SCHEMA_VERSION,
                owner: "metrics".into(),
                stream: "hierarchy".into(),
                contract: "canvas.widget.snapshot.v1".into(),
                project_id: Some("dock".into()),
                sequence: 1,
                emitted_at_ms: now_ms(),
                payload: serde_json::json!({"style":style,"nodes":[
                    {"id":"root","label":"Root","value":10},
                    {"id":"leaf","label":"Leaf","parent":"root","value":10}
                ]}),
            };
            apply_named_adapter("hierarchy-v1", &event, &mut target).expect("hierarchy renders");
            let WidgetKind::Hierarchy {
                style: actual,
                nodes,
                ..
            } = target.kind
            else {
                panic!("wrong family")
            };
            assert_eq!(nodes.len(), 2);
            assert_eq!(
                actual == canvas_widgets::HierarchyStyle::Sunburst,
                style == "sunburst"
            );
        }
    }

    #[test]
    fn metric_board_binder_shares_targets_across_radial_and_bullet_views() {
        for style in ["radial", "bullet"] {
            let mut target = test_spec(WidgetKind::MetricBoard {
                title: "Waiting".into(),
                style: canvas_widgets::MetricBoardStyle::Radial,
                metrics: vec![canvas_widgets::TargetMetric {
                    label: "CPU".into(),
                    value: 1.0,
                    max: 100.0,
                    target: None,
                    forecast: None,
                    state: None,
                }],
                unit: None,
            });
            let event = StreamEnvelope {
                schema_version: STREAM_SCHEMA_VERSION,
                owner: "metrics".into(),
                stream: "targets".into(),
                contract: "canvas.widget.snapshot.v1".into(),
                project_id: Some("dock".into()),
                sequence: 1,
                emitted_at_ms: now_ms(),
                payload: serde_json::json!({"style":style,"metrics":[{"label":"CPU","value":67,"max":100,"target":80,"forecast":74,"state":"good"}]}),
            };
            apply_named_adapter("metric-board-v1", &event, &mut target)
                .expect("metric board renders");
            let WidgetKind::MetricBoard {
                style: actual,
                metrics,
                ..
            } = target.kind
            else {
                panic!("wrong family")
            };
            assert_eq!(metrics[0].target, Some(80.0));
            assert_eq!(
                actual == canvas_widgets::MetricBoardStyle::Bullet,
                style == "bullet"
            );
        }
    }

    #[test]
    fn distribution_binder_preserves_samples_for_box_and_violin_views() {
        let mut target = test_spec(WidgetKind::Distribution {
            title: "Waiting".into(),
            style: canvas_widgets::DistributionStyle::Box,
            groups: vec![canvas_widgets::DistributionGroup {
                label: "api".into(),
                samples: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            }],
            unit: None,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "metrics".into(),
            stream: "latency".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({"style":"violin","groups":[{"label":"api","samples":[1,2,3,4,5,8]}]}),
        };
        apply_named_adapter("distribution-v1", &event, &mut target).expect("distribution renders");
        let WidgetKind::Distribution { style, groups, .. } = target.kind else {
            panic!("wrong family")
        };
        assert_eq!(style, canvas_widgets::DistributionStyle::Violin);
        assert_eq!(groups[0].samples.len(), 6);
    }

    #[test]
    fn interaction_card_binders_keep_actions_and_content_typed() {
        let mut rich = test_spec(WidgetKind::RichCard {
            title: "Waiting".into(),
            format: canvas_widgets::RichCardFormat::Markdown,
            body: "wait".into(),
            language: None,
            citations: Vec::new(),
        });
        let base = |stream: &str, payload| StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "jarvis".into(),
            stream: stream.into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload,
        };
        apply_named_adapter("rich-card-v1",&base("briefing",serde_json::json!({"format":"code","title":"Example","body":"let x = 1;","language":"rust"})),&mut rich).unwrap();
        assert!(matches!(
            rich.kind,
            WidgetKind::RichCard {
                format: canvas_widgets::RichCardFormat::Code,
                ..
            }
        ));

        let mut approval = test_spec(WidgetKind::ApprovalCard {
            request_id: "r".into(),
            title: "Waiting".into(),
            summary: "wait".into(),
            scope: vec!["read".into()],
            expires_at: "soon".into(),
            provenance: "agent".into(),
            approve_action: "yes".into(),
            deny_action: "no".into(),
            status: canvas_widgets::ApprovalStatus::Pending,
        });
        apply_named_adapter("approval-card-v1",&base("approval",serde_json::json!({"request_id":"deploy-1","summary":"Deploy","scope":["host:pi"],"approve_action":"deploy.approve","deny_action":"deploy.deny"})),&mut approval).unwrap();
        let WidgetKind::ApprovalCard {
            approve_action,
            deny_action,
            ..
        } = &approval.kind
        else {
            panic!("wrong family")
        };
        assert_ne!(approve_action, deny_action);

        let mut commands = test_spec(WidgetKind::CommandPalette {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
        });
        apply_named_adapter("command-palette-v1",&base("commands",serde_json::json!({"query":"open","results":[{"id":"ohm","label":"Ohm","kind":"project"}]})),&mut commands).unwrap();
        let WidgetKind::CommandPalette { results, .. } = commands.kind else {
            panic!("wrong family")
        };
        assert_eq!(results[0].id, "ohm");
    }

    #[test]
    fn media_and_model_binders_preserve_transport_and_annotation_semantics() {
        let base = |stream: &str, payload| StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "edge".into(),
            stream: stream.into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload,
        };
        let mut media = test_spec(WidgetKind::MediaTransport {
            title: "Waiting".into(),
            source: "x".into(),
            codec: "x".into(),
            width: 1,
            height: 1,
            fps: 0.0,
            bitrate_kbps: 0.0,
            buffer_ms: 0.0,
            delay_ms: 0.0,
            route: vec!["x".into()],
            state: None,
        });
        apply_named_adapter("media-transport-v1",&base("media",serde_json::json!({"source":"studio","codec":"h264","width":1280,"height":720,"fps":24,"bitrate_kbps":3200,"buffer_ms":72,"delay_ms":118,"route":["studio","pi"]})),&mut media).unwrap();
        let WidgetKind::MediaTransport {
            route, delay_ms, ..
        } = media.kind
        else {
            panic!("wrong family")
        };
        assert_eq!(route.len(), 2);
        assert_eq!(delay_ms, 118.0);

        let mut model = test_spec(WidgetKind::ModelTelemetry {
            title: "Waiting".into(),
            model: "m".into(),
            view: canvas_widgets::ModelView::Isometric,
            annotations: Vec::new(),
            exploded: 0.0,
        });
        apply_named_adapter("model-telemetry-v1",&base("model",serde_json::json!({"model":"benchy.glb","view":"top","exploded":0.2,"annotations":[{"id":"bow","position":[-0.6,0.1,0.4],"value":"2mm"}]})),&mut model).unwrap();
        let WidgetKind::ModelTelemetry {
            view,
            annotations,
            exploded,
            ..
        } = model.kind
        else {
            panic!("wrong family")
        };
        assert_eq!(view, canvas_widgets::ModelView::Top);
        assert_eq!(annotations.len(), 1);
        assert_eq!(exploded, 0.2);
    }

    #[test]
    fn log_binder_exposes_filter_pause_and_follow_as_snapshot_state() {
        let mut target = test_spec(WidgetKind::EventLog {
            title: "Waiting".into(),
            entries: Vec::new(),
            query: None,
            paused: false,
            follow: true,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "runtime".into(),
            stream: "logs".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "entries":[{"time":"12:00:00","line":"decoder late","severity":"warn"}],
                "query":"decoder", "paused":true, "follow":false
            }),
        };
        apply_named_adapter("log-stream-v1", &event, &mut target).expect("logs render");
        let WidgetKind::EventLog {
            entries,
            query,
            paused,
            follow,
            ..
        } = target.kind
        else {
            panic!("log binder changed widget family")
        };
        assert_eq!(entries[0].level, canvas_widgets::StatusLevel::Warning);
        assert_eq!(query.as_deref(), Some("decoder"));
        assert!(paused);
        assert!(!follow);
    }

    #[test]
    fn service_topology_binder_reuses_the_retained_graph_contract() {
        let mut target = test_spec(WidgetKind::NeuralGraph {
            title: "Waiting".into(),
            nodes: Vec::new(),
            edges: Vec::new(),
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "unibus".into(),
            stream: "service-topology".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "nodes": [
                    {"id":"edge","name":"Edge","health":"healthy","activity":0.7},
                    {"id":"api","name":"API","health":"degraded","activity":1.0}
                ],
                "edges": [{"source":"edge","target":"api","activity":0.8}]
            }),
        };
        apply_named_adapter("service-topology-v1", &event, &mut target).expect("topology renders");
        let WidgetKind::NeuralGraph { nodes, edges, .. } = target.kind else {
            panic!("topology binder changed widget family")
        };
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].state, canvas_widgets::StatusLevel::Warning);
        assert_eq!(edges[0].heat, 0.8);
    }

    #[test]
    fn mycelium_topology_binder_preserves_network_domains() {
        let mut target = test_spec(WidgetKind::NetworkTopology {
            title: "Waiting".into(),
            segments: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            conflicts: 0,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mycelium".into(),
            stream: "topology".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("home".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "segments": {
                    "pris/vlan:35":{"id":"pris/vlan:35","kind":"vlan","vlan":35,"origins":["pris/router:config"]},
                    "pris/192.168.35.0/24":{"id":"pris/192.168.35.0/24","kind":"lan","subnet":["192.168.35.0",24],"gw":"192.168.35.1","origins":["pris/router:config"]},
                    "titan/vlan:35":{"id":"titan/vlan:35","kind":"vlan","vlan":35,"origins":["titan/router:config"]},
                    "titan/192.168.35.0/24":{"id":"titan/192.168.35.0/24","kind":"lan","subnet":["192.168.35.0",24],"gw":"192.168.35.1","origins":["titan/router:config"]}
                },
                "nodes": {
                    "pris-router":{"id":"pris-router","device":true,"sites":["pris"],"ips":{"192.168.35.1":{"addr":"192.168.35.1","origins":["pris/router:ip"]}},"hostnames":["pris-router"],"ports":{}},
                    "titan-router":{"id":"titan-router","device":true,"sites":["titan"],"ips":{"192.168.35.1":{"addr":"192.168.35.1","origins":["titan/router:ip"]}},"hostnames":["titan-router"],"ports":{}}
                },
                "conflicts": []
            }),
        };
        apply_lua_adapter(
            include_str!("../../../config/binders/mycelium-topology-v1.lua"),
            &event,
            &mut target,
        )
        .expect("Mycelium topology renders");
        let WidgetKind::NetworkTopology {
            segments, nodes, ..
        } = target.kind
        else {
            panic!("Mycelium binder changed widget family")
        };
        assert_eq!(
            segments.len(),
            2,
            "site-local subnet views fold into their VLANs: {segments:#?}"
        );
        let pris_segment = segments
            .iter()
            .find(|segment| segment.site.as_deref() == Some("pris"))
            .expect("pris segment");
        let titan_segment = segments
            .iter()
            .find(|segment| segment.site.as_deref() == Some("titan"))
            .expect("titan segment");
        assert_eq!(pris_segment.id, "pris/vlan:35");
        assert_eq!(titan_segment.id, "titan/vlan:35");
        let pris_node = nodes
            .iter()
            .find(|node| node.id == "pris-router")
            .expect("pris node");
        let titan_node = nodes
            .iter()
            .find(|node| node.id == "titan-router")
            .expect("titan node");
        assert_eq!(pris_node.sites, ["pris"]);
        assert_eq!(pris_node.segments, ["pris/vlan:35"]);
        assert_eq!(titan_node.sites, ["titan"]);
        assert_eq!(titan_node.segments, ["titan/vlan:35"]);
    }

    #[test]
    fn span_timeline_binder_selects_semantics_without_owning_frames() {
        let mut target = test_spec(WidgetKind::SpanTimeline {
            title: "Waiting".into(),
            style: canvas_widgets::SpanTimelineStyle::Waterfall,
            spans: vec![canvas_widgets::TimedSpan {
                id: "waiting".into(),
                label: "Waiting".into(),
                start: 0.0,
                end: 1.0,
                lane: None,
                parent: None,
                state: None,
                detail: None,
            }],
            unit: None,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "traces".into(),
            stream: "request".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "title":"Request waterfall", "style":"waterfall", "unit":"ms",
                "spans":[
                    {"id":"root","label":"request","start":0,"end":42,"state":"good"},
                    {"id":"db","label":"database","start":8,"end":30,"parent":"root","state":"warning"}
                ]
            }),
        };
        apply_named_adapter("span-timeline-v1", &event, &mut target).expect("timeline renders");
        let WidgetKind::SpanTimeline {
            style, spans, unit, ..
        } = target.kind
        else {
            panic!("timeline binder changed widget family")
        };
        assert_eq!(style, canvas_widgets::SpanTimelineStyle::Waterfall);
        assert_eq!(spans.len(), 2);
        assert_eq!(unit.as_deref(), Some("ms"));
    }

    #[test]
    fn spectral_binder_appends_frames_without_running_lua_per_tick() {
        let mut target = test_spec(WidgetKind::SpectralWaterfall {
            title: "FM spectrum".into(),
            rows: Vec::new(),
            min_frequency: 88.0,
            max_frequency: 108.0,
            unit: Some("MHz".into()),
            history: 8,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "sdr".into(),
            stream: "fft".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({"bins":[0.0,0.1,0.2,0.9,0.4,0.2,0.1,0.0]}),
        };
        apply_named_adapter("spectral-frame-v1", &event, &mut target)
            .expect("spectrum frame appends");
        let WidgetKind::SpectralWaterfall { rows, .. } = target.kind else {
            panic!("spectral binder changed widget family")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][3], 0.9);
    }

    #[test]
    fn spatial_tracks_binder_keeps_tracking_out_of_the_renderer() {
        let mut target = test_spec(WidgetKind::SpatialTracks {
            title: "Waiting".into(),
            tracks: Vec::new(),
            bounds: canvas_widgets::SpatialBounds {
                min_x: 0.0,
                max_x: 1.0,
                min_z: 0.0,
                max_z: 1.0,
            },
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "ruview".into(),
            stream: "tracks".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("dock".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "title":"Studio occupancy",
                "bounds":{"min_x":0,"max_x":8,"min_z":0,"max_z":5},
                "tracks":[{
                    "id":7,
                    "position":{"x":5.9,"z":3.4},
                    "trail":[{"x":4.1,"z":2.3},{"x":5.9,"z":3.4}],
                    "confidence":0.93,
                    "motion":68
                }]
            }),
        };
        apply_named_adapter("spatial-tracks-v1", &event, &mut target)
            .expect("spatial snapshot renders");
        let WidgetKind::SpatialTracks { tracks, bounds, .. } = target.kind else {
            panic!("spatial binder changed widget family")
        };
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, 7);
        assert_eq!(tracks[0].trail.len(), 2);
        assert_eq!(bounds.max_x, 8.0);
    }

    #[test]
    fn alfalfa_manifest_search_is_presented_by_lua() {
        let mut target = test_spec(WidgetKind::Table {
            title: "Files".into(),
            columns: Vec::new(),
            rows: Vec::new(),
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "canvas-artifact-index".into(),
            stream: "transmog-artifacts".into(),
            contract: canvas_core::project_browser::SEARCH_CONTRACT.into(),
            project_id: Some("transmog".into()),
            sequence: 8,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "project_id":"transmog", "revision":"r9", "query":"pdf",
                "matched":1,
                "results":[{"id":"drawing","path":"drawings/case.pdf","display_name":"Case drawing","cid":"bafycase","media_type":"application/pdf","bytes":2048,"modified_at_ms":7}]
            }),
        };
        apply_named_adapter("alfalfa-manifest-search-v1", &event, &mut target).unwrap();
        let WidgetKind::Table { title, rows, .. } = target.kind else {
            panic!("manifest search binder changed widget family")
        };
        assert_eq!(title, "pdf");
        assert_eq!(rows[0], ["Case drawing", "PDF", "drawings", "2 KB"]);
        assert_eq!(target.data.window.as_deref(), Some("revision r9"));
    }

    #[test]
    fn music_binder_constructs_a_retained_media_card_on_event_only() {
        let mut target = test_spec(WidgetKind::MediaCard {
            title: "Waiting for playback".into(),
            artist: String::new(),
            album: None,
            source: None,
            artwork_source: Some("now-playing-art".into()),
            elapsed_seconds: 0.0,
            duration_seconds: 0.0,
            observed_at_ms: 0,
            playing: false,
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "home-pi-media".into(),
            stream: "now-playing".into(),
            contract: "canvas.music.nowplaying.v1".into(),
            project_id: Some("home".into()),
            sequence: 7,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "project":"home", "track":"Everything In Its Right Place",
                "artist":"Radiohead", "album":"Kid A", "source":"Spotify",
                "elapsed_sec":42, "duration_sec":251, "is_playing":true
            }),
        };
        apply_lua_adapter(
            include_str!("../../../config/binders/music-nowplaying-v1.lua"),
            &event,
            &mut target,
        )
        .unwrap();
        let WidgetKind::MediaCard {
            title,
            artist,
            elapsed_seconds,
            duration_seconds,
            observed_at_ms,
            playing,
            ..
        } = target.kind
        else {
            panic!("music binder changed widget family")
        };
        assert_eq!(title, "Everything In Its Right Place");
        assert_eq!(artist, "Radiohead");
        assert_eq!(elapsed_seconds, 42.0);
        assert_eq!(duration_seconds, 251.0);
        assert_eq!(observed_at_ms, event.emitted_at_ms as i64);
        assert!(playing);
        assert_eq!(target.data.state, canvas_widgets::WidgetDataState::Ready);
        assert!(target.data.source.as_deref().unwrap().contains("Spotify"));
    }

    fn event(time: i64, close: f64) -> StreamEnvelope {
        StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "demo".into(),
            stream: "esu6".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: time as u64,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({ "ts": time, "o": 100.0,
                "h": close.max(101.0), "l": 99.0, "c": close, "v": 12.0 }),
        }
    }

    #[test]
    fn adapter_coalesces_forming_candle() {
        let mut target = spec();
        apply_named_adapter("market-ohlcv-v1", &event(1, 100.5), &mut target).unwrap();
        let WidgetKind::Quant { widget } = target.kind else {
            panic!("quant")
        };
        let QuantWidget::Candlestick { candles, .. } = widget else {
            unreachable!()
        };
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].close, 100.5);
    }

    #[test]
    fn malformed_event_does_not_mutate_target() {
        let mut target = spec();
        let before = target.clone();
        let mut malformed = event(2, 102.0);
        malformed.payload = serde_json::json!({"ts": 2});
        assert!(apply_named_adapter("market-ohlcv-v1", &malformed, &mut target).is_err());
        assert_eq!(target, before);
    }

    #[test]
    fn inbox_never_grows_without_bound() {
        let inbox = StreamInbox::default();
        for sequence in 0..(MAX_PENDING_EVENTS as u64 + 20) {
            let mut item = event(sequence as i64, 100.0);
            item.sequence = sequence;
            inbox.push(item);
        }
        let stats = inbox.stats();
        assert_eq!(stats.retained, MAX_PENDING_EVENTS);
        assert_eq!(stats.evicted, 20);
        assert_eq!(stats.received, MAX_PENDING_EVENTS as u64 + 20);
    }

    fn numbered(inbox: &StreamInbox, count: u64) {
        for sequence in 0..count {
            let mut item = event(sequence as i64, 100.0);
            item.sequence = sequence;
            inbox.push(item);
        }
    }

    #[test]
    fn a_reader_that_keeps_up_sees_every_event_exactly_once() {
        let inbox = StreamInbox::default();
        numbered(&inbox, 5);
        let first = inbox.read_from(0, 3);
        assert_eq!(first.missed, 0);
        assert_eq!(first.position, 3);
        assert_eq!(
            first.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        let second = inbox.read_from(first.position, 3);
        assert_eq!(second.missed, 0);
        assert_eq!(second.position, 5);
        assert_eq!(
            second.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![3, 4]
        );

        // Reading again with nothing new is idle, not a gap.
        let third = inbox.read_from(second.position, 3);
        assert!(third.events.is_empty());
        assert_eq!(third.missed, 0);
        assert_eq!(third.position, 5);
    }

    #[test]
    fn reading_does_not_consume_so_two_readers_are_independent() {
        let inbox = StreamInbox::default();
        numbered(&inbox, 4);
        let a = inbox.read_from(0, 4);
        let b = inbox.read_from(0, 4);
        assert_eq!(a.events, b.events);
        assert_eq!(inbox.stats().retained, 4);
    }

    #[test]
    fn a_reader_that_falls_behind_the_window_is_told_how_much_it_lost() {
        let inbox = StreamInbox::default();
        numbered(&inbox, MAX_PENDING_EVENTS as u64 + 20);

        // Still parked at the origin while 20 events aged out beneath it.
        let read = inbox.read_from(0, 4);
        assert_eq!(
            read.missed, 20,
            "a gap must be reported, not silently skipped"
        );
        assert_eq!(
            read.events.first().map(|e| e.sequence),
            Some(20),
            "the read resumes at the oldest event still retained"
        );
        assert_eq!(read.position, 24);

        // Having been told, the reader is caught up and loses nothing further.
        assert_eq!(inbox.read_from(read.position, 4).missed, 0);
    }

    #[test]
    fn starting_at_head_skips_history_without_reporting_a_gap() {
        let inbox = StreamInbox::default();
        numbered(&inbox, 10);
        let head = inbox.head();
        assert_eq!(head, 10);
        let read = inbox.read_from(head, 8);
        assert!(read.events.is_empty());
        assert_eq!(
            read.missed, 0,
            "deliberately joining live is not the same as falling behind"
        );

        numbered(&inbox, 2);
        let next = inbox.read_from(head, 8);
        assert_eq!(next.events.len(), 2);
        assert_eq!(next.missed, 0);
    }

    #[test]
    fn a_position_beyond_the_head_reads_as_caught_up_rather_than_panicking() {
        let inbox = StreamInbox::default();
        numbered(&inbox, 3);
        let read = inbox.read_from(9_000, 4);
        assert!(read.events.is_empty());
        assert_eq!(read.missed, 0);
        assert_eq!(read.position, 9_000);
    }

    #[test]
    fn generic_snapshot_adapter_updates_data_but_preserves_surface_spec() {
        let mut target = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "health".into(),
            kind: WidgetKind::Stat {
                label: "Backend health".into(),
                value: 0.0,
                unit: None,
                change: None,
                state: None,
                history: Vec::new(),
            },
            theme: Some("project".into()),
            refresh_ms: Some(5_000),
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData {
                state: canvas_widgets::WidgetDataState::Loading,
                source: Some("prometheus:bifrost/health".into()),
                ..Default::default()
            },
        };
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "bifrost".into(),
            stream: "prometheus-health".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "widget": {
                    "kind": "stat",
                    "label": "Backend health",
                    "value": 1.0,
                    "state": "good"
                },
                "data": {
                    "state": "ready",
                    "source": "prometheus:bifrost/health",
                    "observed_at": "2026-09-09T00:00:00Z",
                    "window": "now"
                }
            }),
        };

        apply_named_adapter("widget-snapshot-v1", &event, &mut target).unwrap();
        assert_eq!(target.id, "health");
        assert_eq!(target.theme.as_deref(), Some("project"));
        assert_eq!(target.refresh_ms, Some(5_000));
        assert_eq!(target.data.state, canvas_widgets::WidgetDataState::Ready);
        let WidgetKind::Stat { value, state, .. } = target.kind else {
            panic!("stat")
        };
        assert_eq!(value, 1.0);
        assert_eq!(state, Some(canvas_widgets::StatusLevel::Good));
    }

    #[test]
    fn alfalfa_binders_render_their_widget_kinds() {
        // The seam: these payloads are built with the producer's own
        // projections (bead_rows carries the severity), so passing here means
        // producer + binder agree on the wire, not on a hand-copied fixture.
        let beads = serde_json::json!([
            {"id": "dock-8gr", "status": "closed", "title": "Boot restore", "blocked": false},
            {"id": "dock-1x", "status": "open", "title": "Blocked thing", "blocked": true},
        ]);
        let marbles = canvas_alfalfa::bead_rows(beads.as_array(), 6);
        let payload = serde_json::json!({
            "project": "dock",
            "state": "active",
            "activity": 75,
            "active": 2, "ready": 1, "backlog": 110, "idle": "25%",
            "marbles": marbles,
            "pulls": [["pull", "#12", "open", "Widgets", "good"]],
        });
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "alfalfa".into(),
            stream: "dock-pulse".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: payload.clone(),
        };

        let mut pulse = test_spec(WidgetKind::EventLog {
            title: "stale".into(),
            entries: Vec::new(),
            query: None,
            paused: false,
            follow: true,
        });
        apply_named_adapter("alfalfa-pulse-v1", &event, &mut pulse).expect("eventlog renders");
        let WidgetKind::EventLog { title, entries, .. } = &pulse.kind else {
            panic!("eventlog, got {:?}", pulse.kind)
        };
        assert_eq!(title, "dock — recent marbles");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].timestamp, "dock-8gr");
        assert_eq!(entries[0].level, canvas_widgets::StatusLevel::Neutral);
        assert_eq!(entries[1].level, canvas_widgets::StatusLevel::Critical);
        assert_eq!(entries[2].message, "PR Widgets open");

        let mut gauge = test_spec(WidgetKind::Gauge {
            value: 0.0,
            label: None,
        });
        apply_named_adapter("alfalfa-activity-v1", &event, &mut gauge).expect("gauge renders");
        let WidgetKind::Gauge { value, label } = &gauge.kind else {
            panic!("gauge")
        };
        assert_eq!(*value, 75.0);
        assert_eq!(label.as_deref(), Some("dock activity"));

        let fleet_rows = serde_json::json!([
            ["dock", "active", 2, 1, 110, "25%"],
            ["JARVIS", "paused", 0, 0, 31, "100%"],
        ]);
        let overview = serde_json::json!({
            "projects": 2, "active": 2, "ready": 1, "backlog": 141, "idle": 1,
            "rows": fleet_rows, "truncated": false,
            "matrix": {
                "columns": ["act", "rdy", "bky"],
                "rows": ["dock", "JARVIS"],
                "values": [[2, 1, 110], [0, 0, 31]],
            },
        });
        let overview_event = StreamEnvelope {
            owner: "alfalfa".into(),
            stream: "overview".into(),
            payload: overview,
            ..event.clone()
        };
        let mut fleet = test_spec(WidgetKind::StatusList {
            title: "stale".into(),
            nodes: Vec::new(),
        });
        apply_named_adapter("alfalfa-overview-v1", &overview_event, &mut fleet)
            .expect("status list renders");
        let WidgetKind::StatusList { nodes, .. } = &fleet.kind else {
            panic!("status list")
        };
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[1].label, "dock");
        assert_eq!(nodes[1].state, canvas_widgets::StatusLevel::Good);
        assert_eq!(nodes[2].state, canvas_widgets::StatusLevel::Warning);

        let mut heat = test_spec(WidgetKind::Heatmap {
            title: "stale".into(),
            rows: Vec::new(),
            columns: Vec::new(),
            values: Vec::new(),
            style: canvas_widgets::HeatmapStyle::Matrix,
            unit: None,
        });
        apply_named_adapter("alfalfa-fleet-heatmap-v1", &overview_event, &mut heat)
            .expect("heatmap renders");
        let WidgetKind::Heatmap { rows, values, .. } = &heat.kind else {
            panic!("heatmap")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(values[1][2], 31.0);

        let dashboard_event = StreamEnvelope {
            stream: "ohm-dashboard".into(),
            project_id: Some("ohm".into()),
            payload: serde_json::json!({
                "workspace_id": "ohm", "name": "Ohm", "activity_state": "observed",
                "goals": {"total": 4, "active": 1, "queued": 2, "items": [{"id": "g1", "outcome": "Ship", "status": "active"}]},
                "beads": {"open": 3, "in_progress": 1, "blocked": 1, "ready": 1, "items": [{"id": "ohm-1", "title": "Wire metrics", "status": "blocked", "ready": false}]},
                "agents": {"total": 2, "running": 1, "items": []},
                "reviewers": {"total": 1, "running": 1}
            }),
            ..event
        };
        let mut summary = test_spec(WidgetKind::StatusList {
            title: "stale".into(),
            nodes: Vec::new(),
        });
        apply_named_adapter("alfalfa-project-summary-v1", &dashboard_event, &mut summary)
            .expect("dashboard summary renders");
        let WidgetKind::StatusList { title, nodes } = &summary.kind else {
            panic!("dashboard status list")
        };
        assert_eq!(title, "Ohm pulse");
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[1].state, canvas_widgets::StatusLevel::Critical);

        let mut work = test_spec(WidgetKind::Stat {
            label: "stale".into(),
            value: 0.0,
            unit: None,
            change: None,
            state: None,
            history: Vec::new(),
        });
        apply_named_adapter("alfalfa-project-work-v1", &dashboard_event, &mut work)
            .expect("dashboard marbles render");
        let WidgetKind::Stat {
            label,
            value,
            state,
            ..
        } = &work.kind
        else {
            panic!("dashboard marble stat")
        };
        assert_eq!(label, "Open marbles");
        assert_eq!(*value, 3.0);
        assert_eq!(*state, Some(canvas_widgets::StatusLevel::Critical));

        let mut agents = test_spec(WidgetKind::Stat {
            label: "stale".into(),
            value: 0.0,
            unit: None,
            change: None,
            state: None,
            history: Vec::new(),
        });
        apply_named_adapter("alfalfa-project-agents-v1", &dashboard_event, &mut agents)
            .expect("dashboard agents render");
        let WidgetKind::Stat { value, .. } = agents.kind else {
            panic!("agent stat")
        };
        assert_eq!(value, 1.0);

        let mut lineage = test_spec(WidgetKind::DagGraph {
            title: "stale".into(),
            nodes: Vec::new(),
            edges: Vec::new(),
        });
        let mut lineage_event = dashboard_event.clone();
        lineage_event.payload["agents"]["items"] = serde_json::json!([
            {"id":"root","role":"orchestrator","state":"working"},
            {"id":"child","parent_id":"root","role":"reviewer","state":"idle"}
        ]);
        apply_named_adapter("alfalfa-agent-dag-v1", &lineage_event, &mut lineage)
            .expect("agent lineage renders");
        let WidgetKind::DagGraph {
            ref nodes,
            ref edges,
            ..
        } = lineage.kind
        else {
            panic!("agent lineage DAG")
        };
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "root");
        assert_eq!(edges[0].to, "child");

        lineage_event.payload["agents"]["items"] = serde_json::json!([]);
        lineage_event.payload["activity_state"] = serde_json::json!("unavailable");
        apply_named_adapter("alfalfa-agent-dag-v1", &lineage_event, &mut lineage)
            .expect("empty agent lineage renders an honest data state");
        assert_eq!(lineage.data.state, canvas_widgets::WidgetDataState::Partial);
    }

    #[test]
    fn fab_binders_render_owner_streams() {
        let cases = [
            (
                "fab-projects-v1",
                serde_json::json!({"projects":[{"id":"fab","name":"Fab","status":"active"}]}),
            ),
            (
                "fab-metrics-v1",
                serde_json::json!({"totals":{"projects":1,"recentRuns":2},"worker":{"concurrency":4}}),
            ),
            (
                "fab-runs-v1",
                serde_json::json!({"recent_runs":[{"project":"fab","status":"succeeded"}]}),
            ),
            (
                "fab-deliveries-v1",
                serde_json::json!({"recent_deliveries":[{"repository":"fab","status":"succeeded"}]}),
            ),
            (
                "fab-trends-v1",
                serde_json::json!({"period":"4m","series":[{"label":"Active runs","points":[[0,1],[5,2]]}]}),
            ),
        ];
        for (adapter, payload) in cases {
            let event = StreamEnvelope {
                schema_version: STREAM_SCHEMA_VERSION,
                owner: "fab".into(),
                stream: "test".into(),
                contract: "canvas.widget.snapshot.v1".into(),
                project_id: Some("fab".into()),
                sequence: 1,
                emitted_at_ms: now_ms(),
                payload,
            };
            let mut target = test_spec(if adapter.contains("trends") {
                WidgetKind::TimeSeries {
                    title: "loading".into(),
                    series: Vec::new(),
                    unit: None,
                }
            } else if adapter.contains("runs") || adapter.contains("deliveries") {
                WidgetKind::EventLog {
                    title: "loading".into(),
                    entries: Vec::new(),
                    query: None,
                    paused: false,
                    follow: true,
                }
            } else {
                WidgetKind::StatusList {
                    title: "loading".into(),
                    nodes: Vec::new(),
                }
            });
            apply_named_adapter(adapter, &event, &mut target).unwrap_or_else(|error| {
                panic!("{adapter} failed: {error}");
            });
            assert_eq!(target.data.state, canvas_widgets::WidgetDataState::Ready);
        }
    }

    #[test]
    fn bifrost_binders_render_aggregate_production_streams() {
        let cases = [
            ("bifrost-requests-v1", serde_json::json!({"requests":12})),
            ("bifrost-tokens-v1", serde_json::json!({"tokens":3400})),
            ("bifrost-cost-v1", serde_json::json!({"cost_usd":1.25})),
            (
                "bifrost-health-v1",
                serde_json::json!({"failures":0,"throttled":1,"mean_duration_seconds":0.8}),
            ),
            (
                "bifrost-backends-v1",
                serde_json::json!({"backends":[{"backend":"openai","up":1,"models":3,"circuit_open":0}]}),
            ),
            (
                "bifrost-mesh-v1",
                serde_json::json!({"nodes":[{"node":"agora-one","up":1,"utilization_pct":25,"memory_used_bytes":1024,"memory_bytes":2048,"power_watts":90}]}),
            ),
            (
                "bifrost-models-v1",
                serde_json::json!({"period":"Since gateway restart","models":[{"model":"fpl/llm","value":1200}]}),
            ),
        ];
        for (adapter, payload) in cases {
            let event = StreamEnvelope {
                schema_version: STREAM_SCHEMA_VERSION,
                owner: "bifrost".into(),
                stream: "test".into(),
                contract: "canvas.widget.snapshot.v1".into(),
                project_id: Some("bifrost".into()),
                sequence: 1,
                emitted_at_ms: now_ms(),
                payload,
            };
            let kind = if adapter.contains("models") {
                WidgetKind::PieChart {
                    title: "loading".into(),
                    values: vec![canvas_widgets::BarValue {
                        label: "waiting".into(),
                        value: 1.0,
                        state: None,
                    }],
                    unit: None,
                    period: None,
                }
            } else if adapter.contains("backends") || adapter.contains("mesh") {
                WidgetKind::Table {
                    title: "loading".into(),
                    columns: Vec::new(),
                    rows: Vec::new(),
                }
            } else if adapter.contains("health") {
                WidgetKind::StatusList {
                    title: "loading".into(),
                    nodes: Vec::new(),
                }
            } else {
                WidgetKind::Stat {
                    label: "loading".into(),
                    value: 0.0,
                    unit: None,
                    change: None,
                    history: Vec::new(),
                    state: None,
                }
            };
            let mut target = test_spec(kind);
            apply_named_adapter(adapter, &event, &mut target)
                .unwrap_or_else(|error| panic!("{adapter} failed: {error}"));
            assert_eq!(target.data.state, canvas_widgets::WidgetDataState::Ready);
        }
    }
    #[test]
    fn feed_headlines_survives_an_emoji_splitting_the_clip_boundary() {
        // The live failure: a headline long enough that the binder's 55-byte
        // clip landed inside a 4-byte emoji, so Lua returned an invalid-UTF-8
        // string, mlua surfaced it as a byte buffer, and serde rejected the
        // whole batch ("expected a string") — the reel sat empty on the wall.
        let mut target = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "feed-hn".into(),
            kind: WidgetKind::Table {
                title: "Hacker News".into(),
                columns: ["#", "story", "pts", "who", "age"]
                    .iter()
                    .map(|c| (*c).to_string())
                    .collect(),
                rows: Vec::new(),
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData::default(),
        };
        let mut title = "a".repeat(50);
        title.push_str("😀😀 reaches the cut");
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "feeds".into(),
            stream: "hn-headlines".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "source": "hn",
                "title": "Hacker News",
                "rows": [{
                    "title": title,
                    "url": "https://news.ycombinator.com/item?id=1",
                    "score": 42,
                    "by": "alice",
                    "age_secs": 120,
                }],
            }),
        };
        apply_named_adapter("feed-headlines-v1", &event, &mut target)
            .expect("emoji titles must not poison the batch");
        let WidgetKind::Table { rows, .. } = &target.kind else {
            panic!("table")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][2], "42");
        assert_eq!(rows[0][3], "alice");
        assert!(rows[0][1].ends_with('…'), "clipped with ellipsis");
    }

    #[test]
    fn feed_headlines_keeps_a_multibyte_dash_ending_exactly_at_the_clip_edge() {
        // The live HN shape that poisoned a whole batch: a 51-char ASCII
        // title, an em dash (3 bytes) at 53..55, and clip's limit of 55 —
        // walking back to the sequence start and cutting there left a lone
        // 0xE2. The dash must survive whole.
        let mut target = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "feed-hn".into(),
            kind: WidgetKind::Table {
                title: "Hacker News".into(),
                columns: ["#", "story", "pts", "who", "age"]
                    .iter()
                    .map(|c| (*c).to_string())
                    .collect(),
                rows: Vec::new(),
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData::default(),
        };
        let title = "Why don't machine learning research agents overfit?";
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "feeds".into(),
            stream: "hn-headlines".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "source": "hn",
                "title": "Hacker News",
                "rows": [{
                    "title": title,
                    "url": "https://www.amazon.science/blog/x",
                    "score": 43,
                    "by": "someone",
                    "age_secs": 6413,
                }],
            }),
        };
        apply_named_adapter("feed-headlines-v1", &event, &mut target)
            .expect("a dash ending at the clip edge must not split");
        let WidgetKind::Table { rows, .. } = &target.kind else {
            panic!("table")
        };
        assert!(
            rows[0][1].ends_with("—…"),
            "dash kept whole before the ellipsis: {:?}",
            rows[0][1]
        );
    }

    #[test]
    fn feed_headlines_renders_a_sparse_rss_payload() {
        // xkcd-style rows: title+url only, no score/author/age. Mirrors the
        // live wire shape byte-for-byte (serde skips None fields).
        let mut target = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "feed-rss".into(),
            kind: WidgetKind::Table {
                title: "XKCD".into(),
                columns: ["#", "story", "pts", "who", "age"]
                    .iter()
                    .map(|c| (*c).to_string())
                    .collect(),
                rows: Vec::new(),
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: canvas_widgets::WidgetData::default(),
        };
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "feeds".into(),
            stream: "rss-headlines".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "source": "rss",
                "title": "XKCD",
                "rows": [
                    {"title": "OH Scale", "url": "https://xkcd.com/3297/", "source": "rss"},
                    {"title": "Fault Taunting", "url": "https://xkcd.com/3296/", "source": "rss"},
                ],
            }),
        };
        apply_named_adapter("feed-headlines-v1", &event, &mut target)
            .expect("sparse rss rows must render");
        let WidgetKind::Table { title, rows, .. } = &target.kind else {
            panic!("table")
        };
        assert_eq!(title, "XKCD");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], "OH Scale — xkcd.com");
        assert_eq!(rows[0][2], "-");
    }

    #[test]
    fn project_id_scopes_event_routing() {
        // An event tagged with project_id "cortex" should NOT be delivered
        // to a workspace whose project_id is "alfalfa".
        let mut ev = event(1, 100.0);
        ev.project_id = Some("cortex".into());
        let inbox = StreamInbox::default();
        inbox.push(ev);
        let read = inbox.read_from(0, 3);
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].project_id, Some("cortex".to_string()));
        // The caller (drain_stream_events) filters by project_id match;
        // the inbox itself preserves all events. The routing gate is in
        // main.rs drain_stream_events, verified by integration tests.
    }

    #[test]
    fn event_without_project_id_is_unscoped() {
        let ev = event(1, 100.0);
        assert!(
            ev.project_id.is_none(),
            "legacy events have no project scope"
        );
        ev.validate().unwrap();
    }

    #[test]
    fn an_undeclared_contract_is_refused_at_ingress() {
        let mut ev = event(1, 100.0);
        ev.contract = "fabricated.stream.v1".into();
        assert!(
            ev.validate().is_err(),
            "an undeclared contract must be rejected at ingress"
        );
        let error = ev.validate().unwrap_err().to_string();
        assert!(
            error.contains("undeclared stream contract"),
            "error must name the contract: {error}"
        );
    }

    #[test]
    fn a_declared_contract_passes_validation() {
        let ev = event(1, 100.0);
        assert!(
            is_declared_contract(&ev.contract),
            "canvas.widget.snapshot.v1 must be declared"
        );
        ev.validate().unwrap();
    }

    #[test]
    fn a_builtin_artifact_loads_and_validates() {
        let manifest = ArtifactManifest::load("fpl-health").expect("fpl-health loads");
        assert_eq!(manifest.id, "fpl-health");
        assert_eq!(manifest.binder, "widget-snapshot-v1");
        assert_eq!(manifest.widget_kind, "stat");
        // The binder is a known adapter that already exists in apply_named_adapter.
        assert!(!manifest.binder.is_empty());
    }

    #[test]
    fn an_unknown_artifact_is_rejected() {
        assert!(ArtifactManifest::load("does-not-exist").is_err());
    }

    #[test]
    fn an_invalid_artifact_id_is_rejected() {
        assert!(ArtifactManifest::load("has_uppercase").is_err());
        assert!(ArtifactManifest::load("").is_err());
    }

    #[test]
    fn a_v1_manifest_round_trips() {
        let json = serde_json::json!({
            "schema_version": 1,
            "id": "test",
            "version": "0.1.0",
            "source": "prometheus:test/metric",
            "binder": "widget-snapshot-v1",
            "widget_kind": "stat",
            "provenance": "demo",
            "refresh_ms": 5000
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        // Built-in parse is tested via load(); this validates the schema.
        let manifest: ArtifactManifest = serde_json::from_slice(&bytes).expect("parses");
        assert_eq!(
            manifest.schema_version,
            ArtifactManifest::MANIFEST_SCHEMA_VERSION
        );
        assert_eq!(manifest.id, "test");
    }

    /// The stream reader is the only consumer of the Canvas stream socket, so
    /// exiting on a transient receive error leaves every edge publishing into a
    /// kernel buffer that fills and then rejects forever with ENOBUFS — the
    /// surfaces simply stop updating with nothing in the UI to explain it.
    #[test]
    fn reader_keeps_serving_after_transient_receive_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stream.sock");
        let socket = UnixDatagram::bind(&path).expect("bind stream socket");
        // Non-blocking turns every empty poll into a real recv error, which is
        // what the old loop treated as fatal.
        socket.set_nonblocking(true).expect("non-blocking");

        let inbox = StreamInbox::default();
        let reader = inbox.clone();
        std::thread::spawn(move || receive(socket, reader));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            inbox.stats().received,
            0,
            "nothing was sent yet, so nothing may have been ingested"
        );

        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mac-studio-speedy-smk".into(),
            stream: "retro-smk".into(),
            contract: "canvas.widget.snapshot.v1".into(),
            project_id: Some("speedy".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "widget": { "kind": "event_log", "title": "RLCD", "entries": [] }
            }),
        };
        let bytes = serde_json::to_vec(&event).expect("encode event");
        UnixDatagram::unbound()
            .expect("sender")
            .send_to(&bytes, &path)
            .expect("send event");

        // Worst case the reader is mid-backoff, so poll rather than sleep once.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while inbox.stats().received == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            inbox.stats().received,
            1,
            "reader retired itself on a transient error and never ingested the event"
        );
    }

    #[test]
    fn transmog_lua_normalizes_operation_icons_for_model_timeline() {
        let event = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mac-studio-transmog".into(),
            stream: "transmog-timeline".into(),
            contract: "transmog.build.status.v1".into(),
            project_id: Some("transmog".into()),
            sequence: 1,
            emitted_at_ms: now_ms(),
            payload: serde_json::json!({
                "total": 3,
                "done": 3,
                "complete": true,
                "source": "draft.ron",
                "material": "PETG",
                "process": "FDM",
                "stream_id": "transmog-step",
                "steps": [
                    {"index": 2, "kind": "Cut", "feature": 9},
                    {"index": 1, "kind": "Extrude", "feature": 8}
                ]
            }),
        };
        let timeline = apply_lua_timeline_adapter(
            include_str!("../../../config/binders/transmog-build-v1.lua"),
            &event,
        )
        .expect("timeline renders");
        assert_eq!(timeline.stream_id, "transmog-step");
        assert_eq!(timeline.items[0].glyph, "↥");
        assert_eq!(timeline.items[0].steps_back, 1);
        assert_eq!(timeline.items[1].glyph, "−");
        assert_eq!(timeline.items[1].steps_back, 0);
    }
}
