//! canvas-edge: attaches this machine's Canvas runtime to a unibus hub.
//!
//! Registers local displays as screen anchors, then translates bus traffic
//! into the Canvas control protocol with an explicit authority model
//! (dock marbles `dock-rfwf` follow-up):
//!
//! - `presence_update` is data: any hub-granted publisher may feed the rail.
//! - `canvas_command` is authority: only producers listed in `actors` may
//!   command, only the commands each is granted, always stamped with that
//!   actor's identity, and always answered with a `canvas.result` receipt so
//!   the central learns what *executed*, not just what was delivered.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas::control::{
    Command, ControlRequest, Outcome, PresenceOccupant, PresenceSignal, PresenceSignalState,
    PresenceSnapshot,
};
use canvas::stream_binding::{STREAM_SCHEMA_VERSION, StreamEnvelope};
use canvas::workspace_mirror::{
    LocalWorkspaceSnapshot, MIRROR_KIND, MIRROR_SCHEMA_VERSION, WorkspaceState,
};
use canvas_core::unibus::{Envelope, envelope, kinds, result_message};
use serde::Deserialize;

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(15);
/// macOS's default `net.local.dgram.maxdgram` is 2048 bytes. Leave framing
/// headroom so every local stream snapshot remains portable across hosts.
const MAX_LOCAL_STREAM_DATAGRAM: usize = 1_900;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeConfig {
    #[serde(flatten)]
    bus: BusConfig,
    /// Producers permitted to command this Canvas, with their granted
    /// command names (control-protocol variant names, snake_case).
    #[serde(default)]
    actors: Vec<BusActor>,
    /// Opt-in shared representation. Edges in the same zone exchange only
    /// durable workspace documents; video and live data remain local.
    #[serde(default)]
    mirror: Option<MirrorConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConfig {
    #[serde(default = "mirror_enabled")]
    enabled: bool,
    /// Destination zone. Defaults to the edge's first registered zone.
    #[serde(default)]
    zone: Option<String>,
}

fn mirror_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusActor {
    producer: String,
    /// Actor identity stamped onto accepted commands, e.g. "opencode:central".
    id: String,
    /// Command variant names this producer may execute. Empty means "observe
    /// only": presence flows, commands are refused.
    #[serde(default)]
    commands: Vec<String>,
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-edge <edge-config.json>")?;
    let config = EdgeConfig::read(std::path::Path::new(&path))?;
    let mut backoff = RECONNECT_MIN;
    loop {
        match run(&config) {
            Ok(()) => eprintln!("canvas-edge: hub connection closed; reconnecting"),
            Err(e) => eprintln!("canvas-edge: {e:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

impl EdgeConfig {
    fn read(path: &std::path::Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read edge config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse edge config {}", path.display()))?;
        for actor in &config.actors {
            ensure!(
                !actor.producer.trim().is_empty(),
                "actor with empty producer"
            );
            ensure!(!actor.id.trim().is_empty(), "actor with empty id");
        }
        Ok(config)
    }
}

fn run(config: &EdgeConfig) -> Result<()> {
    let bus = Arc::new(
        Bus::connect(config.bus.clone())
            .with_context(|| format!("connect hub {}", config.bus.hub_endpoint))?,
    );
    eprintln!(
        "canvas-edge: registered as '{}' via {} (anchors {:?}, actors {:?})",
        config.bus.producer_id,
        config.bus.hub_endpoint,
        config.bus.screen_anchors,
        config
            .actors
            .iter()
            .map(|a| a.producer.as_str())
            .collect::<Vec<_>>(),
    );
    let mut liveness = Liveness::default();
    let mirror_runtime = Arc::new(Mutex::new(MirrorRuntime::default()));
    let _mirror_socket =
        start_workspace_mirror(config, Arc::clone(&bus), Arc::clone(&mirror_runtime))?;
    loop {
        match bus.try_read() {
            Ok(Some((envelope, _payload))) => {
                if let Err(e) = ingest(&bus, config, &mut liveness, &mirror_runtime, &envelope) {
                    eprintln!("canvas-edge: rejected {}: {e:#}", envelope.message.kind);
                }
            }
            Ok(None) => {}
            Err(e) => return Err(e.context("hub read")),
        }
    }
}

/// Indicators this edge applied on behalf of each bus producer, so a
/// `unibus.node_state` offline announcement can clear what the dead
/// producer left on screen (the stuck "thinking" pill class of bug).
#[derive(Default)]
struct Liveness {
    by_producer: HashMap<String, BTreeSet<String>>,
    rlcd_by_session: HashMap<String, VecDeque<serde_json::Value>>,
    mirror_sequences: HashMap<String, u64>,
}

impl Liveness {
    fn record(&mut self, producer: &str, indicator_id: &str) {
        self.by_producer
            .entry(producer.to_owned())
            .or_default()
            .insert(indicator_id.to_owned());
    }

    fn forget(&mut self, producer: &str, indicator_id: &str) {
        if let Some(set) = self.by_producer.get_mut(producer) {
            set.remove(indicator_id);
        }
    }

    fn take_for_dismissal(&mut self, producer: &str) -> Vec<String> {
        self.by_producer
            .remove(producer)
            .map_or_else(Vec::new, |s| s.into_iter().collect())
    }
}

/// One inbound bus frame -> control action + optional receipt.
fn ingest(
    bus: &Bus,
    config: &EdgeConfig,
    liveness: &mut Liveness,
    mirror_runtime: &Arc<Mutex<MirrorRuntime>>,
    envelope: &Envelope,
) -> Result<()> {
    match envelope.message.kind.as_str() {
        "presence_update" | "presence.update" => {
            let summary = envelope
                .message
                .body
                .get("snapshot")
                .context("missing snapshot")?;
            let snapshot = presence_snapshot(summary)?;
            submit(ControlRequest {
                schema_version: canvas::control::CONTROL_VERSION,
                request_id: format!("presence-{}", envelope::now_ms()),
                expected_revision: None,
                actor: None,
                command: Command::SetPresence { presence: snapshot },
            })
            .context("SetPresence")
        }
        "canvas_command" | "canvas.command" => {
            let raw = envelope
                .message
                .body
                .get("command")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let request_id = raw
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or(envelope.request_id.as_str())
                .to_owned();
            let variant = raw
                .pointer("/command/command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let indicator_id = raw
                .pointer("/command/indicator/id")
                .or_else(|| raw.pointer("/command/indicator_id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let outcome = match authorize(config, envelope, &raw) {
                Ok(request) => match submit(request) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(format!("{e:#}"))),
                },
                Err(reason) => {
                    eprintln!("canvas-edge: denied: {reason}");
                    (false, Some(reason))
                }
            };
            if outcome.0 {
                if let Some(id) = indicator_id {
                    match variant.as_str() {
                        "upsert_indicator" => {
                            liveness.record(&envelope.producer_id, &id);
                        }
                        "dismiss_indicator" => {
                            liveness.forget(&envelope.producer_id, &id);
                        }
                        _ => {}
                    }
                }
            }
            receipt(bus, envelope, &request_id, outcome.0, outcome.1)
        }
        "audio_level" | "audio.level" => publish_audio_level(envelope),
        "rlcd.decision.v1" => publish_rlcd_decision(liveness, envelope),
        "spatial_tracks" | "spatial.tracks" => publish_spatial_tracks(envelope),
        MIRROR_KIND => apply_workspace_mirror(config, liveness, mirror_runtime, envelope),
        kinds::V1_REPLY | kinds::REPLY => Ok(()),
        canvas_core::unibus::kinds::NODE_STATE => {
            let online = envelope.message.body["online"].as_bool().unwrap_or(true);
            if !online {
                let producer = envelope.message.body["producer_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                for indicator_id in liveness.take_for_dismissal(&producer) {
                    eprintln!(
                        "canvas-edge: producer '{producer}' died; dismissing '{indicator_id}'"
                    );
                    let _ = submit(ControlRequest {
                        schema_version: canvas::control::CONTROL_VERSION,
                        request_id: format!("liveness-{indicator_id}-{}", envelope::now_ms()),
                        expected_revision: None,
                        actor: None,
                        command: Command::DismissIndicator { indicator_id },
                    });
                }
            }
            Ok(())
        }
        other => {
            // Unknown kinds are not errors: newer domains must not break
            // older edges. The log line is the loud part.
            eprintln!("canvas-edge: ignoring unrouted kind '{other}'");
            Ok(())
        }
    }
}

struct MirrorSocketGuard(std::path::PathBuf);

#[derive(Default)]
struct MirrorRuntime {
    local_document: Option<canvas::WorkspaceDocument>,
    suppress_snapshot: Option<Vec<u8>>,
}

impl Drop for MirrorSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn start_workspace_mirror(
    config: &EdgeConfig,
    bus: Arc<Bus>,
    runtime: Arc<Mutex<MirrorRuntime>>,
) -> Result<Option<MirrorSocketGuard>> {
    let Some(mirror) = config.mirror.as_ref().filter(|mirror| mirror.enabled) else {
        return Ok(None);
    };
    let path = canvas_core::workspace_mirror_socket_path()
        .context("Canvas workspace mirror socket is unavailable")?;
    let parent = path.parent().context("mirror socket has no parent")?;
    fs::create_dir_all(parent).context("create mirror socket directory")?;
    if path.exists() {
        fs::remove_file(&path).context("remove stale mirror socket")?;
    }
    let listener = UnixListener::bind(&path).context("bind workspace mirror socket")?;
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .context("secure workspace mirror socket")?;
    let zone = mirror
        .zone
        .clone()
        .or_else(|| config.bus.zones.first().cloned())
        .context("mirror requires a destination zone")?;
    let origin = config.bus.producer_id.clone();
    let sequence = Arc::new(AtomicU64::new(0));
    thread::Builder::new()
        .name("canvas-workspace-mirror".into())
        .spawn(move || {
            for accepted in listener.incoming() {
                let stream = match accepted {
                    Ok(stream) => stream,
                    Err(error) => {
                        eprintln!("canvas-edge: mirror socket accept failed: {error}");
                        continue;
                    }
                };
                let mut reader = BufReader::new(stream);
                let mut frame = Vec::new();
                loop {
                    frame.clear();
                    match reader
                        .by_ref()
                        .take(canvas::control::MAX_CONTROL_MESSAGE + 1)
                        .read_until(b'\n', &mut frame)
                    {
                        Ok(0) => break,
                        Ok(_)
                            if frame.len() as u64 <= canvas::control::MAX_CONTROL_MESSAGE
                                && frame.last() == Some(&b'\n') => {}
                        Ok(_) => {
                            eprintln!("canvas-edge: rejected oversized mirror snapshot");
                            break;
                        }
                        Err(error) => {
                            eprintln!("canvas-edge: mirror socket receive failed: {error}");
                            break;
                        }
                    }
                    frame.pop();
                    let local: LocalWorkspaceSnapshot = match serde_json::from_slice(&frame) {
                        Ok(local) => local,
                        Err(error) => {
                            eprintln!("canvas-edge: rejected local mirror snapshot: {error}");
                            continue;
                        }
                    };
                    if local.schema_version != MIRROR_SCHEMA_VERSION {
                        eprintln!("canvas-edge: rejected unsupported local mirror schema");
                        continue;
                    }
                    let local_bytes = frame.clone();
                    let suppressed = runtime
                        .lock()
                        .ok()
                        .and_then(|mut state| {
                            state.local_document = Some(local.document.clone());
                            if state.suppress_snapshot.as_ref() == Some(&local_bytes) {
                                state.suppress_snapshot.take()
                            } else {
                                None
                            }
                        })
                        .is_some();
                    if suppressed {
                        continue;
                    }
                    let state = WorkspaceState {
                        schema_version: MIRROR_SCHEMA_VERSION,
                        workspace_key: local.document.id.clone(),
                        origin: origin.clone(),
                        sequence: sequence.fetch_add(1, Ordering::Relaxed) + 1,
                        document: local.document,
                    };
                    let message = canvas_core::unibus::Message::new(
                        MIRROR_KIND,
                        serde_json::to_value(state).expect("mirror state serializes"),
                    );
                    let result = bus
                        .to_destination(&zone, message)
                        .and_then(|envelope| bus.send(&envelope, &[]));
                    if let Err(error) = result {
                        eprintln!("canvas-edge: publish workspace mirror failed: {error:#}");
                    }
                }
            }
        })
        .context("start workspace mirror publisher")?;
    // Publish the current representation immediately, even when Canvas was
    // already running before its edge connected.
    if let Ok(document) = read_document() {
        let snapshot = LocalWorkspaceSnapshot {
            schema_version: MIRROR_SCHEMA_VERSION,
            document,
        };
        if let Ok(mut stream) = UnixStream::connect(&path) {
            if let Ok(mut bytes) = serde_json::to_vec(&snapshot) {
                bytes.push(b'\n');
                let _ = stream.write_all(&bytes);
            }
        }
    }
    Ok(Some(MirrorSocketGuard(path)))
}

fn apply_workspace_mirror(
    config: &EdgeConfig,
    liveness: &mut Liveness,
    runtime: &Arc<Mutex<MirrorRuntime>>,
    envelope: &Envelope,
) -> Result<()> {
    if !config.mirror.as_ref().is_some_and(|mirror| mirror.enabled) {
        return Ok(());
    }
    let state: WorkspaceState =
        serde_json::from_value(serde_json::Value::Object(envelope.message.body.clone()))
            .context("decode workspace mirror state")?;
    ensure!(
        state.schema_version == MIRROR_SCHEMA_VERSION,
        "unsupported mirror schema"
    );
    if state.origin == config.bus.producer_id {
        return Ok(());
    }
    let previous = liveness
        .mirror_sequences
        .entry(state.origin.clone())
        .or_default();
    if state.sequence <= *previous {
        return Ok(());
    }
    *previous = state.sequence;

    let current = runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("mirror runtime lock poisoned"))?
        .local_document
        .clone()
        .map(Ok)
        .unwrap_or_else(read_document)?;
    // "Mirror" is intentionally opt-in by representation: two canvases only
    // converge while they are displaying the same named workspace.
    if !should_apply_workspace_state(&current, &state) {
        return Ok(());
    }
    // Snapshot QoS may legitimately redeliver the latest value after a peer
    // reconnects. Applying an identical document would restart transitions,
    // wake render work, and echo through the local publisher for no visual
    // change. Equality is the idempotency boundary for complete snapshots.
    let local = LocalWorkspaceSnapshot {
        schema_version: MIRROR_SCHEMA_VERSION,
        document: state.document.clone(),
    };
    {
        let mut runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("mirror runtime lock poisoned"))?;
        runtime.suppress_snapshot = Some(serde_json::to_vec(&local)?);
        runtime.local_document = Some(state.document.clone());
    }
    submit(ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: format!("mirror-{}-{}", state.origin, state.sequence),
        expected_revision: None,
        actor: None,
        command: Command::ApplyDocument {
            document: state.document,
        },
    })
}

fn should_apply_workspace_state(
    current: &canvas::WorkspaceDocument,
    state: &WorkspaceState,
) -> bool {
    current.id == state.workspace_key && current != &state.document
}

fn read_document() -> Result<canvas::WorkspaceDocument> {
    let request = ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: format!("mirror-inspect-{}", canvas_core::unibus::envelope::now_ms()),
        expected_revision: None,
        actor: None,
        command: Command::ReadDocument,
    };
    let reply = canvas::control::send(&request).map_err(anyhow::Error::msg)?;
    match reply.outcome {
        Outcome::Ok {
            receipt: canvas::control::Receipt::DocumentSnapshot { document, .. },
        } => Ok(*document),
        Outcome::Ok { .. } => bail!("Canvas returned the wrong receipt for ReadDocument"),
        Outcome::Error { code, message } => {
            bail!("Canvas rejected ReadDocument ({code:?}): {message}")
        }
    }
}

/// Turn the transport-level RLCD event into Canvas's existing bounded widget
/// snapshot contract. The edge owns the short presentation history; the
/// producer remains unaware of Canvas layout and rendering details.
fn publish_rlcd_decision(liveness: &mut Liveness, envelope: &Envelope) -> Result<()> {
    let path = canvas_core::stream_socket_path().context("Canvas stream socket is unavailable")?;
    let socket = UnixDatagram::unbound().context("open Canvas stream datagram")?;
    let log = rlcd_decision_event(liveness, envelope)?;
    let mut events = vec![log];
    if let Some(probabilities) = rlcd_probability_event(envelope)? {
        events.push(probabilities);
    }
    for event in events {
        let bytes = serde_json::to_vec(&event).context("encode Canvas RLCD event")?;
        ensure!(
            bytes.len() <= MAX_LOCAL_STREAM_DATAGRAM,
            "Canvas RLCD event is {} bytes (local socket limit {})",
            bytes.len(),
            MAX_LOCAL_STREAM_DATAGRAM
        );
        socket
            .send_to(&bytes, &path)
            .with_context(|| format!("send RLCD event to {}", path.display()))?;
    }
    Ok(())
}

fn rlcd_probability_event(envelope: &Envelope) -> Result<Option<StreamEnvelope>> {
    let body = &envelope.message.body;
    if body["phase"].as_str() != Some("completed") {
        return Ok(None);
    }
    let session = body["session_id"]
        .as_str()
        .context("RLCD event lacks session_id")?;
    let sequence = body["sequence"]
        .as_u64()
        .context("RLCD event lacks sequence")?;
    let selected = body["selected"].as_str();
    let values = body
        .get("response")
        .and_then(|value| value.pointer("/answers/action/probabilities"))
        .and_then(serde_json::Value::as_object)
        .context("completed RLCD event lacks action probabilities")?;
    let mut bars: Vec<_> = values
        .iter()
        .filter_map(|(label, value)| {
            value.as_f64().map(|probability| {
                serde_json::json!({
                    "label": label.replace('_', " "),
                    "value": probability * 100.0,
                    "state": if selected == Some(label.as_str()) { "good" } else { "neutral" }
                })
            })
        })
        .collect();
    bars.sort_by(|left, right| {
        right["value"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&left["value"].as_f64().unwrap_or_default())
    });
    ensure!(
        !bars.is_empty(),
        "completed RLCD event has no probabilities"
    );
    Ok(Some(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: format!("{session}-probabilities"),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence,
        emitted_at_ms: body["at_unix_ms"]
            .as_u64()
            .unwrap_or(envelope.issued_at_unix_ms),
        project_id: Some("speedy".into()),
        payload: serde_json::json!({
            "widget": {
                "kind": "ranked_bar",
                "title": format!("Decision #{sequence} · {}", selected.unwrap_or("unknown").replace('_', " ")),
                "bars": bars,
                "unit": "%"
            },
            "data": {
                "state": "ready",
                "source": format!("unibus:{}:rlcd.decision.v1", envelope.producer_id),
                "window": "live"
            }
        }),
    }))
}

