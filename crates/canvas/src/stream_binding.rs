//! Bounded owner-local stream ingress and sandboxed event-to-widget adapters.

use crate::{Error, Result};
use canvas_widgets::{WidgetMutation, WidgetSpec};
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
const MAX_ADAPTER_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEnvelope {
    pub schema_version: u32,
    pub owner: String,
    pub stream: String,
    pub contract: String,
    pub sequence: u64,
    pub emitted_at_ms: u64,
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
        Ok(())
    }

    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.emitted_at_ms)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub received: u64,
    pub dropped: u64,
    pub queue_depth: usize,
}

#[derive(Clone, Default)]
pub struct StreamInbox(Arc<Mutex<InboxState>>);

#[derive(Default)]
struct InboxState {
    pending: VecDeque<StreamEnvelope>,
    received: u64,
    dropped: u64,
}

impl StreamInbox {
    pub fn push(&self, event: StreamEnvelope) {
        let mut state = self.0.lock().expect("stream inbox lock poisoned");
        state.received = state.received.saturating_add(1);
        if state.pending.len() == MAX_PENDING_EVENTS {
            state.pending.pop_front();
            state.dropped = state.dropped.saturating_add(1);
        }
        state.pending.push_back(event);
    }

    pub fn drain(&self, limit: usize) -> Vec<StreamEnvelope> {
        let mut state = self.0.lock().expect("stream inbox lock poisoned");
        let count = limit.min(state.pending.len());
        state.pending.drain(..count).collect()
    }

    pub fn stats(&self) -> StreamStats {
        let state = self.0.lock().expect("stream inbox lock poisoned");
        StreamStats {
            received: state.received,
            dropped: state.dropped,
            queue_depth: state.pending.len(),
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
    let source = match adapter {
        "market-ohlcv-v1" => include_str!("../../../config/binders/market-ohlcv-v1.lua"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_widgets::{
        WidgetChrome, WidgetKind,
        quant::{Candle, QuantWidget},
    };

    fn spec() -> WidgetSpec {
        WidgetSpec {
            schema_version: 1,
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
            contract: "market.ohlcv.raw.v1".into(),
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
        assert_eq!(stats.queue_depth, MAX_PENDING_EVENTS);
        assert_eq!(stats.dropped, 20);
    }
}
