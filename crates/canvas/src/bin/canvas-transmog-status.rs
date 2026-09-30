use std::io::Read;
use std::os::unix::net::UnixDatagram;

use anyhow::{Context, Result};
use canvas::stream_binding::{STREAM_SCHEMA_VERSION, StreamEnvelope};

const STREAMS: [&str; 5] = [
    "transmog-fan",
    "transmog-brief",
    "transmog-steps",
    "transmog-confidence",
    "transmog-timeline",
];

fn main() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let status: serde_json::Value =
        serde_json::from_str(&input).context("parse Transmog status")?;
    let payload = transport_payload(&status)?;
    // Publication time, not artifact mtime, is the stream sequence. Replaying
    // an unchanged status after Canvas restarts or a workspace is opened must
    // still be a newer delivery than the last publication.
    let sequence = canvas::stream_binding::now_ms().max(1);

    let path = canvas_core::stream_socket_path().context("Canvas stream socket unavailable")?;
    let socket = UnixDatagram::unbound()?;
    for stream in STREAMS {
        let envelope = StreamEnvelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: "mac-studio-transmog".into(),
            stream: stream.into(),
            contract: "transmog.build.status.v1".into(),
            sequence,
            emitted_at_ms: sequence,
            project_id: Some("transmog".into()),
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

/// Keep the producer concerned with facts and transport only. Canvas' Lua
/// binder owns how those facts become cards, graphs, labels, and colors.
fn transport_payload(status: &serde_json::Value) -> Result<serde_json::Value> {
    let steps = status["steps"].as_array().context("status has no steps")?;
    // Keep transport bounded by copying facts the binders actually consume,
    // never the potentially large per-step mesh payload.
    let recent = steps
        .iter()
        .rev()
        .take(12)
        .map(|step| {
            let mut compact = serde_json::Map::new();
            for key in [
                "index",
                "feature",
                "kind",
                "operation",
                "subtype",
                "component",
                "csg",
                "slow",
                "ms",
                "tris",
            ] {
                if let Some(value) = step.get(key).filter(|value| !value.is_null()) {
                    compact.insert(key.into(), value.clone());
                }
            }
            serde_json::Value::Object(compact)
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "total": status["total"].as_u64().unwrap_or(steps.len() as u64),
        "done": status["done"].as_u64().unwrap_or(steps.len() as u64),
        "complete": status["complete"].as_bool().unwrap_or(false),
        "source": status["source"].as_str().unwrap_or("transmog build"),
        "material": "PETG",
        "material_source": "authored",
        "process": "FDM",
        "stream_id": "transmog-step",
        "steps": recent,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_payload_contains_facts_not_widget_presentation() {
        let status = serde_json::json!({
            "total": 2,
            "done": 1,
            "complete": false,
            "source": "draft.ron",
            "steps": [{"index": 1, "kind": "union", "feature": 3}]
        });
        let payload = transport_payload(&status).expect("payload");
        assert_eq!(payload["material"], "PETG");
        assert!(payload.get("widget").is_none());
        assert!(payload.get("title").is_none());
    }
}
