//! Authenticated Unibus status -> owner-local Canvas stream. No vendor protocol.
use anyhow::{Context, Result, ensure};
use canvas::stream_binding::{STREAM_SCHEMA_VERSION, StreamEnvelope};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::SocketAddr, os::unix::net::UnixDatagram};
use unibus_device::printer::{PrinterStatus, STATUS_KIND};
use unibus_sdk::{Event, Session};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: serde_json::Value,
    /// Explicit enrollment: device ID -> authenticated producer ID.
    printers: BTreeMap<String, String>,
    project_id: String,
}
fn projection(config: &Config, event: &Event, now: u64, sequence: u64) -> Result<StreamEnvelope> {
    ensure!(event.kind == STATUS_KIND, "not a printer status");
    let status: PrinterStatus = serde_json::from_value(event.body.clone())?;
    status.validate(now).map_err(anyhow::Error::msg)?;
    ensure!(
        config.printers.get(&status.device_id) == Some(&event.producer_id)
            && status.observer == event.producer_id,
        "printer source is not enrolled"
    );
    ensure!(
        status.device_id == format!("bambu:{}", status.serial),
        "printer identity mismatch"
    );
    Ok(StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: "bambu".into(),
        stream: format!("printer-{:x}", Sha256::digest(status.device_id.as_bytes()))[..40]
            .to_owned(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence,
        emitted_at_ms: status.observed_at_unix_ms,
        project_id: Some(config.project_id.clone()),
        payload: serde_json::json!({"printer": status}),
    })
}
fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .context("usage: canvas-printer-observer <config.json>")?;
    let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        config.bus["schema_version"].as_u64() == Some(2)
            && config.bus["hub_endpoint"]
                .as_str()
                .context("missing hub endpoint")?
                .parse::<SocketAddr>()?
                .ip()
                .is_loopback(),
        "observer requires v2 and a local hub or authenticated loopback tunnel"
    );
    ensure!(
        !config.printers.is_empty() && config.printers.len() <= 64,
        "enroll 1–64 printers"
    );
    let bus = Session::connect(serde_json::from_value(config.bus.clone())?)?;
    let socket = UnixDatagram::unbound()?;
    let path = canvas_core::stream_socket_path().context("Canvas socket unavailable")?;
    let mut sequence = 0u64;
    loop {
        ensure!(bus.status().connected, "printer observer bus disconnected");
        let Some(event) = bus.poll() else {
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        };
        if event.kind != STATUS_KIND {
            continue;
        }
        sequence = sequence
            .checked_add(1)
            .context("observer sequence exhausted")?;
        let frame = match projection(&config, &event, unibus::envelope::now_ms(), sequence) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!("printer status rejected: {error}");
                continue;
            }
        };
        frame.validate().map_err(anyhow::Error::msg)?;
        let bytes = serde_json::to_vec(&frame)?;
        ensure!(
            bytes.len() <= 2048,
            "printer projection exceeds portable datagram budget"
        );
        socket
            .send_to(&bytes, &path)
            .context("deliver printer status to Canvas")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_identity_and_freshness_are_enforced() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "bus":{"installation_id":"lab","producer_id":"canvas-printers","hub_endpoint":"127.0.0.1:1","token_env":"TEST_TOKEN","zones":["lab"],"schema_version":2},
            "project_id":"printers","printers":{"bambu:SERIAL":"printer-one"}
        })).unwrap();
        let mut event = Event {
            producer_id: "printer-one".into(),
            request_id: "test".into(),
            issued_at_unix_ms: 10000,
            kind: STATUS_KIND.into(),
            body: serde_json::json!({
                "schema_version":1,"device_id":"bambu:SERIAL","serial":"SERIAL","observer":"printer-one",
                "revision":1,"observed_at_unix_ms":10000,"valid_until_unix_ms":16000,"last_report_at_unix_ms":10000,
                "link":"live","read_only":true,"telemetry":{"nozzle_c":{"value":210.0,"observed_at_unix_ms":10000}}
            }),
        };
        let frame = projection(&config, &event, 10000, 1).unwrap();
        frame.validate().unwrap();
        assert!(projection(&config, &event, 16000, 1).is_err());
        event.producer_id = "imposter".into();
        assert!(projection(&config, &event, 10000, 1).is_err());
    }
}
