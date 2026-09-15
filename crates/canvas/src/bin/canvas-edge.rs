//! canvas-edge: attaches this machine's Canvas runtime to a unibus hub.
//!
//! Registers local displays as screen anchors, then translates bus traffic
//! into the Canvas control protocol with an explicit authority model
//! (dock marbles `dock-rfwf` follow-up):
//!
//! - `presence_update` is data: any hub-granted publisher may feed the rail.
//! - `canvas_command` is authority: only producers listed in `actors` may
//!   command, only the commands each is granted, always stamped with that
//!   actor's identity, and always answered with a `canvas.result` receipt so
//!   the central learns what *executed*, not just what was delivered.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas::control::{
    Command, ControlRequest, PresenceOccupant, PresenceSignal, PresenceSignalState,
    PresenceSnapshot,
};
use canvas_core::unibus::{Envelope, envelope, kinds, result_message};
use serde::Deserialize;

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeConfig {
    #[serde(flatten)]
    bus: BusConfig,
    /// Producers permitted to command this Canvas, with their granted
    /// command names (control-protocol variant names, snake_case).
    #[serde(default)]
    actors: Vec<BusActor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusActor {
    producer: String,
    /// Actor identity stamped onto accepted commands, e.g. "opencode:central".
    id: String,
    /// Command variant names this producer may execute. Empty means "observe
    /// only": presence flows, commands are refused.
    #[serde(default)]
    commands: Vec<String>,
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-edge <edge-config.json>")?;
    let config = EdgeConfig::read(std::path::Path::new(&path))?;
    let mut backoff = RECONNECT_MIN;
    loop {
        match run(&config) {
            Ok(()) => eprintln!("canvas-edge: hub connection closed; reconnecting"),
            Err(e) => eprintln!("canvas-edge: {e:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

impl EdgeConfig {
    fn read(path: &std::path::Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read edge config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse edge config {}", path.display()))?;
        for actor in &config.actors {
            ensure!(
                !actor.producer.trim().is_empty(),
                "actor with empty producer"
            );
            ensure!(!actor.id.trim().is_empty(), "actor with empty id");
        }
        Ok(config)
    }
}

fn run(config: &EdgeConfig) -> Result<()> {
    let bus = Bus::connect(config.bus.clone())
        .with_context(|| format!("connect hub {}", config.bus.hub_endpoint))?;
    eprintln!(
        "canvas-edge: registered as '{}' via {} (anchors {:?}, actors {:?})",
        config.bus.producer_id,
        config.bus.hub_endpoint,
        config.bus.screen_anchors,
        config
            .actors
            .iter()
            .map(|a| a.producer.as_str())
            .collect::<Vec<_>>(),
    );
    let quiet_after = Instant::now();
    loop {
        match bus.try_read() {
            Ok(Some((envelope, _payload))) => {
                if let Err(e) = ingest(&bus, config, &envelope) {
                    eprintln!("canvas-edge: rejected {}: {e:#}", envelope.message.kind);
                }
            }
            Ok(None) => {}
            Err(e) => return Err(e.context("hub read")),
        }
        // Reconnect if the hub went quiet well past the heartbeat window:
        // a half-open socket must not pretend to be an alive display.
        // Hubs push no keepalives, so quiet is normal; this only catches a
        // half-open downlink after several missed registry windows.
        if quiet_after.elapsed() > Duration::from_secs(90) {
            bail!("no hub frames for 90 s; forcing reconnect");
        }
    }
}

/// One inbound bus frame -> control action + optional receipt.
fn ingest(bus: &Bus, config: &EdgeConfig, envelope: &Envelope) -> Result<()> {
    match envelope.message.kind.as_str() {
        "presence_update" | "presence.update" => {
            let summary = envelope
                .message
                .body
                .get("snapshot")
                .context("missing snapshot")?;
            let snapshot = presence_snapshot(summary)?;
            submit(ControlRequest {
                schema_version: canvas::control::CONTROL_VERSION,
                request_id: format!("presence-{}", envelope::now_ms()),
                expected_revision: None,
                actor: None,
                command: Command::SetPresence { presence: snapshot },
            })
            .context("SetPresence")
        }
        "canvas_command" | "canvas.command" => {
            let raw = envelope
                .message
                .body
                .get("command")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let request_id = raw
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or(envelope.request_id.as_str())
                .to_owned();
            let outcome = match authorize(config, envelope, &raw) {
                Ok(request) => match submit(request) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(format!("{e:#}"))),
                },
                Err(reason) => {
                    eprintln!("canvas-edge: denied: {reason}");
                    (false, Some(reason))
                }
            };
            receipt(bus, envelope, &request_id, outcome.0, outcome.1)
        }
        kinds::V1_REPLY | kinds::REPLY => Ok(()),
        other => {
            // Unknown kinds are not errors: newer domains must not break
            // older edges. The log line is the loud part.
            eprintln!("canvas-edge: ignoring unrouted kind '{other}'");
            Ok(())
        }
    }
}

/// Producer allowlist + per-command grant, then actor stamping.
fn authorize(
    config: &EdgeConfig,
    envelope: &Envelope,
    raw: &serde_json::Value,
) -> Result<ControlRequest, String> {
    let actor = config
        .actors
        .iter()
        .find(|a| a.producer == envelope.producer_id)
        .ok_or_else(|| {
            format!(
                "producer '{}' is not an allowed command actor on this screen",
                envelope.producer_id
            )
        })?;
    let variant = raw
        .pointer("/command/command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "command is missing its tagged 'command' name".to_owned())?;
    if !actor.commands.iter().any(|c| c == variant) {
        return Err(format!(
            "actor '{}' is not granted command '{variant}'",
            actor.id
        ));
    }
    let mut value = raw.clone();
    value["actor"] = serde_json::json!({
        "id": actor.id,
        "timestamp_ms": envelope::now_ms(),
    });
    let request: ControlRequest =
        serde_json::from_value(value).map_err(|e| format!("invalid control request: {e}"))?;
    Ok(request)
}

fn submit(request: ControlRequest) -> Result<()> {
    request.validate().map_err(anyhow::Error::msg)?;
    canvas::control::send(&request)
        .map_err(|e| anyhow::anyhow!(e))
        .map(|_| ())
}

fn receipt(
    bus: &Bus,
    command_envelope: &Envelope,
    request_id: &str,
    ok: bool,
    error: Option<String>,
) -> Result<()> {
    let message = result_message(request_id, ok, error);
    let envelope = bus
        .to_destination(&command_envelope.producer_id, message)
        .context("build receipt envelope")?;
    bus.send(&envelope, &[]).context("send receipt")
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
            confidence: value
                .get("face")
                .and_then(|f| f.get("confidence"))
                .and_then(|c| c.as_f64())
                .filter(|c| (0.0..=1.0).contains(c))
                .map(|c| c as f32),
            age_ms: 0,
        },
        face: value
            .get("face")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
        voice: value
            .get("voice")
            .map(|s| map_signal(&s))
            .unwrap_or(PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            }),
    }
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
        confidence: signal
            .get("confidence")
            .and_then(|v| v.as_f64())
            .filter(|c| (0.0..=1.0).contains(c))
            .map(|c| c as f32),
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

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::unibus::{Message, parse_result};
    use std::path::PathBuf;

    fn config_with(actors: Vec<BusActor>) -> EdgeConfig {
        EdgeConfig {
            bus: BusConfig {
                installation_id: "home".into(),
                producer_id: "canvas-neo".into(),
                hub_endpoint: "127.0.0.1:1".into(),
                token_file: PathBuf::from("t"),
                zones: vec!["default".into()],
                capabilities: vec!["display".into()],
                screen_anchors: vec!["screen-laptop".into()],
                schema_version: 1,
            },
            actors,
        }
    }

    fn command_envelope_from(producer: &str, inner: serde_json::Value) -> Envelope {
        Envelope::new(
            "home",
            producer,
            None,
            "",
            Message::new("canvas_command", serde_json::json!({ "command": inner })),
        )
    }

    fn indicator_request(request_id: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": canvas::control::CONTROL_VERSION,
            "request_id": request_id,
            "command": {
                "command": "upsert_indicator",
                "indicator": {
                    "id": "assistant-status", "title": "Assistant", "state": "listening",
                },
            },
        })
    }

    #[test]
    fn unknown_producer_is_denied() {
        let config = config_with(vec![]);
        let envelope = command_envelope_from("ghost", indicator_request("r1"));
        let err = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap_err();
        assert!(err.contains("not an allowed command actor"));
    }

    #[test]
    fn granted_command_gets_the_actors_identity_stamped() {
        let config = config_with(vec![BusActor {
            producer: "canvas-central-studio".into(),
            id: "opencode:central".into(),
            commands: vec!["upsert_indicator".into()],
        }]);
        let envelope = command_envelope_from("canvas-central-studio", indicator_request("r2"));
        let request = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap();
        assert_eq!(request.request_id, "r2");
        let actor = request.actor.expect("actor stamped");
        assert_eq!(actor.id, "opencode:central");
        assert!(actor.timestamp_ms > 0);
    }

    #[test]
    fn ungranted_command_variant_is_denied() {
        let config = config_with(vec![BusActor {
            producer: "canvas-central-studio".into(),
            id: "opencode:central".into(),
            commands: vec!["set_presence".into()],
        }]);
        let envelope = command_envelope_from("canvas-central-studio", indicator_request("r3"));
        let err = authorize(&config, &envelope, &envelope.message.body["command"]).unwrap_err();
        assert!(err.contains("not granted command 'upsert_indicator'"));
    }

    #[test]
    fn receipts_parse_back_to_correlated_results() {
        let message = result_message("r4", false, Some("denied".into()));
        let parsed = parse_result(&message).expect("parses");
        assert_eq!(parsed.request_id, "r4");
        assert!(!parsed.ok);
        assert_eq!(parsed.error.as_deref(), Some("denied"));
    }

    #[test]
    fn presence_mapping_still_validates() {
        let summary = serde_json::json!({
            "zone": "default", "occupied": true, "recency_ms": 80,
            "occupants": [{"id": "avery", "display_name": "Avery",
                "face": {"state": "known", "confidence": 0.9, "age_ms": 80},
                "voice": {"state": "unavailable", "confidence": null, "age_ms": 0}}],
            "tact": "Direct",
        });
        presence_snapshot(&summary).unwrap().validate().unwrap();
    }
}
