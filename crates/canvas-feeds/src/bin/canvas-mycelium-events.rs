//! Bridge Mycelium's durable MQTT security export into ephemeral Canvas chrome.
//!
//! Mycelium owns collection, gossip, credentials, and MQTT delivery. This
//! process only narrows the exported record to Canvas's shared notification
//! contract and publishes it through Canvas's owner-local stream socket.

use canvas_core::notification::{Level, Notification};
use rumqttc::{Client, Event, Incoming, MqttOptions, QoS};
use serde::Deserialize;
use serde_json::json;
use std::process::ExitCode;
use std::time::Duration;

const CONTRACT: &str = canvas_core::notification::CONTRACT;
const DEFAULT_TOPIC: &str = "mycelium/security/events/v1";

#[derive(Debug, Deserialize)]
struct ExportRecord {
    delivery_id: String,
    hostname: String,
    site: String,
    observed_at: u64,
    category: String,
    severity: Severity,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Severity {
    Informational,
    Low,
    Medium,
    High,
    Critical,
    Unknown,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("canvas-mycelium-events: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let host = env("MYCELIUM_MQTT_HOST").unwrap_or_else(|| "127.0.0.1".into());
    let port = env("MYCELIUM_MQTT_PORT")
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(1883);
    let topic = env("MYCELIUM_MQTT_TOPIC").unwrap_or_else(|| DEFAULT_TOPIC.into());
    let client_id = env("MYCELIUM_MQTT_CLIENT_ID").unwrap_or_else(|| {
        format!(
            "canvas-{}",
            env("HOSTNAME").unwrap_or_else(|| "screen".into())
        )
    });

    let mut options = MqttOptions::new(client_id, host, port);
    options.set_keep_alive(Duration::from_secs(30));
    if let (Some(username), Some(password)) = (
        env("MYCELIUM_MQTT_USERNAME"),
        env("MYCELIUM_MQTT_PASSWORD"),
    ) {
        options.set_credentials(username, password);
    }

    let (client, mut connection) = Client::new(options, 32);
    client.subscribe(&topic, QoS::AtLeastOnce)?;
    eprintln!("canvas-mycelium-events: subscribed to {topic}");
    for event in connection.iter() {
        let Event::Incoming(Incoming::Publish(publish)) = event? else {
            continue;
        };
        match serde_json::from_slice::<ExportRecord>(&publish.payload) {
            Ok(record) => emit(record)?,
            Err(error) => eprintln!(
                "canvas-mycelium-events: rejected event on {}: {error}",
                publish.topic
            ),
        }
    }
    Err("MQTT connection ended".into())
}

fn emit(record: ExportRecord) -> Result<(), Box<dyn std::error::Error>> {
    let notification = narrow(record);
    notification.validate()?;
    let sequence = notification.created_at_ms;
    let envelope = json!({
        "schema_version": 1,
        "owner": "mycelium",
        "stream": "security-events",
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": now_ms(),
        "payload": notification,
    });
    canvas_feeds::send_raw(&envelope).map_err(|error| error.to_string().into())
}

fn narrow(record: ExportRecord) -> Notification {
    let level = match record.severity {
        Severity::Informational | Severity::Unknown => Level::Neutral,
        Severity::Low => Level::Good,
        Severity::Medium => Level::Warning,
        Severity::High | Severity::Critical => Level::Critical,
    };
    Notification {
        id: record.delivery_id,
        project_id: None,
        title: format!("{} · {} · {}", record.hostname, record.site, record.category),
        message: record.message,
        level,
        created_at_ms: record.observed_at.saturating_mul(1_000).max(1),
        announce: false,
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_security_event_narrows_to_shared_notification_contract() {
        let notification = narrow(ExportRecord {
            delivery_id: "node:event-7".into(),
            hostname: "home-pi".into(),
            site: "wagar-house".into(),
            observed_at: 42,
            category: "authentication".into(),
            severity: Severity::High,
            message: "SSH login failed".into(),
        });
        notification.validate().unwrap();
        assert_eq!(notification.id, "node:event-7");
        assert_eq!(notification.level, Level::Critical);
        assert_eq!(notification.created_at_ms, 42_000);
        assert!(!notification.announce);
    }
}
