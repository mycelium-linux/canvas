//! Headless Unibus-to-media-plane adapter for native Canvas camera surfaces.

use anyhow::{Context, Result, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_media_edge::{
    DecodePolicy, HardwareDecoder, ffmpeg_raw_plan, gstreamer_raw_plan, now_unix_ms,
    write_detections, write_raw_frame,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{BufReader, Read},
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
    loop {
        decoders.expire(now_unix_ms());
        match bus.try_read()? {
            Some((envelope, _payload)) if envelope.message.kind == CAMERA_STREAM_CONTRACT => {
                let stream: CameraStream =
                    serde_json::from_value(serde_json::Value::Object(envelope.message.body))
                        .context("decode camera.stream.v1")?;
                decoders.upsert(stream)?;
            }
            Some((envelope, _payload)) if envelope.message.kind == VIDEO_STREAM_CONTRACT => {
                let stream: VideoStream =
                    serde_json::from_value(serde_json::Value::Object(envelope.message.body))
                        .context("decode media.video_stream.v1")?;
                stream.validate().map_err(anyhow::Error::msg)?;
                decoders.upsert(CameraStream {
                    schema_version: stream.schema_version,
                    camera_id: stream.stream_id,
                    display_name: stream.display_name,
                    zone: stream.zone,
                    transport: stream.transport,
                    endpoint: stream.endpoint,
                    codec: stream.codec,
                    width: stream.width,
                    height: stream.height,
                    frame_rate_millihz: stream.frame_rate_millihz,
                    announced_at_unix_ms: stream.announced_at_unix_ms,
                    valid_until_unix_ms: stream.valid_until_unix_ms,
                })?;
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

    fn upsert(&mut self, stream: CameraStream) -> Result<()> {
        stream.validate().map_err(anyhow::Error::msg)?;
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
            HardwareDecoder::VideoToolbox => {
                (self.ffmpeg.clone(), ffmpeg_raw_plan(&stream, self.policy)?)
            }
            HardwareDecoder::V4l2M2m => (
                self.gst_launch.clone(),
                gstreamer_raw_plan(&stream, self.policy)?,
            ),
        };
        let worker = DecoderWorker::spawn(stream, program, plan)?;
        self.workers.insert(id.clone(), worker);
        eprintln!("canvas-media-edge: started native camera source '{id}'");
        Ok(())
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
