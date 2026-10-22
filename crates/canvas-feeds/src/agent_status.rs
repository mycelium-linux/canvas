//! Read-only owner telemetry: never infer waiting from terminal/process idleness.
use serde_json::{Value, json};
use std::{collections::BTreeSet, os::unix::net::UnixStream, path::Path, time::Duration};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub active: usize,
    pub idle: usize,
    pub input: usize,
    pub approval: usize,
    pub unknown: usize,
}

pub fn count(statuses: &[Value]) -> Counts {
    let mut counts = Counts::default();
    for status in statuses {
        match status["type"].as_str() {
            Some("active") => {
                let Some(flags) = status["activeFlags"].as_array() else {
                    counts.unknown += 1; continue;
                };
                let input = flags.iter().any(|f| f == "waitingOnUserInput");
                let approval = flags.iter().any(|f| f == "waitingOnApproval");
                counts.input += usize::from(input);
                counts.approval += usize::from(approval);
                if flags.is_empty() { counts.active += 1; }
                else if !input && !approval { counts.unknown += 1; }
            }
            Some("idle") => counts.idle += 1,
            Some("notLoaded") => {}, // Unloaded history is not a running agent.
            _ => counts.unknown += 1,
        }
    }
    counts
}

pub fn observe(socket: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut config = tungstenite::protocol::WebSocketConfig::default();
    config.max_message_size = Some(4 * 1024 * 1024);
    config.max_frame_size = Some(4 * 1024 * 1024);
    let (mut ws, _) = tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let rpc = |ws: &mut tungstenite::WebSocket<UnixStream>, id: u64, method: &str, params: Value|
        -> Result<Value, Box<dyn std::error::Error>> {
        if std::time::Instant::now() >= deadline { return Err("Owner observation deadline exceeded".into()); }
        ws.send(tungstenite::Message::Text(json!({"id":id,"method":method,"params":params}).to_string().into()))?;
        for _ in 0..128 {
            if let tungstenite::Message::Text(text) = ws.read()? {
                let value: Value = serde_json::from_str(&text)?;
                if value["id"] == id {
                    if value.get("error").is_some() { return Err(format!("Owner rejected {method}").into()); }
                    return value.get("result").cloned().ok_or("Missing owner result".into());
                }
            }
        }
        Err("Owner reply budget exceeded".into())
    };
    rpc(&mut ws, 1, "initialize", json!({"clientInfo":{"name":"canvas-agent-status","version":"0.1.0"}}))?;
    ws.send(tungstenite::Message::Text(json!({"method":"initialized","params":{}}).to_string().into()))?;
    let loaded = rpc(&mut ws, 2, "thread/loaded/list", json!({}))?;
    let ids = loaded["data"].as_array().ok_or("Missing loaded thread IDs")?;
    if ids.len() > 512 { return Err("Too many loaded threads; observation is not complete".into()); }
    let mut unique = BTreeSet::new();
    let mut statuses = Vec::new();
    for id in ids {
        let id = id.as_str().ok_or("Invalid thread ID")?;
        if !unique.insert(id) { continue; }
        let result = rpc(&mut ws, 3 + statuses.len() as u64, "thread/read", json!({"threadId":id,"includeTurns":false}))?;
        statuses.push(result["thread"]["status"].clone());
    }
    let counts = count(&statuses);
    let row = |label: &str, value: usize, state: &str| json!({"label":label,"detail":value.to_string(),"state":state});
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let observed = chrono::DateTime::from_timestamp_millis(now_ms).ok_or("Invalid clock")?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Ok(json!({
        "widget":{"kind":"status_list","title":"Codex · Neo", "nodes":[
            row("Active", counts.active, "good"),
            row("Needs response", counts.input, if counts.input > 0 {"warning"} else {"neutral"}),
            row("Needs approval", counts.approval, if counts.approval > 0 {"warning"} else {"neutral"}),
            row("Idle", counts.idle, "neutral"),
            row("Unknown", counts.unknown, if counts.unknown > 0 {"warning"} else {"neutral"})
        ]},
        "data":{"state":if counts.unknown > 0 {"partial"} else {"ready"},"provenance":"live",
            "source":"codex:local-app-server","observed_at":observed,"detail":"Local app-server only"}
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waiting_is_explicit_and_never_guessed() {
        let counts = count(&[json!({"type":"active","activeFlags":[]}),
            json!({"type":"active","activeFlags":["waitingOnUserInput"]}),
            json!({"type":"active","activeFlags":["waitingOnApproval"]}),
            json!({"type":"idle"}), json!({"type":"notLoaded"})]);
        assert_eq!(counts, Counts { active:1, input:1, approval:1, idle:1, unknown:0 });
    }
    #[test]
    fn missing_or_new_flags_are_not_running() {
        assert_eq!(count(&[json!({"type":"active"}),
            json!({"type":"active","activeFlags":["futureFlag"]})]).unknown, 2);
    }
}
