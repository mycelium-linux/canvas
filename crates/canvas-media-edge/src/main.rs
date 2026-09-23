//! Headless Unibus-to-media-plane adapter for native Canvas camera surfaces.

use anyhow::{Context, Result, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas::stream_contract::{BRIEFING_CONTRACT, STREAM_SCHEMA_VERSION, StreamEnvelope};
use canvas_media_edge::{
    DecodePolicy, HardwareDecoder, gstreamer_raw_plan, now_unix_ms, write_detections,
    write_raw_frame,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{BufReader, Read},
    os::unix::net::UnixDatagram,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
use unibus_media::{
    CAMERA_DETECTIONS_CONTRACT, CAMERA_STREAM_CONTRACT, CameraDetections, CameraStream,
    VIDEO_STREAM_CONTRACT, VideoStream,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeConfig {
    bus: BusConfig,
    #[serde(default)]
    runtime_dir: Option<PathBuf>,
    #[serde(default = "default_gst_launch")]
    gst_launch: PathBuf,
    #[serde(default = "default_ffmpeg")]
    ffmpeg: PathBuf,
    #[serde(default = "default_max_height")]
    max_height: u32,
    #[serde(default = "default_max_fps")]
    max_fps: f64,
    #[serde(default)]
    hardware_decoder: Option<String>,
}

fn default_gst_launch() -> PathBuf {
    PathBuf::from("gst-launch-1.0")
}
fn default_ffmpeg() -> PathBuf {
    PathBuf::from("ffmpeg")
}
fn default_max_height() -> u32 {
    720
}
fn default_max_fps() -> f64 {
    24.0
}

impl EdgeConfig {
    fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
    }

    fn runtime_root(&self) -> Result<PathBuf> {
        self.runtime_dir
            .clone()
            .or_else(canvas_core::runtime_root)
            .context("set runtime_dir, CANVAS_CONFIG_DIR, or the platform Canvas config directory")
    }

    fn policy(&self) -> Result<DecodePolicy> {
        let decoder = match self.hardware_decoder.as_deref() {
            Some(value) => HardwareDecoder::parse(value)?,
            None => HardwareDecoder::native()?,
        };
        let policy = DecodePolicy {
            max_height: self.max_height,
            max_fps: self.max_fps,
            decoder,
        };
        policy.validate()?;
        Ok(policy)
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-media-edge <config.json>")?;
    let config = EdgeConfig::read(Path::new(&path))?;
    let root = config.runtime_root()?;
    let policy = config.policy()?;
    let mut backoff = Duration::from_secs(1);
    loop {
        match run(&config, &root, policy) {
            Ok(()) => eprintln!("canvas-media-edge: hub closed; reconnecting"),
            Err(error) => eprintln!("canvas-media-edge: {error:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

fn run(config: &EdgeConfig, root: &Path, policy: DecodePolicy) -> Result<()> {
    let bus = Bus::connect(config.bus.clone())?;
    eprintln!(
        "canvas-media-edge: registered '{}' via {}; decoder={:?}",
        config.bus.producer_id, config.bus.hub_endpoint, policy.decoder
    );
    let mut decoders = DecoderSet::new(config.gst_launch.clone(), config.ffmpeg.clone(), policy);
    let mut speedy_games = BTreeMap::<String, u64>::new();
    let mut last_beat = 0_u64;
    loop {
        let now = now_unix_ms();
        let wanted = wanted_video();
        decoders.expire(now);
        decoders.retain_wanted(wanted.as_ref());
        let before = speedy_games.len();
        speedy_games.retain(|_, valid_until| *valid_until > now);
        // Republish when the clock turns the beat over, or when the roster
        // itself changed. A roster change recomposes the board around the same
        // spotlight, because the beat does not depend on it.
        let beat = spotlight_beat(now);
        if speedy_games.len() != before || (beat != last_beat && !speedy_games.is_empty()) {
            last_beat = beat;
            // Canvas owns the stream socket, and on a machine that starts this
            // edge first the socket does not exist yet. Propagating that error
            // tore down the run loop and every decoder with it, so the host
            // never decoded anything until Canvas happened to win the race.
            // A roster nobody can receive yet is a reason to try again later,
            // not a reason to stop decoding video.
            if let Err(error) = publish_speedy_roster(&config.bus.producer_id, beat, &speedy_games)
            {
                eprintln!("canvas-media-edge: roster not published yet: {error:#}");
            }
        }
        match bus.try_read()? {
            Some((envelope, _payload)) if envelope.message.kind == CAMERA_STREAM_CONTRACT => {
                let stream: CameraStream =
                    serde_json::from_value(serde_json::Value::Object(envelope.message.body))
                        .context("decode camera.stream.v1")?;
                decoders.upsert(stream, wanted.as_ref())?;
            }
            Some((envelope, _payload)) if envelope.message.kind == VIDEO_STREAM_CONTRACT => {
                let stream: VideoStream =
                    serde_json::from_value(serde_json::Value::Object(envelope.message.body))
                        .context("decode media.video_stream.v1")?;
                stream.validate().map_err(anyhow::Error::msg)?;
                let stream_id = stream.stream_id.clone();
                let valid_until = stream.valid_until_unix_ms;
                decoders.upsert(
                    CameraStream {
                    schema_version: stream.schema_version,
                    camera_id: stream.stream_id,
                    display_name: stream.display_name,
                    zone: stream.zone,
                    transport: stream.transport,
                    transport_role: stream.transport_role,
                    endpoint: stream.endpoint,
                    codec: stream.codec,
                    width: stream.width,
                    height: stream.height,
                    frame_rate_millihz: stream.frame_rate_millihz,
                    announced_at_unix_ms: stream.announced_at_unix_ms,
                    valid_until_unix_ms: stream.valid_until_unix_ms,
                    },
                    wanted.as_ref(),
                )?;
                if stream_id.starts_with("retro-") {
                    // The roster payload is the membership, and a session's
                    // re-announcement only carries a later lease. Comparing the
                    // expiry made every heartbeat look like a change, so the
                    // roster republished several times a second — and because
                    // the roster is a briefing, Canvas repainted the whole wall
                    // each time and the decision widgets visibly flickered.
                    // Departures are handled by the expiry sweep above.
                    let joined = speedy_games.insert(stream_id, valid_until).is_none();
                    if joined {
                        // A new game recomposes the board, but under the
                        // spotlight the clock already chose — joining must not
                        // advance the rotation.
                        if let Err(error) = publish_speedy_roster(
                            &config.bus.producer_id,
                            spotlight_beat(now_unix_ms()),
                            &speedy_games,
                        ) {
                            eprintln!("canvas-media-edge: roster not published yet: {error:#}");
                        }
                    }
                }
            }
            Some((envelope, _payload)) if envelope.message.kind == CAMERA_DETECTIONS_CONTRACT => {
                let detections: CameraDetections =
                    serde_json::from_value(serde_json::Value::Object(envelope.message.body))
                        .context("decode camera.detections.v1")?;
                let camera_id = detections.camera_id.clone();
                write_detections(root, &detections, now_unix_ms())?;
                eprintln!("canvas-media-edge: refreshed {camera_id} detections");
            }
            Some((envelope, _)) => {
                eprintln!("canvas-media-edge: ignoring '{}'", envelope.message.kind);
            }
            None => {}
        }
    }
}

/// How long a game holds the Speedy spotlight before the next takes it.
const SPOTLIGHT_DWELL_MS: u64 = 20_000;

/// Which game holds the spotlight, derived from the clock rather than counted.
///
/// Every display runs its own media edge, and the boards are mirrored — so a
/// per-process counter meant three machines each rotating on their own beat and
/// overwriting one another's layout through the mirror, which reads as the
/// spotlight flickering through every game at once. Deriving the beat from wall
/// time makes all three compute the same board at the same moment, so the
/// mirror has nothing to fight about and the cadence is exactly the dwell.
fn spotlight_beat(now_unix_ms: u64) -> u64 {
    now_unix_ms / SPOTLIGHT_DWELL_MS
}


/// The video sources Canvas says it is displaying.
///
/// `None` is "no preference", which is what an absent, unreadable or empty list
/// means: a Canvas that has not written one yet, or is not running, must not
/// cause every decoder to stop. Only an explicit, non-empty list narrows what
/// this edge decodes.
fn wanted_video() -> Option<BTreeSet<String>> {
    let root = canvas_core::runtime_root()?;
    let body = fs::read_to_string(root.join("state/wanted-video")).ok()?;
    let wanted: BTreeSet<String> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    (!wanted.is_empty()).then_some(wanted)
}

fn publish_speedy_roster(owner: &str, sequence: u64, games: &BTreeMap<String, u64>) -> Result<()> {
    let path = canvas_core::stream_socket_path().context("Canvas stream socket unavailable")?;
    let event = StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: owner.to_owned(),
        stream: "speedy-layout".into(),
        contract: BRIEFING_CONTRACT.into(),
        sequence,
        emitted_at_ms: now_unix_ms(),
        project_id: Some("speedy".into()),
        payload: serde_json::json!({
            "briefing": "speedy-layout",
            "beat": sequence,
            "games": games.keys().collect::<Vec<_>>()
        }),
    };
    event.validate().map_err(anyhow::Error::msg)?;
    let bytes = serde_json::to_vec(&event).context("encode Speedy roster")?;
    UnixDatagram::unbound()
        .context("open Speedy roster socket")?
        .send_to(&bytes, &path)
        .with_context(|| format!("publish Speedy roster to {}", path.display()))?;
    Ok(())
}

struct DecoderSet {
    gst_launch: PathBuf,
    ffmpeg: PathBuf,
    policy: DecodePolicy,
    workers: HashMap<String, DecoderWorker>,
}

impl DecoderSet {
    fn new(gst_launch: PathBuf, ffmpeg: PathBuf, policy: DecodePolicy) -> Self {
        Self {
            gst_launch,
            ffmpeg,
            policy,
            workers: HashMap::new(),
        }
    }

    fn upsert(&mut self, stream: CameraStream, wanted: Option<&BTreeSet<String>>) -> Result<()> {
        stream.validate().map_err(anyhow::Error::msg)?;
        // Announced but not displayed: remember nothing and spawn nothing. The
        // announcement will come round again when it is wanted.
        if wanted.is_some_and(|wanted| !wanted.contains(&stream.camera_id)) {
            return Ok(());
        }
        ensure!(
            stream.valid_until_unix_ms > now_unix_ms(),
            "stream announcement is expired"
        );
        if let Some(worker) = self.workers.get(&stream.camera_id) {
            if worker.stream_identity == StreamIdentity::from(&stream)
                && !worker.thread.is_finished()
            {
                worker
                    .valid_until
                    .store(stream.valid_until_unix_ms, Ordering::Release);
                return Ok(());
            }
        }
        if let Some(worker) = self.workers.remove(&stream.camera_id) {
            worker.stop();
        }
        let id = stream.camera_id.clone();
        let (program, plan) = match self.policy.decoder {
            HardwareDecoder::VideoToolbox | HardwareDecoder::V4l2M2m => (
                self.gst_launch.clone(),
                gstreamer_raw_plan(&stream, self.policy)?,
            ),
        };
        let worker = DecoderWorker::spawn(stream, program, plan)?;
        self.workers.insert(id.clone(), worker);
        eprintln!("canvas-media-edge: started native camera source '{id}'");
        Ok(())
    }

    /// Stop decoding anything the display is not asking for.
    ///
    /// `None` means no preference — an absent or unreadable list must keep the
    /// wall fed, never blank it.
    fn retain_wanted(&mut self, wanted: Option<&BTreeSet<String>>) {
        let Some(wanted) = wanted else { return };
        let unwanted: Vec<_> = self
            .workers
            .keys()
            .filter(|id| !wanted.contains(*id))
            .cloned()
            .collect();
        for id in unwanted {
            if let Some(worker) = self.workers.remove(&id) {
                worker.stop();
            }
            eprintln!("canvas-media-edge: stopped '{id}', nothing on screen shows it");
        }
    }

    fn expire(&mut self, now: u64) {
        let expired: Vec<_> = self
            .workers
            .iter()
            .filter(|(_, worker)| worker.valid_until.load(Ordering::Acquire) <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(worker) = self.workers.remove(&id) {
                worker.stop();
            }
            eprintln!("canvas-media-edge: expired camera source '{id}'");
        }
    }
}

impl Drop for DecoderSet {
    fn drop(&mut self) {
        for (_, worker) in self.workers.drain() {
            worker.stop();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamIdentity {
    endpoint: String,
    transport: unibus_media::VideoTransport,
    codec: unibus_media::VideoCodec,
}

impl From<&CameraStream> for StreamIdentity {
    fn from(value: &CameraStream) -> Self {
        Self {
            endpoint: value.endpoint.clone(),
            transport: value.transport.clone(),
            codec: value.codec.clone(),
        }
    }
}

struct DecoderWorker {
    stream_identity: StreamIdentity,
    valid_until: Arc<AtomicU64>,
    stop_requested: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: thread::JoinHandle<()>,
}

impl DecoderWorker {
    fn spawn(
        stream: CameraStream,
        program: PathBuf,
        plan: canvas_media_edge::RawVideoPlan,
    ) -> Result<Self> {
        let frame_bytes = plan.frame_bytes()?;
        let mut process = Command::new(&program)
            .args(&plan.argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn hardware decoder {}", program.display()))?;
        let stdout = process
            .stdout
            .take()
            .context("decoder stdout is unavailable")?;
        let child = Arc::new(Mutex::new(Some(process)));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let valid_until = Arc::new(AtomicU64::new(stream.valid_until_unix_ms));
        let id = stream.camera_id.clone();
        let stop_for_thread = stop_requested.clone();
        let child_for_thread = child.clone();
        let valid_for_thread = valid_until.clone();
        let thread = thread::Builder::new()
            .name(format!("media-{id}"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut allocation = vec![0_u8; plan.allocation_bytes];
                while !stop_for_thread.load(Ordering::Acquire)
                    && valid_for_thread.load(Ordering::Acquire) > now_unix_ms()
                {
                    match reader.read_exact(&mut allocation) {
                        Ok(()) => {
                            let result = plan.visible_frame(&allocation).and_then(|frame| {
                                write_raw_frame(&id, frame_bytes, frame).map(|_| ())
                            });
                            if let Err(error) = result {
                                eprintln!("canvas-media-edge: {id} frame rejected: {error:#}");
                            }
                        }
                        Err(error) => {
                            eprintln!("canvas-media-edge: {id} decoder read failed: {error}");
                            break;
                        }
                    }
                }
                if let Ok(mut guard) = child_for_thread.lock() {
                    if let Some(mut process) = guard.take() {
                        let _ = process.kill();
                        match process.wait() {
                            Ok(status)
                                if !status.success()
                                    && !stop_for_thread.load(Ordering::Acquire) =>
                            {
                                eprintln!(
                                    "canvas-media-edge: {id} hardware decoder exited {status}"
                                )
                            }
                            Err(error) => {
                                eprintln!("canvas-media-edge: {id} decoder wait failed: {error}")
                            }
                            _ => {}
                        }
                    }
                }
            })
            .context("start decoder reader")?;
        Ok(Self {
            stream_identity: StreamIdentity::from(&stream),
            valid_until,
            stop_requested,
            child,
            thread,
        })
    }

    fn stop(self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(process) = guard.as_mut() {
                let _ = process.kill();
            }
        }
        let _ = self.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_refuses_unknown_and_software_decoder() {
        let raw = r#"{
          "bus":{"installation_id":"home","producer_id":"media","hub_endpoint":"127.0.0.1:9000","token_file":"token","zones":["home"]},
          "hardware_decoder":"software"
        }"#;
        let config: EdgeConfig = serde_json::from_str(raw).unwrap();
        assert!(config.policy().is_err());
        assert!(
            serde_json::from_str::<EdgeConfig>(
                &raw.replace("\n        }", ",\"surprise\":true\n        }")
            )
            .is_err()
        );
    }
}
#[cfg(test)]
mod wanted_video_tests {
    use super::*;

    fn parse(body: &str) -> Option<BTreeSet<String>> {
        let wanted: BTreeSet<String> = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        (!wanted.is_empty()).then_some(wanted)
    }

    /// The rule the wall depends on: no list means decode everything. A reader
    /// that treated "I cannot find it" as "show nothing" would blank every
    /// display the moment Canvas restarted.
    #[test]
    fn an_empty_or_blank_list_is_no_preference() {
        assert!(parse("").is_none());
        assert!(parse("\n\n   \n").is_none());
    }

    #[test]
    fn a_list_narrows_to_exactly_what_it_names() {
        let wanted = parse("retro-smb\nretro-smk\n").expect("a real list");
        assert!(wanted.contains("retro-smb"));
        assert!(wanted.contains("retro-smk"));
        assert!(!wanted.contains("cat"), "an off-screen camera is not wanted");
        assert_eq!(wanted.len(), 2);
    }

    #[test]
    fn surrounding_whitespace_does_not_hide_a_source() {
        let wanted = parse("  retro-smb  \n\tcat\t\n").expect("a real list");
        assert!(wanted.contains("retro-smb"));
        assert!(wanted.contains("cat"));
    }
}
