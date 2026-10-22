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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    LocalWorkspaceSnapshot, MIRROR_KIND, MIRROR_SCHEMA_VERSION, WorkspaceState, same_representation,
};
use canvas_core::unibus::{Envelope, envelope, kinds, result_message_with_data};
use serde::Deserialize;

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(15);
/// macOS's default `net.local.dgram.maxdgram` is 2048 bytes. Leave framing
/// headroom so every local stream snapshot remains portable across hosts.
const MAX_LOCAL_STREAM_DATAGRAM: usize = 1_900;

/// Retries before a snapshot is dropped when Canvas has not drained the socket.
///
/// Small on purpose: the next decision is already on its way, so a snapshot
/// worth waiting long for is a snapshot already out of date.
const STREAM_SEND_ATTEMPTS: u32 = 4;

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
    /// Stable identity of the Canvas output this edge fronts. Defaults to the
    /// first screen anchor for backward-compatible deployments.
    #[serde(default)]
    output_id: Option<String>,
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
    let camera_seen: CameraSeen = Arc::new(Mutex::new(BTreeMap::new()));
    start_camera_bridge(config, Arc::clone(&bus), Arc::clone(&camera_seen))?;
    start_prompt_bridge(config, Arc::clone(&bus))?;
    loop {
        match bus.try_read() {
            Ok(Some((envelope, _payload))) => {
                if let Err(e) = ingest(
                    &bus,
                    config,
                    &mut liveness,
                    &mirror_runtime,
                    &camera_seen,
                    &envelope,
                ) {
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
    fan_by_session: HashMap<String, VecDeque<FanEntry>>,
}

/// A posterior as whole percent, for a label the eye can rank at a glance.
///
/// Clamped because a producer that sends a score outside 0..=1 should show a
/// wrong-looking number rather than wrap around to a plausible one.
fn percent(score: f64) -> u8 {
    (score.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// One decision, reduced to what the fan draws.
///
/// The model scores *every* action it was offered, not just the one it took,
/// and that posterior is already on the wire. Keeping it here is what lets the
/// paths nobody took fade by their real probability rather than by decoration.
#[derive(Clone)]
struct FanEntry {
    sequence: u64,
    /// `None` while the call is still in flight, which is what draws a
    /// thinking node rather than a gap.
    selected: Option<String>,
    /// Posterior of the action actually taken.
    ///
    /// Held per decision so a trunk node is lit by how sure that decision was.
    /// `heat` then means one thing everywhere in the graph — strength of belief
    /// — rather than recency on the trunk and probability on the branches,
    /// which cannot be read as a single picture.
    confidence: Option<f64>,
    /// Offered actions with their posterior, highest first.
    alternatives: Vec<(String, f64)>,
    failed: bool,
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
    camera_seen: &CameraSeen,
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
            .map(|_| ())
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
                    Ok(data) => (true, None, data),
                    Err(e) => (false, Some(format!("{e:#}")), None),
                },
                Err(reason) => {
                    eprintln!("canvas-edge: denied: {reason}");
                    (false, Some(reason), None)
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
            receipt(bus, envelope, &request_id, outcome.0, outcome.1, outcome.2)
        }
        CAMERA_KIND => apply_camera_pose(camera_seen, envelope),
        "audio_level" | "audio.level" => publish_audio_level(envelope),
        "market.order_book.v1" | "market.trade.v1" | "market.ohlcv.raw.v1" => {
            publish_market_data(envelope)
        }
        "rlcd.decision.v1" => publish_rlcd_decision(liveness, envelope),
        "spatial_tracks" | "spatial.tracks" => publish_spatial_tracks(envelope),
        MIRROR_KIND => apply_workspace_mirror(config, liveness, mirror_runtime, envelope),
        kinds::V1_REPLY | kinds::REPLY => Ok(()),
        canvas_core::unibus::kinds::NODE_STATE => {
            let online = envelope
                .message
                .body
                .get("online")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if !online {
                let producer = envelope
                    .message
                    .body
                    .get("producer_id")
                    .map_or(serde_json::Value::Null, Clone::clone)
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

/// Bridge normalized market data onto Canvas's local stream contract.
///
/// FIX parsing and book state deliberately remain at the market edge. This
/// process only translates the Unibus envelope, so a WebSocket crypto source
/// can publish the exact same contracts without another Canvas integration.
fn publish_market_data(envelope: &Envelope) -> Result<()> {
    let symbol = envelope
        .message
        .body
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .context("market event missing symbol")?;
    let stream = symbol
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    ensure!(!stream.is_empty(), "market event has empty symbol");
    let event = StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: "market".into(),
        stream,
        contract: envelope.message.kind.clone(),
        sequence: envelope
            .message
            .body
            .get("sequence")
            .and_then(serde_json::Value::as_u64)
            .context("market event missing sequence")?,
        emitted_at_ms: envelope
            .message
            .body
            .get("emitted_at_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(envelope.issued_at_unix_ms),
        project_id: None,
        payload: serde_json::Value::Object(envelope.message.body.clone()),
    };
    event.validate().map_err(anyhow::Error::msg)?;
    let bytes = serde_json::to_vec(&event).context("encode Canvas market event")?;
    ensure!(
        bytes.len() <= MAX_LOCAL_STREAM_DATAGRAM,
        "market snapshot is {} bytes; local Canvas limit is {}",
        bytes.len(),
        MAX_LOCAL_STREAM_DATAGRAM
    );
    let path = canvas_core::stream_socket_path().context("Canvas stream socket is unavailable")?;
    UnixDatagram::unbound()
        .context("open Canvas market stream datagram")?
        .send_to(&bytes, path)
        .context("send Canvas market event")?;
    Ok(())
}

struct MirrorSocketGuard {
    path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct MirrorRuntime {
    local_document: Option<canvas::WorkspaceDocument>,
    /// The board we just handed to the local canvas, held so its echo can be
    /// recognised. Stored as a document rather than as bytes because applying
    /// it bumps `revision`, so the snapshot that comes back never matches the
    /// bytes that went out.
    suppress_snapshot: Option<canvas::WorkspaceDocument>,
}

impl Drop for MirrorSocketGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = fs::remove_file(&self.path);
    }
}

/// Contract a prompt typed on the wall is published under.
const PROMPT_KIND: &str = "canvas.prompt.v1";

/// How often the spool is drained.
///
/// The main loop blocks on a hub read with a timeout measured in seconds, so
/// draining there would leave a prompt sitting for up to a minute. Prompts are
/// interactive; this runs on its own thread.
const PROMPT_POLL: Duration = Duration::from_millis(200);

/// A prompt is one typed line. Anything larger did not come from the wall.
const MAX_PROMPT_SPOOL_BYTES: u64 = 16 * 1024;

/// Publish prompts Canvas has spooled, and remove each once it is on the bus.
///
/// Canvas has no bus client: it writes one file per submission under the
/// runtime root and this owns the crossing, which is the same split that keeps
/// the wall from becoming a client of every tool that might answer.
///
/// Addressed to the zone rather than to the target's producer, because Canvas
/// names a target ("transmog") and not an address. Interested services filter
/// on `target`; resolving it here would put a routing table in the display
/// path and break whenever a tool moved.
/// Camera poses are forwarded, not spooled.
///
/// One file per stream holding the newest pose, republished when it changes.
/// A queue would be wrong here in a way that is worth being explicit about:
/// every pose supersedes the one before it, so a backlog can only ever deliver
/// a camera movement that already finished.
const CAMERA_KIND: &str = "canvas.model.camera.v1";

/// Fast enough to keep up with a 30-a-second drag without the poll itself
/// costing anything: the check is one directory read and a string compare.
const CAMERA_POLL: Duration = Duration::from_millis(40);

const MAX_CAMERA_BYTES: u64 = 4 * 1024;

/// The last pose seen for each stream, whether this host published it or
/// received it.
///
/// Shared between the forwarder and the receiver, and this is what stops the
/// echo: without it, a host that receives a pose writes it to the same file
/// the forwarder is watching, sees a change, and publishes it straight back.
/// Two screens showing one model would then trade the same pose forever.
type CameraSeen = Arc<Mutex<BTreeMap<String, String>>>;

fn start_camera_bridge(config: &EdgeConfig, bus: Arc<Bus>, seen: CameraSeen) -> Result<()> {
    let Some(directory) = canvas_core::video_frame_file("probe")
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    else {
        eprintln!("canvas-edge: no runtime frames directory; camera poses will not be forwarded");
        return Ok(());
    };
    fs::create_dir_all(&directory).context("create video frame directory")?;
    eprintln!(
        "canvas-edge: forwarding model camera poses from {}",
        directory.display()
    );
    let zone = config
        .bus
        .zones
        .first()
        .cloned()
        .context("forwarding camera poses requires a zone")?;
    thread::Builder::new()
        .name("canvas-camera-bridge".into())
        .spawn(move || {
            loop {
                forward_cameras(&directory, &bus, &zone, &seen);
                thread::sleep(CAMERA_POLL);
            }
        })
        .context("spawn camera bridge")?;
    Ok(())
}

/// Every `.camera` file whose pose has changed since it was last seen.
fn changed_cameras(directory: &std::path::Path, seen: &CameraSeen) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "camera")
        {
            continue;
        }
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_CAMERA_BYTES)
        {
            continue;
        }
        let Some(stream) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        // Parsed before forwarding so a half-written or malformed pose never
        // reaches the bus, and so an unchanged pose written twice is still
        // recognised as unchanged.
        let Ok(pose) = serde_json::from_str::<canvas_core::model_camera::ViewerCamera>(&body)
        else {
            continue;
        };
        let Ok(canonical) = serde_json::to_string(&pose) else {
            continue;
        };
        let mut seen = match seen.lock() {
            Ok(seen) => seen,
            Err(poisoned) => poisoned.into_inner(),
        };
        if seen.get(stream) == Some(&canonical) {
            continue;
        }
        seen.insert(stream.to_string(), canonical.clone());
        changed.push((stream.to_string(), canonical));
    }
    changed
}

fn forward_cameras(directory: &std::path::Path, bus: &Bus, zone: &str, seen: &CameraSeen) {
    for (stream, body) in changed_cameras(directory, seen) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let message = canvas_core::unibus::Message::new(CAMERA_KIND, value);
        if let Err(error) = bus
            .to_destination(zone, message)
            .and_then(|envelope| bus.send(&envelope, &[]))
        {
            // Not retried, and deliberately so: another pose is moments away,
            // and re-sending this one later would move the camera backwards.
            eprintln!("canvas-edge: camera pose for {stream} not forwarded: {error:#}");
        }
    }
}

/// A pose from another screen, landed where this host's renderer reads it.
fn apply_camera_pose(seen: &CameraSeen, envelope: &Envelope) -> Result<()> {
    let pose: canvas_core::model_camera::ViewerCamera =
        serde_json::from_value(serde_json::to_value(&envelope.message.body)?)
            .context("camera pose does not match the contract")?;
    let path = canvas_core::model_camera::camera_file(&pose.stream_id)
        .context("camera pose names a stream that cannot address a file")?;
    let canonical = serde_json::to_string(&pose).context("re-serialise camera pose")?;
    // Recorded before the write, so the forwarder sees its own arrival as
    // already-seen rather than as a local change to publish back.
    {
        let mut seen = match seen.lock() {
            Ok(seen) => seen,
            Err(poisoned) => poisoned.into_inner(),
        };
        seen.insert(pose.stream_id.clone(), canonical.clone());
    }
    canvas_core::write_atomic(&path, canonical.as_bytes()).context("write camera pose")?;
    Ok(())
}

fn start_prompt_bridge(config: &EdgeConfig, bus: Arc<Bus>) -> Result<()> {
    let Some(root) = canvas_core::runtime_root() else {
        eprintln!("canvas-edge: no runtime root; prompts will not be published");
        return Ok(());
    };
    let directory = root.join("prompts");
    fs::create_dir_all(&directory).context("create prompt spool directory")?;
    let zone = config
        .bus
        .zones
        .first()
        .cloned()
        .context("publishing prompts requires a zone")?;
    let origin = config.bus.producer_id.clone();
    thread::Builder::new()
        .name("canvas-prompt-bridge".into())
        .spawn(move || {
            loop {
                drain_prompts(&directory, &bus, &zone, &origin);
                thread::sleep(PROMPT_POLL);
            }
        })
        .context("spawn prompt bridge")?;
    Ok(())
}

/// Spool files in the order they were submitted.
///
/// The names lead with their timestamp, so a lexical sort is submission order:
/// two prompts typed in sequence must reach the bus in that sequence.
fn spooled_prompts(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut spooled: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    spooled.sort();
    spooled
}

/// A spool file's body, if it is publishable at all.
///
/// `None` means it never will be — malformed, oversize, or missing the two
/// fields that give it meaning — as distinct from a publish that failed and
/// should be retried.
fn parse_prompt(bytes: &[u8]) -> Option<serde_json::Value> {
    if bytes.len() as u64 > MAX_PROMPT_SPOOL_BYTES {
        return None;
    }
    let body: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let target = body.get("target").and_then(serde_json::Value::as_str)?;
    let text = body.get("text").and_then(serde_json::Value::as_str)?;
    if target.trim().is_empty() || text.trim().is_empty() {
        return None;
    }
    Some(body)
}

fn drain_prompts(directory: &std::path::Path, bus: &Bus, zone: &str, origin: &str) {
    for path in spooled_prompts(directory) {
        let Some(mut body) = fs::read(&path).ok().as_deref().and_then(parse_prompt) else {
            // Unparseable or oversize will never become publishable, so it is
            // dropped rather than retried forever — a bad file must not wedge
            // every prompt behind it.
            eprintln!("canvas-edge: discarding unusable prompt {}", path.display());
            let _ = fs::remove_file(&path);
            continue;
        };

        body["origin"] = serde_json::Value::String(origin.to_string());
        let message = canvas_core::unibus::Message::new(PROMPT_KIND, body);
        match bus
            .to_destination(zone, message)
            .and_then(|envelope| bus.send(&envelope, &[]))
        {
            Ok(()) => {
                // Removed only once it is on the bus: a crash between the two
                // re-sends rather than loses what someone typed.
                if let Err(error) = fs::remove_file(&path) {
                    eprintln!("canvas-edge: published prompt but could not clear spool: {error}");
                }
            }
            Err(error) => {
                // Left in place deliberately. The hub may be down; the next
                // tick tries again.
                eprintln!("canvas-edge: prompt publish failed, will retry: {error:#}");
                return;
            }
        }
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
    listener
        .set_nonblocking(true)
        .context("make workspace mirror socket interruptible")?;
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .context("secure workspace mirror socket")?;
    let zone = mirror
        .zone
        .clone()
        .or_else(|| config.bus.zones.first().cloned())
        .context("mirror requires a destination zone")?;
    let origin = config.bus.producer_id.clone();
    // The receiver retains the last sequence per producer. Seed from wall
    // time so restarting one edge does not reset it to 1 and make healthy
    // peers reject every new snapshot until the old counter is surpassed.
    let sequence = Arc::new(AtomicU64::new(canvas::stream_binding::now_ms()));
    let stop = Arc::new(AtomicBool::new(false));
    let listener_stop = Arc::clone(&stop);
    thread::Builder::new()
        .name("canvas-workspace-mirror".into())
        .spawn(move || {
            while !listener_stop.load(Ordering::Acquire) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Err(error) => {
                        eprintln!("canvas-edge: mirror socket accept failed: {error}");
                        continue;
                    }
                };
                if let Err(error) = stream.set_nonblocking(true) {
                    eprintln!("canvas-edge: mirror peer nonblocking setup failed: {error}");
                    continue;
                }
                // One thread per peer. Reading a connection inline meant this
                // listener served exactly one client at a time: a Canvas that
                // connected and then sat idle — which is what a reconnect after
                // a failed write leaves behind — held the only slot, and every
                // later connection waited in the backlog forever. Nothing
                // errored and nothing logged; mirroring simply stopped, which
                // is the worst way for it to fail.
                let bus = Arc::clone(&bus);
                let runtime = Arc::clone(&runtime);
                let sequence = Arc::clone(&sequence);
                let zone = zone.clone();
                let origin = origin.clone();
                let peer_stop = Arc::clone(&listener_stop);
                if let Err(error) = thread::Builder::new()
                    .name("canvas-workspace-mirror-peer".into())
                    .spawn(move || {
                        let mut reader = BufReader::new(stream);
                        let mut frame = Vec::new();
                        while !peer_stop.load(Ordering::Acquire) {
                            frame.clear();
                            match reader
                                .by_ref()
                                .take(canvas::control::MAX_CONTROL_MESSAGE + 1)
                                .read_until(b'\n', &mut frame)
                            {
                                Ok(0) => break,
                                Ok(_)
                                    if frame.len() as u64
                                        <= canvas::control::MAX_CONTROL_MESSAGE
                                        && frame.last() == Some(&b'\n') => {}
                                Ok(_) => {
                                    eprintln!("canvas-edge: rejected oversized mirror snapshot");
                                    break;
                                }
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    thread::sleep(Duration::from_millis(50));
                                    continue;
                                }
                                Err(error) => {
                                    eprintln!("canvas-edge: mirror socket receive failed: {error}");
                                    break;
                                }
                            }
                            frame.pop();
                            let local: LocalWorkspaceSnapshot = match serde_json::from_slice(&frame)
                            {
                                Ok(local) => local,
                                Err(error) => {
                                    eprintln!(
                                        "canvas-edge: rejected local mirror snapshot: {error}"
                                    );
                                    continue;
                                }
                            };
                            if local.schema_version != MIRROR_SCHEMA_VERSION {
                                eprintln!("canvas-edge: rejected unsupported local mirror schema");
                                continue;
                            }
                            let suppressed = runtime
                                .lock()
                                .ok()
                                .and_then(|mut state| {
                                    state.local_document = Some(local.document.clone());
                                    if state.suppress_snapshot.as_ref().is_some_and(|pending| {
                                        same_representation(pending, &local.document)
                                    }) {
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
                                assigned_output: local.assigned_output,
                            };
                            let message = canvas_core::unibus::Message::new(
                                MIRROR_KIND,
                                serde_json::to_value(state).expect("mirror state serializes"),
                            );
                            let result = bus
                                .to_destination(&zone, message)
                                .and_then(|envelope| bus.send(&envelope, &[]));
                            if let Err(error) = result {
                                eprintln!(
                                    "canvas-edge: publish workspace mirror failed: {error:#}"
                                );
                            }
                        }
                    })
                {
                    eprintln!("canvas-edge: mirror peer thread failed to start: {error}");
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
            assigned_output: None,
        };
        if let Ok(mut stream) = UnixStream::connect(&path) {
            if let Ok(mut bytes) = serde_json::to_vec(&snapshot) {
                bytes.push(b'\n');
                let _ = stream.write_all(&bytes);
            }
        }
    }
    Ok(Some(MirrorSocketGuard { path, stop }))
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
    let local_output = config
        .mirror
        .as_ref()
        .and_then(|mirror| mirror.output_id.as_deref())
        .or_else(|| config.bus.screen_anchors.first().map(String::as_str));
    let disposition = mirror_disposition(&current, &state, local_output);
    let (portable, activate) = match disposition {
        MirrorDisposition::Ignore => return Ok(()),
        MirrorDisposition::Legacy => (false, true),
        MirrorDisposition::Import { activate } => (true, activate),
    };
    // Snapshot QoS may legitimately redeliver the latest value after a peer
    // reconnects. Applying an identical document would restart transitions,
    // wake render work, and echo through the local publisher for no visual
    // change. Equality is the idempotency boundary for complete snapshots.
    {
        let mut runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("mirror runtime lock poisoned"))?;
        if activate || !portable {
            runtime.suppress_snapshot = Some(state.document.clone());
            runtime.local_document = Some(state.document.clone());
        }
    }
    let command = if portable {
        Command::WorkspaceImport {
            document: Box::new(state.document),
            assigned_output: state.assigned_output,
            local_output: local_output.map(str::to_owned),
            activate,
        }
    } else {
        Command::ApplyDocument {
            document: state.document,
        }
    };
    submit(ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: format!("mirror-{}-{}", state.origin, state.sequence),
        expected_revision: None,
        actor: None,
        command,
    })
    .map(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MirrorDisposition {
    Ignore,
    Legacy,
    Import { activate: bool },
}

fn mirror_disposition(
    current: &canvas::WorkspaceDocument,
    state: &WorkspaceState,
    local_output: Option<&str>,
) -> MirrorDisposition {
    match state.assigned_output.as_deref() {
        Some(assigned) => MirrorDisposition::Import {
            activate: Some(assigned) == local_output,
        },
        None if should_apply_workspace_state(current, state) => MirrorDisposition::Legacy,
        None => MirrorDisposition::Ignore,
    }
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
    // The event log is the decision; the probability bars and the fan are
    // extra views of it. An extra view that cannot be built degrades itself
    // and nothing else — propagating its error here discarded the log that
    // was already in hand, so one odd event blanked every Speedy widget at
    // once. Same reasoning as the oversize guard below.
    let log = rlcd_decision_event(liveness, envelope)?;
    let mut events = vec![log];
    match rlcd_probability_event(envelope) {
        Ok(Some(probabilities)) => events.push(probabilities),
        Ok(None) => {}
        Err(error) => eprintln!("canvas-edge: no probability view for this decision: {error:#}"),
    }
    match rlcd_decision_fan_event(liveness, envelope) {
        Ok(Some(fan)) => events.push(fan),
        Ok(None) => {}
        Err(error) => eprintln!("canvas-edge: no decision fan for this decision: {error:#}"),
    }
    for event in events {
        let bytes = serde_json::to_vec(&event).context("encode Canvas RLCD event")?;
        if bytes.len() > MAX_LOCAL_STREAM_DATAGRAM {
            // Drop the one snapshot that does not fit, not the whole decision.
            // Aborting here cost the event log its update too, so an oversized
            // fan blanked the surfaces that were working fine.
            eprintln!(
                "canvas-edge: skipped {} snapshot at {} bytes (local socket limit {})",
                event.stream,
                bytes.len(),
                MAX_LOCAL_STREAM_DATAGRAM
            );
            continue;
        }
        // ENOBUFS means Canvas has not drained the socket yet, which is
        // transient — four games deciding at once burst faster than one frame
        // consumes. Retry briefly, then drop this one snapshot. Propagating
        // the error abandoned the rest of the decision, which is how a busy
        // moment used to blank the widgets that were working.
        let mut attempt = 0;
        loop {
            match socket.send_to(&bytes, &path) {
                Ok(_) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(libc::ENOBUFS) =>
                {
                    attempt += 1;
                    if attempt > STREAM_SEND_ATTEMPTS {
                        eprintln!(
                            "canvas-edge: dropped {} snapshot after {} attempts: {error}",
                            event.stream, STREAM_SEND_ATTEMPTS
                        );
                        break;
                    }
                    thread::sleep(Duration::from_millis(u64::from(2 * attempt)));
                }
                Err(error) => {
                    eprintln!("canvas-edge: dropped {} snapshot: {error}", event.stream);
                    break;
                }
            }
        }
    }
    Ok(())
}

/// How much of the trunk stays on screen behind the live decision.
///
/// The trunk is history and competes with the part anyone is actually watching,
/// so it is short and fades. Raising this costs datagram budget that the fan
/// needs more (see `MAX_LOCAL_STREAM_DATAGRAM`).
const FAN_TRUNK_DEPTH: usize = 4;
/// Alternatives drawn around the live decision. The tail of a posterior is
/// visually indistinguishable from zero, so it is not worth the bytes.
const FAN_WIDTH: usize = 5;

/// A decision fan: the recent trunk, and the live decision blooming into every
/// action that was considered.
///
/// This is deliberately one level deep, because that is the shape of the data.
/// Every decision here is an independent `Answer::Choice` over an offered set —
/// nothing on the wire says one decision followed from another beyond sequence
/// order. Drawing a deeper tree would be inventing structure the model never
/// produced. What is real, and what this draws, is the posterior: the paths not
/// taken, scored, fading against the one that was.
fn rlcd_decision_fan_event(
    liveness: &mut Liveness,
    envelope: &Envelope,
) -> Result<Option<StreamEnvelope>> {
    let body = &envelope.message.body;
    let session = body
        .get("session_id")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
        .context("RLCD event lacks session_id")?;
    let sequence = body
        .get("sequence")
        .unwrap_or(&serde_json::Value::Null)
        .as_u64()
        .context("RLCD event lacks sequence")?;
    let phase = body
        .get("phase")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
        .context("RLCD event lacks phase")?;

    // A decision still in flight carries no response at all, so this walks the
    // path rather than indexing it — indexing absent keys panics.
    let mut alternatives: Vec<(String, f64)> = body
        .get("response")
        .and_then(|response| response.get("answers"))
        .and_then(|answers| answers.get("action"))
        .and_then(|action| action.get("probabilities"))
        .and_then(serde_json::Value::as_object)
        .map(|scores| {
            scores
                .iter()
                .filter_map(|(key, value)| value.as_f64().map(|score| (key.clone(), score)))
                .collect()
        })
        .unwrap_or_default();
    alternatives.sort_by(|left, right| right.1.total_cmp(&left.1));
    alternatives.truncate(FAN_WIDTH);

    let selected = body
        .get("selected")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let confidence = selected.as_ref().and_then(|chosen| {
        alternatives
            .iter()
            .find(|(action, _)| action == chosen)
            .map(|(_, score)| *score)
    });
    let entry = FanEntry {
        sequence,
        selected,
        confidence,
        alternatives,
        failed: phase == "failed",
    };

    let trunk = liveness
        .fan_by_session
        .entry(session.to_owned())
        .or_default();
    // A decision is re-reported as it moves requested -> completed, so replace
    // rather than append or the trunk fills with duplicates of one decision.
    match trunk.iter().position(|held| held.sequence == sequence) {
        Some(index) => trunk[index] = entry,
        None => trunk.push_back(entry),
    }
    while trunk.len() > FAN_TRUNK_DEPTH {
        trunk.pop_front();
    }
    let trunk: Vec<FanEntry> = trunk.iter().cloned().collect();
    let Some(live) = trunk.last() else {
        return Ok(None);
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let depth = trunk.len();
    for (index, decision) in trunk.iter().enumerate() {
        // Oldest fades toward idle; the live decision is fully lit. This is the
        // trunk compression: history stays legible without taking the eye.
        let live_node = index + 1 == depth;
        // Heat is confidence, never recency: position already carries time,
        // left to right. A decision nobody scored sits dim rather than
        // borrowing brightness from being recent.
        let heat = decision.confidence.unwrap_or(0.0) as f32;
        let id = format!("d{}", decision.sequence);
        // The live decision's answer is exploded directly beneath it, ticked
        // and with its odds. Repeating the word here too made the same pick
        // read as two separate things, so the live node carries only its
        // number and the history carries what it settled on.
        let label = match (live_node, decision.selected.as_deref()) {
            (true, None) => format!("#{} deciding…", decision.sequence),
            (true, Some(_)) => format!("#{}", decision.sequence),
            (false, Some(chosen)) => format!("#{} {chosen}", decision.sequence),
            (false, None) => format!("#{} —", decision.sequence),
        };
        nodes.push(serde_json::json!({
            "id": id,
            "label": label,
            "group": "decided",
            "heat": heat,
            "state": if decision.failed { "warning" }
                     else if decision.selected.is_some() { "good" }
                     else { "neutral" },
        }));
        if index > 0 {
            edges.push(serde_json::json!({
                "from": format!("d{}", trunk[index - 1].sequence),
                "to": id,
                "heat": heat,
            }));
        }
    }

    // Only the live decision blooms. Fanning the whole trunk would bury the
    // thing being watched and blow the datagram budget.
    for (rank, (action, score)) in live.alternatives.iter().enumerate() {
        let id = format!("a{rank}");
        let chosen = live.selected.as_deref() == Some(action.as_str());
        // Colour alone could not answer "which one did it pick?" — the tick
        // says it outright, and the percentage spares the eye from having to
        // rank shades against each other.
        let label = if chosen {
            format!("✓ {action} {}%", percent(*score))
        } else {
            format!("{action} {}%", percent(*score))
        };
        nodes.push(serde_json::json!({
            "id": id,
            "label": label,
            "group": if chosen { "taken" } else { "considered" },
            "heat": *score as f32,
            "state": if chosen { "good" } else { "neutral" },
        }));
        edges.push(serde_json::json!({
            "from": format!("d{}", live.sequence),
            "to": id,
            "heat": *score as f32,
        }));
    }

    // The title answers the question from across the room, before any of the
    // graph is read: what did it just do, and how sure was it.
    let headline = match (&live.selected, live.confidence) {
        _ if live.failed => "failed".to_string(),
        (Some(chosen), Some(score)) => format!("{chosen} {}%", percent(score)),
        (Some(chosen), None) => chosen.clone(),
        (None, _) => "deciding…".to_string(),
    };
    let considered = live.alternatives.len();
    Ok(Some(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: format!("{session}-fan"),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence,
        emitted_at_ms: body
            .get("at_unix_ms")
            .unwrap_or(&serde_json::Value::Null)
            .as_u64()
            .unwrap_or(envelope.issued_at_unix_ms),
        project_id: Some(rlcd_project(body)),
        payload: serde_json::json!({
            "widget": {
                "kind": "dag_graph",
                "title": format!("#{sequence} · {headline} · of {considered}"),
                "nodes": nodes,
                "edges": edges,
            },
            "data": {
                "state": if live.failed { "error" } else { "ready" },
                "source": format!("unibus:{}:rlcd.decision.v1", envelope.producer_id),
                "window": "live",
            }
        }),
    }))
}

/// Which board an RLCD widget belongs to.
///
/// Every producer of `rlcd.decision.v1` used to be a Speedy game, so the
/// project was hardcoded and the widgets could only ever route to that one
/// board. Other tools run the same decision loop and deserve the same view, so
/// a producer may now name its own board; Speedy's agents predate the field and
/// keep working without it.
fn rlcd_project(body: &serde_json::Map<String, serde_json::Value>) -> String {
    body.get("project")
        .and_then(serde_json::Value::as_str)
        .filter(|project| !project.is_empty())
        .unwrap_or("speedy")
        .to_owned()
}

fn rlcd_probability_event(envelope: &Envelope) -> Result<Option<StreamEnvelope>> {
    let body = &envelope.message.body;
    if body
        .get("phase")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
        != Some("completed")
    {
        return Ok(None);
    }
    let session = body
        .get("session_id")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
        .context("RLCD event lacks session_id")?;
    let sequence = body
        .get("sequence")
        .unwrap_or(&serde_json::Value::Null)
        .as_u64()
        .context("RLCD event lacks sequence")?;
    let selected = body
        .get("selected")
        .unwrap_or(&serde_json::Value::Null)
        .as_str();
    // Absent probabilities means a producer that does not report them, not a
    // malformed event. Treating it as an error aborted the whole decision —
    // including the event log already built — so every Speedy widget stopped
    // updating and showed Disconnected.
    let Some(values) = body
        .get("response")
        .and_then(|value| value.pointer("/answers/action/probabilities"))
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
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
    let headline = match selected {
        Some(action) => {
            let taken = bars
                .iter()
                .find(|bar| bar["label"] == action.replace('_', " "))
                .and_then(|bar| bar["value"].as_f64());
            match taken {
                Some(value) => format!("{} {:.0}%", action.replace('_', " "), value),
                None => action.replace('_', " "),
            }
        }
        None => "deciding…".to_string(),
    };

    Ok(Some(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: envelope.producer_id.clone(),
        stream: format!("{session}-probabilities"),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence,
        emitted_at_ms: body
            .get("at_unix_ms")
            .unwrap_or(&serde_json::Value::Null)
            .as_u64()
            .unwrap_or(envelope.issued_at_unix_ms),
        project_id: Some(rlcd_project(body)),
        payload: serde_json::json!({
            "widget": {
                "kind": "ranked_bar",
                // The action taken and how sure it was, and nothing else. The
                // sequence number read as the loudest thing on the tile while
                // being the least useful at a glance, and the confidence —
                // the one number worth crossing the room for — was missing.
                "title": headline,
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
        body.get("schema_version")
            .unwrap_or(&serde_json::Value::Null)
            .as_u64()
            == Some(1),
        "unsupported RLCD schema"
    );
    let session = body
        .get("session_id")
        .unwrap_or(&serde_json::Value::Null)
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
    let sequence = body
        .get("sequence")
        .unwrap_or(&serde_json::Value::Null)
        .as_u64()
        .context("RLCD event lacks sequence")?;
    let emitted_at_ms = body
        .get("at_unix_ms")
        .unwrap_or(&serde_json::Value::Null)
        .as_u64()
        .unwrap_or(envelope.issued_at_unix_ms);
    let phase = body
        .get("phase")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
        .context("RLCD event lacks phase")?;
    ensure!(
        matches!(phase, "requested" | "completed" | "failed"),
        "invalid RLCD phase"
    );
    let message = match phase {
        "requested" => "thinking…".to_string(),
        // Terse on purpose: this sits in a strip a few characters tall. The
        // sequence number was the widest thing on every line and told a viewer
        // nothing, and a baseline policy reports confidence 1.00 every single
        // time — printing it on every row spends the width on a constant.
        // Confidence appears only when it is not certain, which is exactly
        // when it is worth reading.
        "completed" => {
            let action = body
                .get("selected")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .replace('_', " ");
            let latency = body
                .get("latency_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            match body.get("confidence").and_then(serde_json::Value::as_f64) {
                Some(confidence) if confidence < 0.999 => {
                    format!("{action} · {latency}ms · {:.0}%", confidence * 100.0)
                }
                _ => format!("{action} · {latency}ms"),
            }
        }
        "failed" => format!(
            "failed · {}",
            body.get("error")
                .unwrap_or(&serde_json::Value::Null)
                .as_str()
                .unwrap_or("unknown error")
        ),
        _ => unreachable!(),
    };
    let entries = liveness
        .rlcd_by_session
        .entry(session.to_owned())
        .or_default();
    // A producer that omits call_id is a producer whose rows cannot be
    // de-duplicated, not a reason to take the edge down.
    let call_id = body
        .get("call_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if phase != "requested" && !call_id.is_null() {
        entries.retain(|entry| entry["call_id"] != call_id);
    }
    entries.push_front(serde_json::json!({
        "call_id": call_id,
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
        project_id: Some(rlcd_project(body)),
        payload: serde_json::json!({
            "widget": {
                "kind": "event_log",
                // The surface already names the game, and chrome titles are
                // hidden on this board, so "RLCD · smb" spent a line repeating
                // the tile it sits in.
                "title": body
                    .get("game")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(session)
                    .to_string(),
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

fn submit(request: ControlRequest) -> Result<Option<serde_json::Value>> {
    request.validate().map_err(anyhow::Error::msg)?;
    let reply = canvas::control::send(&request).map_err(anyhow::Error::msg)?;
    // The hub ack proves delivery; the receipt must carry what Canvas
    // actually answered, including application-level failures.
    match reply.outcome {
        canvas::control::Outcome::Ok { receipt } => Ok(Some(
            serde_json::to_value(receipt).context("encode Canvas receipt")?,
        )),
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
    data: Option<serde_json::Value>,
) -> Result<()> {
    let message = result_message_with_data(request_id, ok, error, data);
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
            assigned_output: None,
        };
        assert!(!should_apply_workspace_state(&document, &state));
        state.document.revision += 1;
        assert!(should_apply_workspace_state(&document, &state));
        state.workspace_key = "another-workspace".into();
        assert!(!should_apply_workspace_state(&document, &state));
    }

    #[test]
    fn portable_workspace_is_retained_everywhere_and_active_only_at_its_output() {
        let document: canvas::WorkspaceDocument =
            serde_json::from_str(include_str!("../../../../config/workspaces/tv/speedy.json"))
                .unwrap();
        let state = WorkspaceState {
            schema_version: MIRROR_SCHEMA_VERSION,
            workspace_key: document.id.clone(),
            origin: "screen-neo".into(),
            sequence: 8,
            document: document.clone(),
            assigned_output: Some("screen-tv".into()),
        };
        assert_eq!(
            mirror_disposition(&document, &state, Some("screen-tv")),
            MirrorDisposition::Import { activate: true }
        );
        for output in ["screen-neo", "screen-studio"] {
            assert_eq!(
                mirror_disposition(&document, &state, Some(output)),
                MirrorDisposition::Import { activate: false }
            );
        }
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
        let message = canvas_core::unibus::result_message("r4", false, Some("denied".into()));
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
        assert!(message.contains("44ms"));
        // The row sits in a strip a few characters tall, so what is *absent*
        // matters as much as what is present: the sequence number was the
        // widest thing on the line, and a baseline reports confidence 1.00
        // every time.
        assert!(
            !message.contains('#'),
            "the sequence number is not worth the width: {message}"
        );
        assert!(
            !message.contains("confidence"),
            "a certain decision should not spend a line saying so: {message}"
        );

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

#[cfg(test)]
mod decision_fan_tests {
    use super::*;
    use canvas_core::unibus::Message;

    fn decision(sequence: u64, phase: &str, selected: Option<&str>) -> Envelope {
        let mut body = serde_json::json!({
            "schema_version": 1,
            "session_id": "retro-umk3",
            "call_id": format!("c{sequence}"),
            "sequence": sequence,
            "at_unix_ms": 1_790_016_064_000_u64 + sequence,
            "phase": phase,
            "game": "umk3",
        });
        if let Some(selected) = selected {
            body["selected"] = selected.into();
            // A real posterior from a captured UMK3 run: every offered action
            // scored, not just the one taken.
            body["response"] = serde_json::json!({"answers": {"action": {"probabilities": {
                "uppercut": 0.24, "block": 0.20, "advance": 0.19, "sweep": 0.19,
                "retreat": 0.07, "high_punch": 0.05, "jump_kick": 0.03, "low_punch": 0.03
            }}}});
        }
        Envelope::new(
            "home",
            "mac-studio-speedy-umk3",
            None,
            "",
            Message::new("rlcd.decision.v1", body),
        )
    }

    fn fan_envelope(liveness: &mut Liveness, envelope: &Envelope) -> StreamEnvelope {
        rlcd_decision_fan_event(liveness, envelope)
            .expect("fan composes")
            .expect("fan present")
    }

    fn fan(liveness: &mut Liveness, envelope: &Envelope) -> serde_json::Value {
        fan_envelope(liveness, envelope).payload
    }

    /// The whole point of the view: paths nobody took are drawn with their real
    /// posterior so they fade honestly, rather than being dropped or invented.
    #[test]
    fn every_considered_action_is_drawn_with_its_own_score() {
        let mut liveness = Liveness::default();
        let payload = fan(&mut liveness, &decision(1, "completed", Some("uppercut")));
        let nodes = payload["widget"]["nodes"].as_array().unwrap();

        let taken: Vec<_> = nodes.iter().filter(|n| n["group"] == "taken").collect();
        assert_eq!(taken.len(), 1, "exactly one path was taken");
        assert_eq!(
            taken[0]["label"], "✓ uppercut 24%",
            "the pick is named outright, not left to be inferred from a shade"
        );

        let considered: Vec<_> = nodes
            .iter()
            .filter(|n| n["group"] == "considered")
            .collect();
        assert_eq!(
            considered.len(),
            FAN_WIDTH - 1,
            "the rest fan out around it"
        );
        let block = considered
            .iter()
            .find(|n| n["label"] == "block 20%")
            .expect("an untaken action states its own odds");
        assert!(
            (block["heat"].as_f64().unwrap() - 0.20).abs() < 1e-6,
            "an untaken path fades by its real posterior, not a placeholder"
        );
    }

    /// Every decision is an independent choice; nothing on the wire says one
    /// followed from another. A second level would be invented structure.
    #[test]
    fn the_fan_is_one_level_deep() {
        let mut liveness = Liveness::default();
        let payload = fan(&mut liveness, &decision(1, "completed", Some("uppercut")));
        let edges = payload["widget"]["edges"].as_array().unwrap();
        let alternatives: Vec<_> = edges
            .iter()
            .filter(|e| e["to"].as_str().unwrap().starts_with('a'))
            .collect();
        assert!(
            alternatives.iter().all(|e| e["from"] == "d1"),
            "every considered action hangs off the live decision, nothing deeper"
        );
    }

    /// The trunk is history: bounded, and fading toward the live decision.
    #[test]
    fn the_trunk_is_bounded_and_fades_into_the_past() {
        let mut liveness = Liveness::default();
        let mut payload = serde_json::Value::Null;
        for sequence in 1..=(FAN_TRUNK_DEPTH as u64 + 4) {
            payload = fan(
                &mut liveness,
                &decision(sequence, "completed", Some("block")),
            );
        }
        let nodes = payload["widget"]["nodes"].as_array().unwrap();
        let trunk: Vec<_> = nodes.iter().filter(|n| n["group"] == "decided").collect();
        assert_eq!(trunk.len(), FAN_TRUNK_DEPTH, "history is capped");
        // Heat is confidence now, not recency — every one of these decisions
        // picked "block" at the same 0.20, so they are equally lit and the
        // passage of time is carried by position instead. Mixing the two
        // meanings into one channel is what made the graph unreadable.
        let heats: Vec<f64> = trunk.iter().map(|n| n["heat"].as_f64().unwrap()).collect();
        assert!(
            heats.iter().all(|heat| (heat - 0.20).abs() < 1e-6),
            "each node is lit by how sure that decision was: {heats:?}"
        );
        // Order still reads left to right, and only the live node drops the
        // word for its choice.
        let labels: Vec<&str> = trunk.iter().map(|n| n["label"].as_str().unwrap()).collect();
        assert_eq!(
            labels.last().copied(),
            Some("#8"),
            "live node is bare: {labels:?}"
        );
        assert!(
            labels[..labels.len() - 1]
                .iter()
                .all(|label| label.ends_with(" block")),
            "history says what it settled on: {labels:?}"
        );
    }

    /// A call still in flight draws a thinking node, and is replaced in place
    /// when it completes rather than leaving a duplicate behind.
    #[test]
    fn an_in_flight_decision_becomes_its_result() {
        let mut liveness = Liveness::default();
        let pending = fan(&mut liveness, &decision(7, "requested", None));
        let thinking: Vec<_> = pending["widget"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["label"] == "#7 deciding…")
            .collect();
        assert_eq!(thinking.len(), 1);

        let done = fan(&mut liveness, &decision(7, "completed", Some("sweep")));
        let trunk: Vec<_> = done["widget"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["group"] == "decided")
            .collect();
        assert_eq!(trunk.len(), 1, "the same decision, not two");
        // The live decision carries only its number: its answer is exploded
        // directly beneath it, ticked, so naming it twice read as two things.
        assert_eq!(trunk[0]["label"], "#7");
    }

    /// A producer that reports no probabilities is normal, not broken. This
    /// used to return Err, and the caller's `?` discarded the decision's event
    /// log along with it — so one such event left every Speedy widget showing
    /// Disconnected until something else happened to refresh them.
    #[test]
    fn a_completed_decision_without_probabilities_is_not_an_error() {
        // `decision` attaches probabilities only alongside `selected`, so a
        // completed decision with none is exactly this shape.
        let mut envelope = decision(7, "completed", Some("left"));
        envelope.message.body.remove("response");

        assert!(
            rlcd_probability_event(&envelope)
                .expect("absent probabilities must not be an error")
                .is_none(),
            "no probabilities means no bar chart, not a failed decision"
        );

        // The part that matters: the decision itself still produces a log.
        let mut liveness = Liveness::default();
        assert!(
            rlcd_decision_event(&mut liveness, &envelope).is_ok(),
            "the event log must survive a decision with no probabilities"
        );
    }

    /// Canvas's stream socket is a Unix datagram; macOS caps it at 2048 and the
    /// edge refuses anything over 1900. A fan that exceeds it is never drawn.
    #[test]
    fn a_full_fan_fits_the_local_datagram() {
        let mut liveness = Liveness::default();
        let mut envelope = None;
        // Long action names and a long session id, because the guard measures
        // the whole envelope and the first version of this test measured only
        // the payload — so it passed while the real board dropped every event.
        for sequence in 1..=(FAN_TRUNK_DEPTH as u64 + 2) {
            let mut decision = decision(sequence, "completed", Some("uppercut"));
            decision.message.body["session_id"] = "retro-umk3-tournament-long".into();
            envelope = Some(fan_envelope(&mut liveness, &decision));
        }
        let encoded = serde_json::to_vec(&envelope.expect("fan")).unwrap();
        assert!(
            encoded.len() <= MAX_LOCAL_STREAM_DATAGRAM,
            "fan envelope is {} bytes, over the {MAX_LOCAL_STREAM_DATAGRAM} limit",
            encoded.len()
        );
    }
}

#[cfg(test)]
mod rlcd_project_tests {
    use super::*;

    /// Speedy's agents predate the field and must keep routing to their board.
    #[test]
    fn a_producer_without_a_project_is_still_speedy() {
        let plain = serde_json::json!({"session_id": "retro-smb"});
        let empty = serde_json::json!({"project": ""});
        assert_eq!(rlcd_project(plain.as_object().unwrap()), "speedy");
        assert_eq!(rlcd_project(empty.as_object().unwrap()), "speedy");
    }

    /// Other tools run the same loop and deserve the same view on their own
    /// board; hardcoding the project meant only Speedy could ever have one.
    #[test]
    fn a_producer_may_name_its_own_board() {
        let named = serde_json::json!({"project": "surf"});
        assert_eq!(rlcd_project(named.as_object().unwrap()), "surf");
    }
}

#[cfg(test)]
mod rlcd_robustness_tests {
    use super::*;
    use canvas_core::unibus::Message;

    fn envelope(body: serde_json::Value) -> Envelope {
        Envelope::new(
            "home",
            "mac-studio-speedy-smb",
            None,
            "",
            Message::new("rlcd.decision.v1", body),
        )
    }

    /// A decision event missing `confidence` took canvas-edge down: the body is
    /// a serde_json::Map, and indexing a Map with an absent key panics rather
    /// than yielding null. One optional field from one producer crash-looped
    /// the process that feeds every RLCD widget on the wall.
    #[test]
    fn a_decision_without_optional_fields_does_not_panic() {
        let mut liveness = Liveness::default();
        let sparse = envelope(serde_json::json!({
            "schema_version": 1,
            "session_id": "retro-smb",
            "sequence": 1,
            "phase": "completed",
            // no selected, latency_ms, confidence, at_unix_ms, game, response
        }));
        rlcd_decision_event(&mut liveness, &sparse).expect("sparse completed event survives");
        rlcd_decision_fan_event(&mut liveness, &sparse).expect("fan survives");
        // Rejecting an event with no probabilities is correct; panicking on one
        // is not. Only the second is a defect, so assert the call returns.
        let _ = rlcd_probability_event(&sparse);
    }

    #[test]
    fn a_failed_decision_without_an_error_string_does_not_panic() {
        let mut liveness = Liveness::default();
        let sparse = envelope(serde_json::json!({
            "schema_version": 1, "session_id": "retro-smb", "sequence": 2, "phase": "failed",
        }));
        rlcd_decision_event(&mut liveness, &sparse).expect("sparse failed event survives");
    }

    /// Malformed traffic is a reason to reject that event, never to exit.
    #[test]
    fn an_event_missing_required_fields_is_rejected_not_fatal() {
        let mut liveness = Liveness::default();
        let empty = envelope(serde_json::json!({}));
        assert!(rlcd_decision_event(&mut liveness, &empty).is_err());
        assert!(rlcd_decision_fan_event(&mut liveness, &empty).is_err());
    }
}

#[cfg(test)]
mod prompt_spool_tests {
    use super::*;

    fn spool(directory: &std::path::Path, name: &str, body: &str) {
        fs::write(directory.join(name), body).expect("spool write");
    }

    fn usable(target: &str, text: &str) -> String {
        serde_json::json!({
            "schema_version": 1, "target": target, "text": text, "at_unix_ms": 1u64
        })
        .to_string()
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("canvas-prompt-{tag}"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch");
        directory
    }

    /// The spool is a queue: two prompts typed in order must publish in order.
    #[test]
    fn prompts_drain_in_submission_order() {
        let directory = scratch("order");
        spool(
            &directory,
            "0000000000002-000001-transmog.json",
            &usable("transmog", "second"),
        );
        spool(
            &directory,
            "0000000000001-000001-transmog.json",
            &usable("transmog", "first"),
        );

        let texts: Vec<String> = spooled_prompts(&directory)
            .iter()
            .filter_map(|path| fs::read(path).ok())
            .filter_map(|bytes| parse_prompt(&bytes))
            .map(|body| body["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn only_json_is_drained() {
        let directory = scratch("ext");
        spool(
            &directory,
            "0000000000001-000001-transmog.json",
            &usable("transmog", "yes"),
        );
        spool(
            &directory,
            "0000000000002-000001-transmog.json.tmp",
            &usable("transmog", "partial"),
        );
        // An atomic write lands via a temporary name; draining it would publish
        // a half-written file.
        assert_eq!(spooled_prompts(&directory).len(), 1);
    }

    /// Unusable is distinct from "publish failed": these can never succeed, so
    /// the drain must drop them rather than block every prompt behind one.
    #[test]
    fn unusable_spool_files_are_rejected_not_retried() {
        assert!(parse_prompt(b"not json at all").is_none());
        assert!(
            parse_prompt(
                serde_json::json!({"text": "no target"})
                    .to_string()
                    .as_bytes()
            )
            .is_none()
        );
        assert!(
            parse_prompt(
                serde_json::json!({"target": "transmog"})
                    .to_string()
                    .as_bytes()
            )
            .is_none()
        );
        assert!(
            parse_prompt(usable("transmog", "   ").as_bytes()).is_none(),
            "whitespace is not a prompt"
        );
        let huge = usable("transmog", &"x".repeat(MAX_PROMPT_SPOOL_BYTES as usize + 1));
        assert!(
            parse_prompt(huge.as_bytes()).is_none(),
            "oversize never came from the wall"
        );
    }

    #[test]
    fn a_usable_prompt_keeps_its_target_and_text() {
        let body =
            parse_prompt(usable("transmog", "a bracket 40mm tall").as_bytes()).expect("usable");
        assert_eq!(body["target"], "transmog");
        assert_eq!(body["text"], "a bracket 40mm tall");
    }
}

#[cfg(test)]
mod camera_bridge_tests {
    use super::{CameraSeen, changed_cameras};
    use canvas_core::model_camera::ViewerCamera;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn seen() -> CameraSeen {
        Arc::new(Mutex::new(BTreeMap::new()))
    }

    fn write(directory: &std::path::Path, stream: &str, pose: &ViewerCamera) {
        std::fs::write(
            directory.join(format!("{stream}.camera")),
            serde_json::to_vec(pose).expect("serialises"),
        )
        .expect("writes");
    }

    #[test]
    fn a_new_pose_is_forwarded_once_and_then_stays_quiet() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        let pose = ViewerCamera::new("transmog-step", 45.0, 20.0, 1.0, true, 10);
        write(directory.path(), "transmog-step", &pose);

        let first = changed_cameras(directory.path(), &seen);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, "transmog-step");

        // Same pose, still on disk: forwarding it again would be pure traffic.
        assert!(changed_cameras(directory.path(), &seen).is_empty());
    }

    /// The echo guard. Without it a host that receives a pose writes it to the
    /// file the forwarder watches, reads back a change, and publishes it
    /// straight home -- two screens trading one pose forever.
    #[test]
    fn a_pose_written_by_a_receiver_is_not_published_back() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        let pose = ViewerCamera::new("transmog-step", 10.0, 5.0, 1.0, false, 99);

        // Exactly what apply_camera_pose does: record, then write.
        seen.lock().unwrap().insert(
            "transmog-step".into(),
            serde_json::to_string(&pose).unwrap(),
        );
        write(directory.path(), "transmog-step", &pose);

        assert!(
            changed_cameras(directory.path(), &seen).is_empty(),
            "an arriving pose must not be forwarded back to its sender"
        );
    }

    #[test]
    fn a_later_pose_for_the_same_stream_is_forwarded() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        write(
            directory.path(),
            "transmog-step",
            &ViewerCamera::new("transmog-step", 0.0, 0.0, 1.0, true, 1),
        );
        assert_eq!(changed_cameras(directory.path(), &seen).len(), 1);
        write(
            directory.path(),
            "transmog-step",
            &ViewerCamera::new("transmog-step", 30.0, 0.0, 1.0, true, 2),
        );
        assert_eq!(
            changed_cameras(directory.path(), &seen).len(),
            1,
            "a moved camera is news"
        );
    }

    /// A pose is written by replacing the file, so a reader can catch it
    /// mid-flight. Anything that does not parse is skipped rather than
    /// forwarded, and skipping must not mark it seen -- the real pose arrives
    /// under the same name a moment later.
    #[test]
    fn an_unparseable_file_is_skipped_and_not_remembered() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        std::fs::write(directory.path().join("transmog-step.camera"), b"{\"yaw\"").unwrap();
        assert!(changed_cameras(directory.path(), &seen).is_empty());
        assert!(
            seen.lock().unwrap().is_empty(),
            "junk must not be remembered as seen"
        );

        write(
            directory.path(),
            "transmog-step",
            &ViewerCamera::new("transmog-step", 5.0, 5.0, 1.0, true, 3),
        );
        assert_eq!(changed_cameras(directory.path(), &seen).len(), 1);
    }

    /// The frames directory is full of .rgba and .size files written many times
    /// a second. Reading those as poses would be constant wasted work.
    #[test]
    fn only_camera_files_are_considered() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        std::fs::write(directory.path().join("cat.rgba"), vec![0_u8; 64]).unwrap();
        std::fs::write(directory.path().join("cat.size"), b"1280x720").unwrap();
        assert!(changed_cameras(directory.path(), &seen).is_empty());
    }

    #[test]
    fn two_streams_are_tracked_separately() {
        let directory = tempfile::tempdir().expect("tempdir");
        let seen = seen();
        write(
            directory.path(),
            "model-a",
            &ViewerCamera::new("model-a", 1.0, 0.0, 1.0, true, 1),
        );
        write(
            directory.path(),
            "model-b",
            &ViewerCamera::new("model-b", 2.0, 0.0, 1.0, true, 1),
        );
        assert_eq!(changed_cameras(directory.path(), &seen).len(), 2);

        write(
            directory.path(),
            "model-a",
            &ViewerCamera::new("model-a", 9.0, 0.0, 1.0, true, 2),
        );
        let changed = changed_cameras(directory.path(), &seen);
        assert_eq!(changed.len(), 1, "only the stream that moved");
        assert_eq!(changed[0].0, "model-a");
    }
}
