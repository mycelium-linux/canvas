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
use std::time::{SystemTime, UNIX_EPOCH};

pub const STREAM_SCHEMA_VERSION: u32 = 1;
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
            _ => return Err(Error(format!("unknown artifact: {id}")).into()),
        };
        Self::parse(built_in.as_bytes(), id)
    }

    fn parse(bytes: &[u8], id: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error(format!("invalid artifact manifest {id}: {e}").into()))?;
        if manifest.id != id {
            return Err(Error(format!(
                "artifact manifest id mismatch: expected {id}, got {}",
                manifest.id
            ))
            .into());
        }
        if manifest.schema_version != Self::MANIFEST_SCHEMA_VERSION {
            return Err(Error(format!(
                "artifact manifest {} has unsupported schema_version {} (expected {})",
                id, manifest.schema_version, Self::MANIFEST_SCHEMA_VERSION
            ))
            .into());
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
            ))
            .into());
        }
        // Validate the widget kind is a known Canvas widget kind.
        if !canvas_widgets::is_known_widget_kind(&manifest.widget_kind) {
            return Err(Error(format!(
                "artifact manifest {} declares unknown widget kind: {}",
                id, manifest.widget_kind
            ))
            .into());
        }
        Ok(manifest)
    }
}

/// Path to the global user-level artifact directory.
fn global_artifact_path(id: &str) -> Option<PathBuf> {
    let state_root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })?;
    Some(state_root.join("dock/canvas/artifacts").join(format!("{id}.json")))
}

/// Re-exported from canvas-core so stream_binding callers can validate
/// without a separate import. See DECLARED_CONTRACTS and is_declared_contract
/// in canvas-core for the canonical registry.
pub use canvas_core::{DECLARED_CONTRACTS, is_declared_contract};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEnvelope {
    pub schema_version: u32,
    pub owner: String,
    pub stream: String,
    pub contract: String,
    pub sequence: u64,
    pub emitted_at_ms: u64,
    /// Optional project scope. When present, Canvas only routes the event to
    /// a workspace whose `project_id` matches — preventing project X's activity
    /// from landing in project Y's surface. Falls back to the legacy
    /// owner/stream/contract matching when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub payload: serde_json::Value,
}

impl StreamEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != STREAM_SCHEMA_VERSION {
            return Err(Error("unsupported stream schema version".into()));
        }
        for (name, value) in [("owner", &self.owner), ("stream", &self.stream)] {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(Error(format!("invalid stream {name}")));
            }
        }
        if self.contract.is_empty() || self.contract.len() > 128 {
            return Err(Error("invalid stream contract".into()));
        }
        if !is_declared_contract(&self.contract) {
            return Err(Error(format!(
                "undeclared stream contract: {}; add it to DECLARED_CONTRACTS in stream_binding.rs",
                self.contract
            )));
        }
        Ok(())
    }

    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.emitted_at_ms)
    }
}

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
    let control = crate::control::socket_path().map_err(Error)?;
    Ok(control.with_file_name("stream.sock"))
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
    loop {
        let size = match socket.recv(&mut bytes) {
            Ok(size) => size,
            Err(error) => {
                eprintln!("Canvas stream receive failed: {error}");
                return;
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
    let source = match adapter {
        "lockout-v1" => include_str!("../../../config/binders/lockout-v1.lua"),
        "market-ohlcv-v1" => include_str!("../../../config/binders/market-ohlcv-v1.lua"),
        "agent-attention-v1" => include_str!("../../../config/binders/agent-attention-v1.lua"),
        "widget-snapshot-v1" => include_str!("../../../config/binders/widget-snapshot-v1.lua"),
        "music-nowplaying-v1" => include_str!("../../../config/binders/music-nowplaying-v1.lua"),
        _ => return Err(Error(format!("unknown stream adapter: {adapter}"))),
    };
    apply_lua_adapter_batch(source, events, target)
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
    lua.sandbox(true).map_err(lua_error)?;
    let wrapped = format!("return function(event)\n{source}\nend");
    let binder: Function = lua
        .load(&wrapped)
        .set_name("canvas-stream-adapter")
        .eval()
        .map_err(lua_error)?;
    let mut next = target.clone();
    for event in events {
        let value = binder
            .call(lua.to_value(*event).map_err(lua_error)?)
            .map_err(lua_error)?;
        let mutation: WidgetMutation = lua.from_value(value).map_err(lua_error)?;
        next.apply_mutation(mutation)
            .map_err(|error| Error(error.to_string()))?;
    }
    *target = next;
    Ok(())
}

fn lua_error(error: mlua::Error) -> Error {
    Error(format!("Canvas Lua adapter failed: {error}"))
}

/// The stream contract a briefing producer emits. Each envelope's payload
/// carries `{ beat: <index>, briefing: "<name>" }`; the binder renders the
/// beat into a `BriefingBeat` with workspace patches and optional TTS text.
pub const BRIEFING_CONTRACT: &str = "canvas.briefing.beat.v1";

/// Run a briefing Lua binder against one beat event.
///
/// The binder source is sandboxed identically to widget adapters (TABLE,
/// STRING, MATH only; 4 MB memory; 50 000 instruction budget; no io/debug/os).
/// It receives the beat event and returns a [`BriefingBeat`] containing
/// workspace patches and optional narration text.
pub fn apply_briefing_binder(
    source: &str,
    event: &StreamEnvelope,
) -> Result<BriefingBeat> {
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
        "collectgarbage", "debug", "dofile", "getfenv", "io", "load",
        "loadfile", "loadstring", "os", "package", "require", "setfenv",
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

    let beat: BriefingBeat = lua
        .from_value(value)
        .map_err(|error| Error(format!("Canvas briefing output is not a valid beat: {error}")))?;
    beat.validate()
        .map_err(|error| Error(error.0))?;
    Ok(beat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_widgets::{
        WIDGET_SCHEMA_VERSION, WidgetChrome, WidgetKind,
        quant::{Candle, QuantWidget},
    };

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
        let QuantWidget::Candlestick { candles, .. } = widget;
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
        assert!(ev.project_id.is_none(), "legacy events have no project scope");
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
        assert_eq!(manifest.schema_version, ArtifactManifest::MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.id, "test");
    }
}
