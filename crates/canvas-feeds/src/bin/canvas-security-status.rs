//! Read Mycelium's owned security observations; project them into an existing
//! status-list primitive. No scanning, discovery, credentials or remediation.
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Command, time::SystemTime};

fn read(kind: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = Command::new("mycelium")
        .args(["security", kind, "--json"])
        .output()?;
    if !output.status.success() {
        return Err(format!("Mycelium security {kind} failed").into());
    }
    if output.stdout.len() > 4 * 1024 * 1024 {
        return Err("Security observation exceeds 4 MiB".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn project(status: &Value, events: &Value, now: i64) -> Result<Value, &'static str> {
    let records = status.as_array().ok_or("Expected security status array")?;
    let event_records = events.as_array().ok_or("Expected security events array")?;
    // Same host can have historical identities after reinstall. Keep newest
    // observation per site/hostname; never merge equal hostnames across sites.
    let mut hosts = BTreeMap::<(String, String), &Value>::new();
    for record in records {
        let key = (
            record["site"].as_str().unwrap_or("unknown").to_owned(),
            record["hostname"].as_str().unwrap_or("unknown").to_owned(),
        );
        let previous = hosts.get(&key);
        if previous.is_none_or(|old| record["observed_at"].as_i64() > old["observed_at"].as_i64()) {
            hosts.insert(key, record);
        }
    }
    let mut nodes = Vec::new();
    let mut fresh_hosts = 0;
    for ((site, host), record) in &hosts {
        let age = now.saturating_sub(record["observed_at"].as_i64().unwrap_or(0));
        let scans = record["scanners"].as_object();
        let incomplete =
            scans.is_none_or(|scans| scans.values().any(|v| v.as_str() != Some("available")));
        let findings = record["findings"].as_array().map_or(0, Vec::len);
        let updates = record["security_updates_available"].as_u64().unwrap_or(0);
        let stale = !(0..=300).contains(&age);
        fresh_hosts += usize::from(!stale);
        nodes.push(json!({
            "label": format!("{host} · {site}"),
            "state": if stale { "neutral" } else if findings > 0 || updates > 0 || incomplete { "warning" } else { "good" },
            "detail": if stale { format!("Stale observation · {age}s old") }
                else { format!("{findings} findings · {updates} updates · {}", if incomplete { "coverage incomplete" } else { "scanners available" }) }
        }));
    }
    // Newest events first, deduplicated by Mycelium delivery/event identity.
    let mut alerts = BTreeMap::new();
    for record in event_records {
        for event in record["events"].as_array().into_iter().flatten() {
            if let Some(id) = event["id"].as_str() {
                alerts.entry(id).or_insert((record, event));
            }
        }
    }
    let mut alerts: Vec<_> = alerts.into_values().collect();
    alerts.sort_by_key(|(_, event)| std::cmp::Reverse(event["observed_at"].as_i64()));
    for (record, event) in alerts.iter().take(3) {
        let age = now.saturating_sub(event["observed_at"].as_i64().unwrap_or(0));
        let historical = !(0..=300).contains(&age);
        nodes.push(json!({"label": format!("Event · {}", record["hostname"].as_str().unwrap_or("unknown")),
            "state": if historical { "neutral" } else if matches!(event["severity"].as_str(), Some("high" | "critical")) { "critical" } else { "warning" },
            "detail": format!("{}{}", if historical { format!("Historical · {age}s ago · ") } else { String::new() }, event["message"].as_str().unwrap_or("No message")) }));
    }
    let total = nodes.len();
    nodes.truncate(9);
    let observed = DateTime::<Utc>::from_timestamp(now, 0).ok_or("Invalid timestamp")?;
    Ok(json!({
        "widget": {"kind": "status_list", "title": format!("Security · {} hosts · {} events", hosts.len(), alerts.len()), "nodes": nodes},
        "data": {"state": if hosts.is_empty() { "no_data" } else if fresh_hosts == 0 { "stale" } else { "partial" },
            "source": "mycelium security status/events", "observed_at": observed.to_rfc3339_opts(SecondsFormat::Millis, true),
            "detail": format!("Read-only owner telemetry · showing {} of {total} rows · host freshness shown per row", total.min(9))}
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    println!("{}", project(&read("status")?, &read("events")?, now)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_newest_identity_without_claiming_missing_scans_are_healthy() {
        let status = json!([
            {"hostname":"a", "site":"home", "observed_at":1},
            {"hostname":"a", "site":"home", "observed_at":999},
            {"hostname":"a", "site":"lab", "observed_at":1}
        ]);
        let output = project(&status, &json!([]), 1000).unwrap();
        assert_eq!(output["widget"]["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(output["widget"]["nodes"][0]["state"], "warning");
        assert_eq!(output["widget"]["nodes"][1]["state"], "neutral");
    }
    #[test]
    fn deduplicates_events_and_handles_empty_and_malformed_observations() {
        let event =
            json!({"id":"e", "observed_at":999, "severity":"high", "message":"sensor failed"});
        let output = project(
            &json!([]),
            &json!([{"hostname":"a", "events":[event.clone(), event]}]),
            1000,
        )
        .unwrap();
        assert_eq!(output["widget"]["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(output["data"]["state"], "no_data");
        assert_eq!(output["widget"]["nodes"][0]["state"], "critical");
        assert!(project(&json!({}), &json!([]), 1000).is_err());
    }
    #[test]
    fn historical_events_do_not_pretend_to_be_live_alerts() {
        let output = project(&json!([{"hostname":"a", "site":"s", "observed_at":1}]),
            &json!([{"hostname":"a", "events":[{"id":"old", "observed_at":1, "severity":"critical", "message":"old alert"}]}]), 1000).unwrap();
        assert_eq!(output["data"]["state"], "stale");
        assert_eq!(output["widget"]["nodes"][1]["state"], "neutral");
    }
}
