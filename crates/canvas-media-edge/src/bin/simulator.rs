use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_core::unibus::Message;
use serde::Deserialize;
use std::{
    io::Write,
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
    xcrun: PathBuf,
    ffmpeg: PathBuf,
    stream_endpoint: String,
    advertised_endpoint: String,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default = "default_fps")]
    fps: u32,
    #[serde(default = "default_ttl_ms")]
    ttl_ms: u64,
}

fn default_width() -> u32 {
    720
}
fn default_height() -> u32 {
    1280
}
fn default_fps() -> u32 {
    12
}
fn default_ttl_ms() -> u64 {
    15_000
}

impl Config {
    fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let config: Self =
            serde_json::from_slice(&bytes).context("parse simulator stream config")?;
        ensure!(
            config.xcrun.is_file(),
            "xcrun not found: {}",
            config.xcrun.display()
        );
        ensure!(
            config.ffmpeg.is_file(),
            "ffmpeg not found: {}",
            config.ffmpeg.display()
        );
        ensure!((1..=30).contains(&config.fps), "fps must be 1..=30");
        ensure!(
            (320..=2160).contains(&config.width) && (480..=3840).contains(&config.height),
            "invalid dimensions"
        );
        ensure!(
            (5_000..=300_000).contains(&config.ttl_ms),
            "ttl_ms must be 5000..=300000"
        );
        ensure!(
            config.stream_endpoint.starts_with("udp://")
                && config.advertised_endpoint.starts_with("udp://"),
            "simulator stream must use UDP MPEG-TS"
        );
        ensure!(!config.destination.is_empty(), "destination is required");
        Ok(config)
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-simulator-stream <config.json>")?;
    let config = Config::read(Path::new(&path))?;
    ensure_booted(&config.xcrun)?;
    let bus = Bus::connect(config.bus.clone())?;
    let mut encoder = spawn_encoder(&config)?;
    let mut input = encoder.stdin.take().context("ffmpeg stdin unavailable")?;
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let mut next_announcement = Instant::now();

    loop {
        let started = Instant::now();
        if started >= next_announcement {
            announce(&bus, &config)?;
            next_announcement = started + Duration::from_millis(config.ttl_ms / 2);
        }
        let png = screenshot(&config.xcrun)?;
        input
            .write_all(&png)
            .context("write Simulator frame to ffmpeg")?;
        thread::sleep(interval.saturating_sub(started.elapsed()));
        if encoder.try_wait()?.is_some() {
            bail!("ffmpeg encoder exited");
        }
    }
}

fn ensure_booted(xcrun: &Path) -> Result<()> {
    let output = Command::new(xcrun)
        .args(["simctl", "list", "devices", "booted", "-j"])
        .output()
        .context("query booted Simulator")?;
    ensure!(output.status.success(), "simctl device query failed");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("decode simctl device list")?;
    let booted = value
        .get("devices")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|runtimes| {
            runtimes.values().any(|devices| {
                devices
                    .as_array()
                    .is_some_and(|devices| !devices.is_empty())
            })
        });
    ensure!(
        booted,
        "no booted iOS Simulator; open Simulator and boot a device"
    );
    Ok(())
}

fn screenshot(xcrun: &Path) -> Result<Vec<u8>> {
    // Current simctl releases treat `-` as a literal path instead of stdout.
    // Reuse one process-scoped file so capture remains bounded and cleanup is
    // deterministic even at the configured frame rate.
    let frame_path = std::env::temp_dir().join(format!(
        "canvas-simulator-stream-{}.png",
        std::process::id()
    ));
    let output = Command::new(xcrun)
        .args(["simctl", "io", "booted", "screenshot", "--type=png"])
        .arg(&frame_path)
        .output()
        .context("capture Simulator")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&frame_path);
        bail!(
            "Simulator screenshot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let png = std::fs::read(&frame_path)
        .with_context(|| format!("read Simulator frame {}", frame_path.display()));
    let _ = std::fs::remove_file(&frame_path);
    let png = png?;
    ensure!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Simulator returned a non-PNG frame"
    );
    Ok(png)
}

fn spawn_encoder(config: &Config) -> Result<Child> {
    let scale = format!(
        "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2",
        config.width, config.height, config.width, config.height
    );
    Command::new(&config.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "image2pipe",
            "-framerate",
            &config.fps.to_string(),
            "-vcodec",
            "png",
            "-i",
            "pipe:0",
            "-an",
            "-vf",
            &scale,
            "-c:v",
            "h264_videotoolbox",
            "-realtime",
            "1",
            "-b:v",
            "3M",
            "-g",
            &config.fps.to_string(),
            "-bf",
            "0",
            "-f",
            "mpegts",
            &config.stream_endpoint,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {}", config.ffmpeg.display()))
}

fn announce(bus: &Bus, config: &Config) -> Result<()> {
    let now = canvas_media_edge::now_unix_ms();
    let stream = VideoStream {
        schema_version: MEDIA_SCHEMA_VERSION,
        stream_id: "simulator-ios".into(),
        display_name: "iOS Simulator".into(),
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
    let body = serde_json::to_value(stream)?;
    let envelope = bus.to_destination(
        &config.destination,
        Message::new(VIDEO_STREAM_CONTRACT, body),
    )?;
    bus.send(&envelope, &[])
        .context("announce Simulator stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_tv_safe_and_bounded() {
        assert_eq!(
            (default_width(), default_height(), default_fps()),
            (720, 1280, 12)
        );
        assert!(default_ttl_ms() >= 5_000);
    }
}