fn rlcd_decision_event(liveness: &mut Liveness, envelope: &Envelope) -> Result<StreamEnvelope> {
    let body = &envelope.message.body;
    ensure!(
        body["schema_version"].as_u64() == Some(1),
        "unsupported RLCD schema"
    );
    let session = body["session_id"]
        .as_str()
        .context("RLCD event lacks session_id")?;
    ensure!(
        !session.is_empty()
            && session.len() <= 64
            && session
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "invalid RLCD session_id"
    );
    let sequence = body["sequence"]
        .as_u64()
        .context("RLCD event lacks sequence")?;
    let emitted_at_ms = body["at_unix_ms"]
        .as_u64()
        .unwrap_or(envelope.issued_at_unix_ms);
    let phase = body["phase"].as_str().context("RLCD event lacks phase")?;
    ensure!(
        matches!(phase, "requested" | "completed" | "failed"),
        "invalid RLCD phase"
    );
    let message = match phase {
        "requested" => format!("#{sequence} · thinking…"),
        "completed" => format!(
            "#{sequence} · {} · {} ms · confidence {}",
            body["selected"].as_str().unwrap_or("unknown"),
            body["latency_ms"].as_u64().unwrap_or(0),
            body["confidence"]
                .as_f64()
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "—".into())
        ),
        "failed" => format!(
            "#{sequence} · failed · {}",
            body["error"].as_str().unwrap_or("unknown error")
        ),
        _ => unreachable!(),
    };
    let entries = liveness
        .rlcd_by_session
        .entry(session.to_owned())
        .or_default();
    if phase != "requested" {
        entries.retain(|entry| entry["call_id"] != body["call_id"]);
    }
    entries.push_front(serde_json::json!({
        "call_id": body["call_id"],
        "timestamp": emitted_at_ms.to_string(),
        "message": message,
        "level": match phase { "completed" => "good", "failed" => "warning", _ => "neutral" }
    }));
    // Eight decisions keep the widget useful and the complete envelope under
    // macOS's 2 KiB Unix-datagram ceiling. Longer history belongs upstream,
    // not in every latest-value UI snapshot.
    entries.truncate(8);
    let visible: Vec<_> = entries
        .iter()
        .cloned()
        .map(|mut entry| {
            entry.as_object_mut().map(|object| object.remove("call_id"));
            entry
        })
        .collect();
    Ok(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: session.to_owned(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence,
        emitted_at_ms,
        project_id: Some("speedy".into()),
        payload: serde_json::json!({
            "widget": {
                "kind": "event_log",
                "title": format!("RLCD · {}", body["game"].as_str().unwrap_or(session)),
                "entries": visible
            },
            "data": {
                "state": if phase == "failed" { "error" } else { "ready" },
                "source": format!("unibus:{}:rlcd.decision.v1", envelope.producer_id),
                "window": "live"
            }
        }),
    })
}

