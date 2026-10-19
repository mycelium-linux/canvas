use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_core::unibus::Message;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use unibus_media::{
    MEDIA_SCHEMA_VERSION, TransportRole, VIDEO_STREAM_CONTRACT, VideoCodec, VideoStream,
    VideoTransport,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: BusConfig,
    destination: String,
    adb: PathBuf,
    ffmpeg: PathBuf,
    serial: String,
    stream_id: String,
    display_name: String,
    stream_endpoint: String,
    advertised_endpoint: String,
    width: u32,
    height: u32,
    fps: u32,
    #[serde(default = "default_ttl_ms")]
    ttl_ms: u64,
}

const fn default_ttl_ms() -> u64 {
    30_000
}

impl Config {
    fn read(path: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            config.adb.is_file(),
            "adb not found: {}",
            config.adb.display()
        );
        ensure!(
            config.ffmpeg.is_file(),
            "ffmpeg not found: {}",
            config.ffmpeg.display()
        );
        ensure!(!config.serial.is_empty(), "ADB serial is required");
        ensure!((1..=60).contains(&config.fps), "fps must be 1..=60");
        ensure!(config.width > 0 && config.height > 0, "invalid dimensions");
        ensure!((5_000..=300_000).contains(&config.ttl_ms), "invalid ttl_ms");
        Ok(config)
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-android-stream <config.json>")?;
    let config = Config::read(Path::new(&path))?;
    ensure_device(&config)?;
    let bus = Bus::connect(config.bus.clone())?;
    let (mut adb, mut encoder) = spawn_pipeline(&config)?;
    let mut next_announcement = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_announcement {
            announce(&bus, &config)?;
            next_announcement = now + Duration::from_millis(config.ttl_ms / 2);
        }
        if let Some(status) = adb.try_wait()? {
            bail!("adb screenrecord exited {status}");
        }
        if let Some(status) = encoder.try_wait()? {
            bail!("ffmpeg encoder exited {status}");
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn ensure_device(config: &Config) -> Result<()> {
    let output = Command::new(&config.adb)
        .args(["-s", &config.serial, "get-state"])
        .output()
        .context("query Android device")?;
    ensure!(
        output.status.success() && output.stdout.starts_with(b"device"),
        "Android device {} is unavailable or unauthorized",
        config.serial
    );
    Ok(())
}

fn spawn_pipeline(config: &Config) -> Result<(Child, Child)> {
    let size = format!("{}x{}", config.width, config.height);
    let mut adb = Command::new(&config.adb)
        .args([
            "-s",
            &config.serial,
            "exec-out",
            "screenrecord",
            "--output-format=h264",
            "--size",
            &size,
            "--bit-rate",
            "4M",
            "--time-limit",
            "175",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start adb screenrecord")?;
    let input = adb.stdout.take().context("ADB video stdout unavailable")?;
    let encoder = Command::new(&config.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "h264",
            "-framerate",
            &config.fps.to_string(),
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "copy",
            "-f",
            "mpegts",
            &config.stream_endpoint,
        ])
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start Android MPEG-TS muxer")?;
    Ok((adb, encoder))
}

fn announce(bus: &Bus, config: &Config) -> Result<()> {
    let now = canvas_media_edge::now_unix_ms();
    let stream = VideoStream {
        schema_version: MEDIA_SCHEMA_VERSION,
        stream_id: config.stream_id.clone(),
        display_name: config.display_name.clone(),
        zone: "living-room".into(),
        transport: VideoTransport::UdpMpegTs,
        transport_role: TransportRole::Listen,
        endpoint: config.advertised_endpoint.clone(),
        codec: VideoCodec::H264,
        width: config.width,
        height: config.height,
        frame_rate_millihz: config.fps * 1_000,
        announced_at_unix_ms: now,
        valid_until_unix_ms: now + config.ttl_ms,
    };
    stream.validate().map_err(anyhow::Error::msg)?;
    let message = Message::new(VIDEO_STREAM_CONTRACT, serde_json::to_value(stream)?);
    let envelope = bus.to_destination(&config.destination, message)?;
    bus.send(&envelope, &[]).context("announce Android stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_default_is_bounded() {
        assert!((5_000..=300_000).contains(&default_ttl_ms()));
    }
}
