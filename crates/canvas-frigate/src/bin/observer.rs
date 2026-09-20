use anyhow::{Context, Result, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_core::unibus::Message;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, time::Duration};
use unibus_media::{CAMERA_DETECTIONS_CONTRACT, CameraDetections, Detection, NormalizedBox};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: BusConfig,
    api_base: String,
    destination: String,
    cameras: Vec<String>,
    poll_ms: u64,
    ttl_ms: u64,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    camera: String,
    label: String,
    data: EventData,
}

#[derive(Deserialize)]
struct EventData {
    r#box: [f32; 4],
    score: f32,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-frigate-observer <config.json>")?;
    let config: Config = serde_json::from_slice(
        &std::fs::read(Path::new(&path)).with_context(|| format!("read {path}"))?,
    )
    .with_context(|| format!("parse {path}"))?;
    ensure!(
        !config.cameras.is_empty() && config.cameras.len() <= 16,
        "cameras must contain 1..=16 ids"
    );
    ensure!(
        (100..=5_000).contains(&config.poll_ms),
        "poll_ms must be 100..=5000"
    );
    ensure!(
        (500..=30_000).contains(&config.ttl_ms),
        "ttl_ms must be 500..=30000"
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let bus = Bus::connect(config.bus.clone())?;
    let endpoint = format!(
        "{}/api/events?limit=100&in_progress=1",
        config.api_base.trim_end_matches('/')
    );
    loop {
        let events = client
            .get(&endpoint)
            .send()?
            .error_for_status()?
            .json::<Vec<Event>>()?;
        let mut grouped: BTreeMap<&str, Vec<Detection>> = config
            .cameras
            .iter()
            .map(|camera| (camera.as_str(), Vec::new()))
            .collect();
        for event in events {
            let Some(detections) = grouped.get_mut(event.camera.as_str()) else {
                continue;
            };
            let [x, y, width, height] = event.data.r#box;
            detections.push(Detection {
                tracking_id: event.id,
                label: event.label,
                confidence: event.data.score.clamp(0.0, 1.0),
                bounds: NormalizedBox {
                    x,
                    y,
                    width,
                    height,
                },
            });
        }
        let observed_at_unix_ms = now_ms();
        for (camera_id, detections) in grouped {
            let value = CameraDetections {
                schema_version: 1,
                camera_id: camera_id.to_owned(),
                observed_at_unix_ms,
                valid_until_unix_ms: observed_at_unix_ms + config.ttl_ms,
                detections,
            };
            value.validate().map_err(anyhow::Error::msg)?;
            let envelope = bus.to_destination(
                &config.destination,
                Message::new(CAMERA_DETECTIONS_CONTRACT, serde_json::to_value(value)?),
            )?;
            bus.send(&envelope, &[])?;
        }
        std::thread::sleep(Duration::from_millis(config.poll_ms));
    }
}