/// Adapt the transport-neutral audio observation into the local Canvas stream
/// contract. The audio edge owns measurement; Canvas owns presentation.
fn publish_audio_level(envelope: &Envelope) -> Result<()> {
    let event = audio_level_event(envelope)?;
    let bytes = serde_json::to_vec(&event).context("encode Canvas audio-level event")?;
    let path = canvas_core::stream_socket_path().context("Canvas stream socket is unavailable")?;
    let socket = UnixDatagram::unbound().context("open Canvas stream datagram")?;
    socket
        .send_to(&bytes, &path)
        .with_context(|| format!("send audio level to {}", path.display()))?;
    Ok(())
}

fn audio_level_event(envelope: &Envelope) -> Result<StreamEnvelope> {
    let peak = envelope
        .message
        .body
        .get("peak")
        .and_then(|value| value.as_f64())
        .context("audio level is missing peak")?
        .clamp(0.0, 1.0);
    let observed_at_ms = envelope
        .message
        .body
        .get("observed_at_unix_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(envelope.issued_at_unix_ms);
    Ok(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: "playback-level".into(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence: observed_at_ms,
        emitted_at_ms: observed_at_ms,
        project_id: None,
        payload: serde_json::json!({
            "widget": {"kind": "meter", "value": peak, "style": "vu"},
            "data": {
                "state": "ready",
                "source": format!("unibus:{}:audio.level", envelope.producer_id),
                "window": "live"
            }
        }),
    })
}

/// Route transport-neutral tracked positions into the existing native widget
/// stream. The producer owns sensing; Canvas owns presentation and never polls
/// a vendor endpoint directly.
fn publish_spatial_tracks(envelope: &Envelope) -> Result<()> {
    let event = spatial_tracks_event(envelope)?;
    let bytes = serde_json::to_vec(&event).context("encode Canvas spatial-tracks event")?;
    let path = canvas_core::stream_socket_path().context("Canvas stream socket is unavailable")?;
    let socket = UnixDatagram::unbound().context("open Canvas stream datagram")?;
    socket
        .send_to(&bytes, &path)
        .with_context(|| format!("send spatial tracks to {}", path.display()))?;
    Ok(())
}

fn spatial_tracks_event(envelope: &Envelope) -> Result<StreamEnvelope> {
    let body = &envelope.message.body;
    let observed_at_ms = body
        .get("observed_at_unix_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(envelope.issued_at_unix_ms);
    let valid_until_ms = body
        .get("valid_until_unix_ms")
        .and_then(|value| value.as_u64())
        .context("spatial tracks are missing valid_until_unix_ms")?;
    ensure!(
        valid_until_ms > observed_at_ms,
        "spatial tracks are already expired"
    );
    let tracks = body
        .get("tracks")
        .cloned()
        .context("spatial tracks are missing tracks")?;
    let bounds = body
        .get("bounds")
        .cloned()
        .context("spatial tracks are missing bounds")?;
    let title = body
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Spatial tracks");
    let event = StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: "spatial-tracks".into(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence: observed_at_ms,
        emitted_at_ms: observed_at_ms,
        project_id: None,
        payload: serde_json::json!({
            "widget": {
                "kind": "spatial_tracks",
                "title": title,
                "tracks": tracks,
                "bounds": bounds
            },
            "data": {
                "state": "ready",
                "source": format!("unibus:{}:spatial.tracks", envelope.producer_id),
                "window": "live",
                "valid_until_unix_ms": valid_until_ms
            }
        }),
    };
    event.validate().map_err(anyhow::Error::msg)?;
    Ok(event)
}

/// Producer allowlist + per-command grant, then actor stamping.
fn authorize(
    config: &EdgeConfig,
    envelope: &Envelope,
    raw: &serde_json::Value,
) -> Result<ControlRequest, String> {
    let actor = config
        .actors
        .iter()
        .find(|a| a.producer == envelope.producer_id)
        .ok_or_else(|| {
            format!(
                "producer '{}' is not an allowed command actor on this screen",
                envelope.producer_id
            )
        })?;
    let variant = raw
        .pointer("/command/command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "command is missing its tagged 'command' name".to_owned())?;
    if !actor.commands.iter().any(|c| c == variant) {
        return Err(format!(
            "actor '{}' is not granted command '{variant}'",
            actor.id
        ));
    }
    let mut value = raw.clone();
    value["actor"] = serde_json::json!({
        "id": actor.id,
        "timestamp_ms": envelope::now_ms(),
    });
    let request: ControlRequest =
        serde_json::from_value(value).map_err(|e| format!("invalid control request: {e}"))?;
    Ok(request)
}

fn submit(request: ControlRequest) -> Result<()> {
    request.validate().map_err(anyhow::Error::msg)?;
    let reply = canvas::control::send(&request).map_err(anyhow::Error::msg)?;
    // The hub ack proves delivery; the receipt must carry what Canvas
    // actually answered, including application-level failures.
    match reply.outcome {
        canvas::control::Outcome::Ok { .. } => Ok(()),
        canvas::control::Outcome::Error { code, message } => {
            bail!("canvas rejected the command ({code:?}): {message}")
        }
    }
}

fn receipt(
    bus: &Bus,
    command_envelope: &Envelope,
    request_id: &str,
    ok: bool,
    error: Option<String>,
) -> Result<()> {
    let message = result_message(request_id, ok, error);
    let envelope = bus
        .to_destination(&command_envelope.producer_id, message)
        .context("build receipt envelope")?;
    bus.send(&envelope, &[]).context("send receipt")
}

fn presence_snapshot(summary: &serde_json::Value) -> Result<PresenceSnapshot> {
    let zone = text(summary, "zone")?;
    let occupied = summary
        .get("occupied")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let recency_ms = summary
        .get("recency_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let occupants = summary
        .get("occupants")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(presence_occupant)
        .collect();
    Ok(PresenceSnapshot {
        zone: zone.chars().take(64).collect(),
        observed_at_unix_ms: envelope::now_ms().saturating_sub(recency_ms),
        expires_after_ms: (recency_ms + 10_000).clamp(250, 300_000),
        occupied,
        occupants,
    })
}

fn presence_occupant(value: &serde_json::Value) -> PresenceOccupant {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect();
    let display_name = value
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .chars()
        .take(64)
        .collect();
    PresenceOccupant {
        id,
        display_name,
        presence: PresenceSignal {
            state: PresenceSignalState::Detected,
            confidence: value
                .get("face")
                .and_then(|f| f.get("confidence"))
                .and_then(|c| c.as_f64())
                .filter(|c| (0.0..=1.0).contains(c))
                .map(|c| c as f32),
            age_ms: 0,
        },
        face: value
            .get("face")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
        voice: value
            .get("voice")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
    }
}

fn map_signal(signal: &serde_json::Value) -> PresenceSignal {
    let state = match signal
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
    {
        "known" | "verified" | "matched" => PresenceSignalState::Verified,
        "detected" => PresenceSignalState::Detected,
        "unavailable" => PresenceSignalState::Unavailable,
        _ => PresenceSignalState::Unknown,
    };
    PresenceSignal {
        state,
        confidence: signal
            .get("confidence")
            .and_then(|v| v.as_f64())
            .filter(|c| (0.0..=1.0).contains(c))
            .map(|c| c as f32),
        age_ms: signal.get("age_ms").and_then(|v| v.as_u64()).unwrap_or(0),
    }
}

fn text(value: &serde_json::Value, field: &str) -> Result<String> {
    Ok(value
        .get(field)
        .and_then(|v| v.as_str())
        .with_context(|| format!("missing string field '{field}'"))?
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::unibus::{Message, parse_result};
    use std::path::PathBuf;

    fn config_with(actors: Vec<BusActor>) -> EdgeConfig {
        EdgeConfig {
            bus: BusConfig {
                installation_id: "home".into(),
                producer_id: "canvas-neo".into(),
                hub_endpoint: "127.0.0.1:1".into(),
                token_file: PathBuf::from("t"),
                zones: vec!["default".into()],
                capabilities: vec!["display".into()],
                screen_anchors: vec!["screen-laptop".into()],
                schema_version: 1,
            },
            actors,
            mirror: None,
        }
    }

    #[test]
    fn mirror_only_applies_changed_state_for_the_active_workspace() {
        let document: canvas::WorkspaceDocument =
            serde_json::from_str(include_str!("../../../../config/workspaces/tv/speedy.json"))
                .unwrap();
        let mut state = WorkspaceState {
            schema_version: MIRROR_SCHEMA_VERSION,
            workspace_key: document.id.clone(),
            origin: "other-screen".into(),
            sequence: 1,
            document: document.clone(),
        };
        assert!(!should_apply_workspace_state(&document, &state));
        state.document.revision += 1;
        assert!(should_apply_workspace_state(&document, &state));
        state.workspace_key = "another-workspace".into();
        assert!(!should_apply_workspace_state(&document, &state));
    }

    fn command_envelope_from(producer: &str, inner: serde_json::Value) -> Envelope {
        Envelope::new(
            "home",
            producer,
            None,
            "",
            Message::new("canvas_command", serde_json::json!({ "command": inner })),
        )
    }

    fn indicator_request(request_id: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": canvas::control::CONTROL_VERSION,
            "request_id": request_id,
            "command": {
                "command": "upsert_indicator",
                "indicator": {
                    "id": "assistant-status", "title": "Assistant", "state": "listening",
                },
            },
        })
    }

    #[test]
    fn unknown_producer_is_denied() {
        let config = config_with(vec![]);
        let envelope = command_envelope_from("ghost", indicator_request("r1"));
        let err = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap_err();
        assert!(err.contains("not an allowed command actor"));
    }

    #[test]
    fn granted_command_gets_the_actors_identity_stamped() {
        let config = config_with(vec![BusActor {
            producer: "canvas-central-studio".into(),
            id: "opencode:central".into(),
            commands: vec!["upsert_indicator".into()],
        }]);
        let envelope = command_envelope_from("canvas-central-studio", indicator_request("r2"));
        let request = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap();
        assert_eq!(request.request_id, "r2");
        let actor = request.actor.expect("actor stamped");
        assert_eq!(actor.id, "opencode:central");
        assert!(actor.timestamp_ms > 0);
    }

    #[test]
    fn ungranted_command_variant_is_denied() {
        let config = config_with(vec![BusActor {
            producer: "canvas-central-studio".into(),
            id: "opencode:central".into(),
            commands: vec!["set_presence".into()],
        }]);
        let envelope = command_envelope_from("canvas-central-studio", indicator_request("r3"));
        let err = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap_err();
        assert!(err.contains("not granted command 'upsert_indicator'"));
    }

    #[test]
    fn receipts_parse_back_to_correlated_results() {
        let message = result_message("r4", false, Some("denied".into()));
        let parsed = parse_result(&message).expect("parses");
        assert_eq!(parsed.request_id, "r4");
        assert!(!parsed.ok);
        assert_eq!(parsed.error.as_deref(), Some("denied"));
    }

    #[test]
    fn liveness_tracks_and_drains_per_producer() {
        let mut l = Liveness::default();
        l.record("sound-neo", "jarvis-voice");
        l.record("sound-neo", "other");
        l.forget("sound-neo", "other");
        assert_eq!(l.take_for_dismissal("sound-neo"), vec!["jarvis-voice"]);
        assert!(l.take_for_dismissal("sound-neo").is_empty());
    }

    #[test]
    fn presence_mapping_still_validates() {
        let summary = serde_json::json!({
            "zone": "default", "occupied": true, "recency_ms": 80,
            "occupants": [{"id": "avery", "display_name": "Avery",
                "face": {"state": "known", "confidence": 0.9, "age_ms": 80},
                "voice": {"state": "unavailable", "confidence": null, "age_ms": 0}}],
            "tact": "Direct",
        });
        presence_snapshot(&summary).unwrap().validate().unwrap();
    }

    #[test]
    fn audio_level_maps_to_the_existing_meter_binding_contract() {
        let envelope = Envelope::new(
            "home",
            "home-pi-audio",
            None,
            "",
            Message::new(
                "audio_level",
                serde_json::json!({
                    "rms": 0.25,
                    "peak": 0.75,
                    "observed_at_unix_ms": 42,
                }),
            ),
        );
        let event = audio_level_event(&envelope).unwrap();
        event.validate().unwrap();
        assert_eq!(event.owner, "home-pi-audio");
        assert_eq!(event.stream, "playback-level");
        assert_eq!(event.sequence, 42);
        assert_eq!(event.payload["widget"]["kind"], "meter");
        assert_eq!(event.payload["widget"]["value"], 0.75);
    }

    #[test]
    fn rlcd_call_animates_from_requested_to_completed_without_duplicate_rows() {
        let mut liveness = Liveness::default();
        let event = |phase: &str, selected: Option<&str>| {
            Envelope::new(
                "home",
                "mac-studio-speedy",
                None,
                "",
                Message::new(
                    "rlcd.decision.v1",
                    serde_json::json!({
                        "schema_version": 1,
                        "session_id": "retro-smk",
                        "call_id": "retro-smk-7",
                        "sequence": 7,
                        "at_unix_ms": 1234,
                        "phase": phase,
                        "game": "Super Mario Kart (SNES)",
                        "request": {"state": {"observation": {"speed": 783}}},
                        "response": selected.map(|_| serde_json::json!({"answers": {"action": {
                            "choice": "left",
                            "probabilities": {"left": 0.81, "right": 0.12, "brake": 0.07}
                        }}})),
                        "selected": selected,
                        "confidence": selected.map(|_| 0.81),
                        "latency_ms": selected.map(|_| 44),
                        "error": null
                    }),
                ),
            )
        };
        let requested = rlcd_decision_event(&mut liveness, &event("requested", None)).unwrap();
        requested.validate().unwrap();
        assert_eq!(requested.project_id.as_deref(), Some("speedy"));
        assert_eq!(
            requested.payload["widget"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            requested.payload["widget"]["entries"][0]["message"]
                .as_str()
                .unwrap()
                .contains("thinking")
        );

        let completed =
            rlcd_decision_event(&mut liveness, &event("completed", Some("left"))).unwrap();
        assert!(serde_json::to_vec(&completed).unwrap().len() <= MAX_LOCAL_STREAM_DATAGRAM);
        assert_eq!(
            completed.payload["widget"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let message = completed.payload["widget"]["entries"][0]["message"]
            .as_str()
            .unwrap();
        assert!(message.contains("left"));
        assert!(message.contains("44 ms"));

        let probabilities = rlcd_probability_event(&event("completed", Some("left")))
            .unwrap()
            .unwrap();
        probabilities.validate().unwrap();
        assert_eq!(probabilities.stream, "retro-smk-probabilities");
        assert_eq!(probabilities.payload["widget"]["kind"], "ranked_bar");
        assert_eq!(probabilities.payload["widget"]["bars"][0]["label"], "left");
        assert_eq!(probabilities.payload["widget"]["bars"][0]["value"], 81.0);
        assert_eq!(probabilities.payload["widget"]["bars"][0]["state"], "good");
    }
}
