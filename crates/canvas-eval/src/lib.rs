//! Eval's replayable SSE snapshots projected into Canvas's generic run-view contract.
//!
//! Eval owns benchmark semantics and redaction. This crate owns only transport,
//! bounds, reconnection, and the stable interface consumed by Canvas views.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SOURCE_CONTRACT: &str = "eval.watch.v1";
pub const CANVAS_CONTRACT: &str = "canvas.run.watch.v1";
pub const MAX_SSE_EVENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_VISIBLE_EVENTS_PER_KIND: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub schema: String,
    pub revision: u64,
    pub run: Value,
    pub state: String,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub stls: Vec<Value>,
    #[serde(default)]
    pub presentation: Value,
}

impl RunSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SOURCE_CONTRACT {
            return Err(format!("expected {SOURCE_CONTRACT}, got {}", self.schema));
        }
        if !matches!(self.state.as_str(), "running" | "complete" | "failed") {
            return Err(format!("unsupported eval run state: {}", self.state));
        }
        if !self.run.is_object() {
            return Err("eval watch run metadata must be an object".into());
        }
        Ok(())
    }

    /// A bounded snapshot suitable for Canvas's datagram ingress. The source is
    /// whole-state, so retaining the newest events is lossless for live viewing;
    /// reconnecting replaces this projection rather than replaying deltas.
    pub fn project(&self) -> Value {
        let mut counts = serde_json::Map::new();
        for event in &self.events {
            let kind = event.get("kind").and_then(Value::as_str).unwrap_or("other");
            let count = counts.get(kind).and_then(Value::as_u64).unwrap_or(0) + 1;
            counts.insert(kind.to_owned(), Value::from(count));
        }
        let mut kept_per_kind = std::collections::BTreeMap::<&str, usize>::new();
        let mut selected = self
            .events
            .iter()
            .rev()
            .filter(|event| {
                let kind = event.get("kind").and_then(Value::as_str).unwrap_or("other");
                let kept = kept_per_kind.entry(kind).or_default();
                if *kept == MAX_VISIBLE_EVENTS_PER_KIND {
                    return false;
                }
                *kept += 1;
                true
            })
            .collect::<Vec<_>>();
        selected.reverse();
        let omitted = self.events.len().saturating_sub(selected.len());
        let events = selected.into_iter().map(project_event).collect::<Vec<_>>();
        let active_artifact = events
            .iter()
            .rev()
            .filter_map(|event| event.get("artifacts")?.as_array())
            .find_map(|artifacts| artifacts.last().cloned());
        json!({
            "schema": CANVAS_CONTRACT,
            "sourceSchema": self.schema,
            "revision": self.revision,
            "run": self.run,
            "state": self.state,
            "events": events,
            "eventCounts": counts,
            "omittedEvents": omitted,
            "artifactCount": self.stls.len(),
            "activeArtifact": active_artifact,
            "presentation": self.presentation,
        })
    }
}

pub fn payload_for_stream(projected: &Value, stream: &str) -> Value {
    let mut payload = projected.clone();
    let wanted = match stream {
        "eval-decisions" => Some("decision"),
        "eval-tree" => Some("all"),
        "eval-events" => Some("build-or-assembly"),
        "eval-verification" => Some("verification"),
        _ => None,
    };
    if let Some(events) = payload.get_mut("events").and_then(Value::as_array_mut) {
        events.retain(|event| match wanted {
            Some("build-or-assembly") => matches!(
                event.get("kind").and_then(Value::as_str),
                Some("build" | "assembly")
            ),
            Some("all") => true,
            Some(kind) => event.get("kind").and_then(Value::as_str) == Some(kind),
            None => false,
        });
    }
    if stream == "eval-tree" {
        if let Some(events) = payload.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                if let Some(object) = event.as_object_mut() {
                    object.remove("artifacts");
                    object.remove("selected");
                    object.remove("detail");
                }
            }
        }
    }
    payload
}

