use std::{
    net::UdpSocket,
    os::unix::net::UnixDatagram,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use canvas::stream_binding::{STREAM_SCHEMA_VERSION, StreamEnvelope};

fn main() -> Result<()> {
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:50070".into());
    let ingress =
        UdpSocket::bind(&bind).with_context(|| format!("bind Radioman ingress {bind}"))?;
    let canvas_path =
        canvas_core::stream_socket_path().context("Canvas stream socket unavailable")?;
    let output = UnixDatagram::unbound().context("open Canvas stream datagram")?;
    let mut bytes = [0_u8; 8192];
    eprintln!(
        "canvas-radioman-observer: {bind} -> {}",
        canvas_path.display()
    );
    loop {
        let (size, peer) = ingress
            .recv_from(&mut bytes)
            .context("receive Radioman frame")?;
        let payload: serde_json::Value = match serde_json::from_slice(&bytes[..size]) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("canvas-radioman-observer: rejected {peer}: {error}");
                continue;
            }
        };
        let sequence = payload
            .get("sequence")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let emitted_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let envelope = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "radioman".into(),
            stream: "spectrum".into(),
            contract: "radioman.spectrum.v1".into(),
            sequence,
            emitted_at_ms,
            project_id: Some("home".into()),
            payload,
        };
        if let Err(error) = envelope.validate() {
            eprintln!("canvas-radioman-observer: rejected {peer}: {error}");
            continue;
        }
        let encoded = serde_json::to_vec(&envelope).context("encode Canvas envelope")?;
        if encoded.len() > 2048 {
            eprintln!(
                "canvas-radioman-observer: frame is {} bytes; rejecting",
                encoded.len()
            );
            continue;
        }
        if let Err(error) = output.send_to(&encoded, &canvas_path) {
            eprintln!("canvas-radioman-observer: Canvas delivery failed: {error}");
        }
    }
}
