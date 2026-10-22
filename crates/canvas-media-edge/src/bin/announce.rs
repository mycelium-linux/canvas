use anyhow::{Context, Result, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_core::unibus::Message;
use serde::Deserialize;
use std::time::Duration;
use unibus_media::{CAMERA_STREAM_CONTRACT, CameraStream};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: BusConfig,
    destination: String,
    ttl_ms: u64,
    streams: Vec<CameraStream>,
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-media-announce <config.json>")?;
    let bytes = std::fs::read(&path).with_context(|| format!("read {path}"))?;
    let mut config: Config = serde_json::from_slice(&bytes).context("parse announcement config")?;
    ensure!(
        (5_000..=300_000).contains(&config.ttl_ms),
        "ttl_ms must be 5000..=300000"
    );
    ensure!(!config.destination.is_empty(), "destination is required");
    ensure!(
        !config.streams.is_empty() && config.streams.len() <= 16,
        "streams must contain 1..=16 entries"
    );
    let bus = Bus::connect(config.bus.clone())?;
    loop {
        let now = canvas_media_edge::now_unix_ms();
        for stream in &mut config.streams {
            stream.announced_at_unix_ms = now;
            stream.valid_until_unix_ms = now + config.ttl_ms;
            stream.validate().map_err(anyhow::Error::msg)?;
            let body = serde_json::to_value(&*stream).context("encode stream")?;
            let envelope = bus.to_destination(
                &config.destination,
                Message::new(CAMERA_STREAM_CONTRACT, body),
            )?;
            bus.send(&envelope, &[])?;
        }
        std::thread::sleep(Duration::from_millis(config.ttl_ms / 2));
    }
}
