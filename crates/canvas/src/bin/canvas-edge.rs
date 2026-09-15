//! canvas-edge: attaches this machine's Canvas runtime to a unibus hub.
//!
//! Registers the local displays as screen anchors, then translates bus
//! traffic into the Canvas control protocol: `presence_update` becomes a
//! `SetPresence` snapshot, `canvas_command` bodies map onto control
//! commands. Commands addressed to another screen or zone never reach this
//! process — the hub's registry decides. (dock marbles `dock-rfwf`.)

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use canvas::control::{
    Command, ControlRequest, PresenceOccupant, PresenceSignal, PresenceSignalState,
    PresenceSnapshot,
};
use canvas_core::unibus::{Envelope, FrameError, Message, envelope, framing, kinds};
use serde::Deserialize;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeConfig {
    installation_id: String,
    producer_id: String,
    hub_endpoint: String,
    token_file: PathBuf,
    zones: Vec<String>,
    screen_anchors: Vec<String>,
}

impl EdgeConfig {
    fn read(path: &std::path::Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read edge config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse edge config {}", path.display()))?;
        ensure!(
            !config.zones.is_empty(),
            "edge must serve at least one zone"
        );
        Ok(config)
    }

    fn token(&self) -> Result<String> {
        Ok(std::fs::read_to_string(&self.token_file)
            .with_context(|| format!("read hub token {}", self.token_file.display()))?
            .trim()
            .to_owned())
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-edge <edge-config.json>")?;
    let config = EdgeConfig::read(std::path::Path::new(&path))?;
    let mut backoff = Duration::from_millis(500);
    loop {
        match run(&config) {
            Ok(()) => eprintln!("canvas-edge: hub connection closed; reconnecting"),
            Err(e) => eprintln!("canvas-edge: {e:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

fn run(config: &EdgeConfig) -> Result<()> {
    let addr: std::net::SocketAddr = config
        .hub_endpoint
        .parse()
        .context("hub_endpoint must be ip:port")?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .with_context(|| format!("connect hub at {addr}"))?;
    let mut writer = stream.try_clone()?;
    let mut reader = stream;
    reader.set_read_timeout(Some(READ_TIMEOUT))?;
    let token = config.token()?;

    let register = Envelope::new(
        &config.installation_id,
        &config.producer_id,
        None,
        &token,
        Message::new(
            kinds::V1_REGISTER,
            serde_json::json!({
                // v1 spelling for deployed hub routes; namespaced for v2.
                "capabilities": ["display", "canvas.display"],
                "zones": config.zones,
                "screen_anchors": config.screen_anchors,
            }),
        ),
    );
    framing::write_frame(&mut writer, &register, &[]).context("send register")?;
    let (reply, _) = framing::read_frame(&mut reader).context("read register reply")?;
    ensure!(
        reply.message.kind == kinds::V1_REPLY && reply.message.body["success"] == true,
        "hub rejected registration: {:?}",
        reply.message.body
    );
    eprintln!(
        "canvas-edge: registered with hub {addr} as '{}' (zones {:?}, anchors {:?})",
        config.producer_id, config.zones, config.screen_anchors
    );

    let alive = std::sync::Arc::new(AtomicBool::new(true));
    let heartbeat = {
        let token = token.clone();
        let installation = config.installation_id.clone();
        let producer = config.producer_id.clone();
        let alive = alive.clone();
        thread::Builder::new()
            .name("canvas-edge-heartbeat".into())
            .spawn(move || {
                while alive.load(Ordering::Acquire) {
                    thread::sleep(HEARTBEAT_INTERVAL);
                    let envelope = Envelope::new(
                        &installation,
                        &producer,
                        None,
                        &token,
                        Message::new(kinds::V1_HEARTBEAT, serde_json::Value::Null),
                    );
                    if let Err(e) = framing::write_frame(&mut writer, &envelope, &[]) {
                        eprintln!("canvas-edge: heartbeat failed: {e:#}");
                        break;
                    }
                }
            })
            .context("spawn heartbeat thread")?
    };

    let outcome = (|| -> Result<()> {
        loop {
            let (envelope, _) = match framing::read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(FrameError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(FrameError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    bail!("hub closed the connection");
                }
                Err(e) => return Err(e.into()),
            };
            if let Err(e) = ingest(&envelope) {
                // A bad frame is the sender's bug, not ours: report loudly,
                // keep the bus alive.
                eprintln!(
                    "canvas-edge: rejected {} frame: {e:#}",
                    envelope.message.kind
                );
            }
        }
    })();

    alive.store(false, Ordering::Release);
    let _ = heartbeat.join();
    outcome
}

/// One inbound bus frame -> zero or one Canvas control request.
fn ingest(envelope: &Envelope) -> Result<()> {
    match envelope.message.kind.as_str() {
        "presence_update" | "presence.update" => {
            let summary = envelope
                .message
                .body
                .get("snapshot")
                .context("missing snapshot")?;
            let snapshot = presence_snapshot(summary)?;
            submit(
                Command::SetPresence { presence: snapshot },
                &envelope.request_id,
            )
        }
        "canvas_command" | "canvas.command" => {
            // The bus carries a complete Canvas control request verbatim —
            // the hub is a relay, not a translator.
            let request = serde_json::from_value::<ControlRequest>(
                envelope
                    .message
                    .body
                    .get("command")
                    .context("missing command")?
                    .clone(),
            )
            .context("canvas_command body is not a Canvas control request")?;
            canvas::control::send(&request)
                .map_err(anyhow::Error::msg)
                .map(|_| ())
        }
        "reply" | "unibus.reply" => Ok(()),
        other => {
            // Unknown kinds are not errors: newer domains must not break
            // older edges. The log line is the loud part.
            eprintln!("canvas-edge: ignoring unrouted kind '{other}'");
            Ok(())
        }
    }
}

fn presence_snapshot(summary: &serde_json::Value) -> Result<PresenceSnapshot> {
    let zone = text(summary, "zone")?;
    let occupied = summary
        .get("occupied")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let recency_ms = summary
        .get("recency_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let occupants = summary
        .get("occupants")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(presence_occupant)
        .collect();
    Ok(PresenceSnapshot {
        zone: zone.chars().take(64).collect(),
        observed_at_unix_ms: envelope::now_ms().saturating_sub(recency_ms),
        // The hub re-publishes on every snapshot; a stale rail goes
        // Unknown by expiry rather than lying about an empty room.
        expires_after_ms: (recency_ms + 10_000).clamp(250, 300_000),
        occupied,
        occupants,
    })
}

fn presence_occupant(value: &serde_json::Value) -> PresenceOccupant {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect();
    let display_name = value
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .chars()
        .take(64)
        .collect();
    PresenceOccupant {
        id,
        display_name,
        presence: PresenceSignal {
            state: PresenceSignalState::Detected,
            confidence: signal(value, "face").and_then(|f| signal_confidence(&f)),
            age_ms: 0,
        },
        face: signal(value, "face")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
        voice: signal(value, "voice")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
    }
}

fn signal<'a>(occupant: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    occupant.get(name)
}

fn signal_confidence(signal: &serde_json::Value) -> Option<f32> {
    signal
        .get("confidence")
        .and_then(|v| v.as_f64())
        .filter(|c| (0.0..=1.0).contains(c))
        .map(|c| c as f32)
}

fn map_signal(signal: &serde_json::Value) -> PresenceSignal {
    let state = match signal
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
    {
        "known" | "verified" | "matched" => PresenceSignalState::Verified,
        "detected" => PresenceSignalState::Detected,
        "unavailable" => PresenceSignalState::Unavailable,
        _ => PresenceSignalState::Unknown,
    };
    PresenceSignal {
        state,
        confidence: signal_confidence(signal),
        age_ms: signal.get("age_ms").and_then(|v| v.as_u64()).unwrap_or(0),
    }
}

fn text(value: &serde_json::Value, field: &str) -> Result<String> {
    Ok(value
        .get(field)
        .and_then(|v| v.as_str())
        .with_context(|| format!("missing string field '{field}'"))?
        .to_owned())
}

fn submit(command: Command, request_id: &str) -> Result<()> {
    let request = ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: request_id.to_owned(),
        expected_revision: None,
        actor: None,
        command,
    };
    canvas::control::send(&request)
        .map_err(anyhow::Error::msg)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_summary_maps_to_canvas_snapshot() {
        let summary = serde_json::json!({
            "zone": "living-room",
            "occupied": true,
            "occupants": [{
                "id": "avery",
                "display_name": "Avery",
                "face": {"state": "known", "confidence": 0.91, "age_ms": 120},
                "voice": {"state": "unavailable", "confidence": null, "age_ms": 9000},
            }],
            "recency_ms": 120,
            "tact": "Direct",
        });
        let snapshot = presence_snapshot(&summary).unwrap();
        snapshot.validate().unwrap();
        assert_eq!(snapshot.zone, "living-room");
        assert!(snapshot.occupied);
        assert_eq!(
            snapshot.occupants[0].face.state,
            PresenceSignalState::Verified
        );
        assert_eq!(
            snapshot.occupants[0].voice.state,
            PresenceSignalState::Unavailable
        );
    }

    #[test]
    fn empty_presence_stays_valid_and_expires() {
        let summary = serde_json::json!({
            "zone": "living-room", "occupied": false, "occupants": [], "recency_ms": 0, "tact": "Direct",
        });
        let snapshot = presence_snapshot(&summary).unwrap();
        snapshot.validate().unwrap();
        assert!(!snapshot.occupied);
        assert!(snapshot.expires_after_ms >= 250);
    }

    #[test]
    fn sound_style_canvas_command_deserializes_as_a_control_request() {
        // The exact shape jarvis Sound emits over the router today.
        let envelope = Envelope::new(
            "home",
            "mac-studio-brain",
            None,
            "",
            Message::new(
                "canvas_command",
                serde_json::json!({"command": {
                    "schema_version": 1,
                    "request_id": "voice-status-1",
                    "command": {
                        "command": "upsert_indicator",
                        "indicator": {
                            "id": "jarvis-voice", "title": "Jarvis", "state": "speaking",
                            "caption": "one two three",
                        },
                    },
                }}),
            ),
        );
        let request =
            serde_json::from_value::<ControlRequest>(envelope.message.body["command"].clone())
                .unwrap();
        request.validate().unwrap();
        assert_eq!(request.request_id, "voice-status-1");
    }

    #[test]
    fn malformed_canvas_command_fails_loud() {
        let envelope = Envelope::new(
            "home",
            "brain",
            None,
            "",
            Message::new(
                "canvas_command",
                serde_json::json!({"command": {"upsert_indicator": {}}}),
            ),
        );
        assert!(ingest(&envelope).is_err());
    }

    #[test]
    fn config_rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edge.json");
        std::fs::write(
            &path,
            r#"{"installation_id":"home","producer_id":"canvas-neo","hub_endpoint":"127.0.0.1:18790","token_file":"t","zones":["default"],"screen_anchors":[],"sudo":true}"#,
        )
        .unwrap();
        assert!(EdgeConfig::read(&path).is_err());
    }
}
