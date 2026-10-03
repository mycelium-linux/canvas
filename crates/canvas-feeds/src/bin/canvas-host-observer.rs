use serde::Serialize;
use serde_json::json;
use std::{
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

const OWNER: &str = "home-pi";
const STREAM: &str = "systems";
const SERVICES: [(&str, &str); 6] = [
    ("Unibus router", "unibus-router.service"),
    ("Unibus audio tap", "unibus-audio-tap.service"),
    ("Isochrone receiver", "isochrone-receiver.service"),
    ("Synesthesia console", "synesthesia-console.service"),
    ("Now playing", "canvas-now-playing.service"),
    ("Mycelium topology", "mycelium.service"),
];

#[derive(Serialize)]
struct ServiceState<'a> {
    label: &'a str,
    unit: &'a str,
    active: bool,
    detail: String,
}

fn observe(label: &'static str, unit: &'static str) -> ServiceState<'static> {
    let output = Command::new("systemctl")
        .args(["show", unit, "--property=LoadState,ActiveState,SubState"])
        .output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.split_once('=').filter(|(key, _)| *key == name))
                    .map(|(_, value)| value)
                    .unwrap_or("unknown")
            };
            let loaded = field("LoadState");
            let active = field("ActiveState");
            let sub = field("SubState");
            ServiceState {
                label,
                unit,
                active: loaded == "loaded" && active == "active",
                detail: format!("{loaded} · {active}/{sub}"),
            }
        }
        Err(error) => ServiceState {
            label,
            unit,
            active: false,
            detail: format!("observation failed: {error}"),
        },
    }
}

fn snapshot(now: u64) -> serde_json::Value {
    let services = SERVICES.map(|(label, unit)| observe(label, unit));
    json!({
        "schema_version": 1, "owner": OWNER, "stream": STREAM,
        "contract": "canvas.widget.snapshot.v1", "sequence": now, "emitted_at_ms": now,
        "payload": { "host": "home-pi", "services": services }
    })
}

fn main() -> ExitCode {
    loop {
        let now = canvas_feeds::now_ms();
        if let Err(error) = canvas_feeds::send_raw(&snapshot(now)) {
            eprintln!("canvas-host-observer: publish: {error}");
        }
        thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_is_narrow_and_identifies_each_owned_service() {
        assert_eq!(SERVICES.len(), 6);
        assert!(SERVICES.iter().any(|(_, unit)| *unit == "mycelium.service"));
    }
}