fn project_event(event: &Value) -> Value {
    fn clipped(value: Option<&Value>, limit: usize) -> Option<String> {
        let value = value?.as_str()?;
        let mut end = value.len().min(limit);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        Some(value[..end].to_owned())
    }
    let artifact = event
        .get("artifacts")
        .and_then(Value::as_array)
        .and_then(|items| items.last())
        .cloned();
    json!({
        "id": clipped(event.get("id"), 96),
        "sequence": event.get("sequence").and_then(Value::as_u64),
        "kind": clipped(event.get("kind"), 32),
        "title": clipped(event.get("title"), 96),
        "detail": clipped(event.get("detail"), 160),
        "selected": clipped(event.get("selected"), 96),
        "confidence": event.get("confidence").and_then(Value::as_f64),
        "verdict": clipped(event.get("verdict"), 32),
        "artifacts": artifact.into_iter().collect::<Vec<_>>(),
    })
}

/// Incremental SSE framing with a hard event ceiling. Network chunk boundaries
/// have no semantic meaning; blank lines do.
#[derive(Debug, Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<(Option<String>, String, Vec<u8>)>, String> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_SSE_EVENT_BYTES {
            return Err(format!(
                "eval SSE event exceeds {MAX_SSE_EVENT_BYTES} bytes"
            ));
        }
        let mut frames = Vec::new();
        while let Some(end) = self.pending.windows(2).position(|pair| pair == b"\n\n") {
            let raw = self.pending.drain(..end + 2).collect::<Vec<_>>();
            if let Some(frame) = parse_frame(&raw[..end])? {
                frames.push(frame);
            }
        }
        Ok(frames)
    }
}

fn parse_frame(raw: &[u8]) -> Result<Option<(Option<String>, String, Vec<u8>)>, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "eval SSE frame is not UTF-8")?;
    let mut id = None;
    let mut event = "message".to_owned();
    let mut data = Vec::new();
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("event:") {
            event = value.trim_start().to_owned();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(value.trim_start().as_bytes());
        }
    }
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some((id, event, data)))
    }
}

pub fn decode_snapshot(event: &str, data: &[u8]) -> Result<RunSnapshot, String> {
    if event != "snapshot" {
        return Err(format!("expected snapshot SSE event, got {event}"));
    }
    let snapshot: RunSnapshot = serde_json::from_slice(data)
        .map_err(|error| format!("invalid eval watch snapshot: {error}"))?;
    snapshot.validate()?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sse_frame_decodes_once_complete() {
        let body =
            br#"{"schema":"eval.watch.v1","revision":7,"run":{},"state":"running","events":[]}"#;
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"id: 7\nevent: snap").unwrap().is_empty());
        let mut tail = b"shot\ndata: ".to_vec();
        tail.extend_from_slice(body);
        tail.extend_from_slice(b"\n\n");
        let frames = decoder.push(&tail).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            decode_snapshot(&frames[0].1, &frames[0].2)
                .unwrap()
                .revision,
            7
        );
    }

    #[test]
    fn projection_is_bounded_and_keeps_latest_artifact() {
        let events = (0..140)
            .map(|index| json!({"id": index, "artifacts": [format!("/{index}.stl")]}))
            .collect();
        let snapshot = RunSnapshot {
            schema: SOURCE_CONTRACT.into(),
            revision: 1,
            run: json!({}),
            state: "complete".into(),
            events,
            stls: vec![json!({}); 200],
            presentation: json!({}),
        };
        let projected = snapshot.project();
        assert_eq!(
            projected["events"].as_array().unwrap().len(),
            MAX_VISIBLE_EVENTS_PER_KIND
        );
        assert_eq!(
            projected["omittedEvents"],
            140 - MAX_VISIBLE_EVENTS_PER_KIND
        );
        assert_eq!(projected["activeArtifact"], "/139.stl");
        assert_eq!(projected["artifactCount"], 200);
    }

    #[test]
    fn projection_drops_cumulative_artifact_history_and_clips_text() {
        let snapshot = RunSnapshot {
            schema: SOURCE_CONTRACT.into(),
            revision: 1,
            run: json!({}),
            state: "running".into(),
            events: vec![
                json!({"id":"step","kind":"build","title":"x".repeat(500),"artifacts":["/0.stl","/1.stl","/2.stl"]}),
            ],
            stls: vec![],
            presentation: json!({}),
        };
        let projected = snapshot.project();
        let event = &projected["events"][0];
        assert_eq!(event["artifacts"], json!(["/2.stl"]));
        assert_eq!(event["title"].as_str().unwrap().len(), 96);
    }
}
