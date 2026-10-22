use std::io::Read;
use std::os::unix::net::UnixDatagram;

use anyhow::{Context, Result};
use canvas::stream_binding::{STREAM_SCHEMA_VERSION, StreamEnvelope};

const CONTRACT: &str = "lob.pipeline.status.v1";
const STREAMS: [&str; 4] = ["lob-fan", "lob-brief", "lob-pipeline", "lob-confidence"];

fn main() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let payload: serde_json::Value = serde_json::from_str(&input).context("parse lob status")?;
    validate(&payload)?;
    let sequence = payload["updated_epoch_ms"].as_u64().unwrap_or(1).max(1);
    let path = canvas_core::stream_socket_path().context("Canvas stream socket unavailable")?;
    let socket = UnixDatagram::unbound()?;
    for stream in STREAMS {
        let envelope = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "legion-of-bom".into(),
            stream: stream.into(),
            contract: CONTRACT.into(),
            sequence,
            emitted_at_ms: sequence,
            project_id: Some("lob".into()),
            payload: payload.clone(),
        };
        let bytes = serde_json::to_vec(&envelope)?;
        anyhow::ensure!(
            bytes.len() <= 2048,
            "{stream} status is {} bytes",
            bytes.len()
        );
        socket.send_to(&bytes, &path)?;
    }
    Ok(())
}

fn validate(value: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(
        value["schema_version"].as_u64() == Some(1),
        "unsupported lob status schema"
    );
    anyhow::ensure!(value["circuit"].is_string(), "lob status has no circuit");
    anyhow::ensure!(value["stages"].is_array(), "lob status has no stages");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_the_lob_facts_contract() {
        assert!(
            validate(&serde_json::json!({"schema_version":1,"circuit":"filter","stages":[]}))
                .is_ok()
        );
        assert!(validate(&serde_json::json!({"schema_version":1,"circuit":"filter"})).is_err());
    }
}
