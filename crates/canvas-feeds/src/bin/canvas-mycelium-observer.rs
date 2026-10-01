use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

const OWNER: &str = "mycelium";
const STREAM: &str = "mesh";
const MAX_TEXT_CHARS: usize = 64;

#[derive(Debug, Deserialize)]
struct PeerView {
    hello: Option<PeerHello>,
    health: Option<PeerHealth>,
    last_seen: u64,
}

#[derive(Debug, Deserialize)]
struct PeerHello {
    hostname: String,
    site: String,
    platform: String,
    architecture: String,
    daemon_version: String,
}

#[derive(Debug, Deserialize)]
struct PeerHealth {
    load_average: [f64; 3],
    logical_cpus: u32,
    memory_total_bytes: u64,
    memory_available_bytes: u64,
    filesystems: Vec<FilesystemHealth>,
    ssh_listening: bool,
}

#[derive(Debug, Deserialize)]
struct FilesystemHealth {
    mount: String,
    total_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Serialize)]
struct PeerSummary {
    hostname: String,
    site: String,
    platform: String,
    architecture: String,
    version: String,
    age_seconds: u64,
    load_1: f64,
    logical_cpus: u32,
    memory_available_bytes: u64,
    memory_total_bytes: u64,
    root_available_bytes: Option<u64>,
    root_total_bytes: Option<u64>,
    ssh_listening: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReleaseSummary {
    #[serde(default = "default_component")]
    component: String,
    version: String,
    channel: String,
    target: String,
}

fn default_component() -> String {
    "mycelium".into()
}

fn bounded(text: String) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

fn project(peers: Vec<PeerView>, now_seconds: u64) -> Vec<PeerSummary> {
    let mut peers = peers
        .into_iter()
        .filter_map(|peer| {
            let hello = peer.hello?;
            let health = peer.health?;
            let root = health
                .filesystems
                .iter()
                .find(|filesystem| filesystem.mount == "/");
            Some(PeerSummary {
                hostname: bounded(hello.hostname),
                site: bounded(hello.site),
                platform: bounded(hello.platform),
                architecture: bounded(hello.architecture),
                version: bounded(hello.daemon_version),
                age_seconds: now_seconds.saturating_sub(peer.last_seen),
                load_1: health.load_average[0],
                logical_cpus: health.logical_cpus,
                memory_available_bytes: health.memory_available_bytes,
                memory_total_bytes: health.memory_total_bytes,
                root_available_bytes: root.map(|filesystem| filesystem.available_bytes),
                root_total_bytes: root.map(|filesystem| filesystem.total_bytes),
                ssh_listening: health.ssh_listening,
            })
        })
        .collect::<Vec<_>>();
    peers.sort_by(|left, right| (&left.site, &left.hostname).cmp(&(&right.site, &right.hostname)));
    peers.truncate(12);
    peers
}

fn query(binary: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(binary)
        .args(args)
        .env("MYCELIUM_NO_AUTOSTART", "1")
        .output()
        .map_err(|error| format!("start {binary}: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

fn snapshot(binary: &str, now_ms: u64) -> serde_json::Value {
    let result = (|| {
        let peers = serde_json::from_slice::<Vec<PeerView>>(&query(binary, &["peers", "--json"])?)
            .map_err(|error| format!("decode peers: {error}"))?;
        let releases = serde_json::from_slice::<Vec<ReleaseSummary>>(&query(
            binary,
            &["releases", "list", "--json"],
        )?)
        .map_err(|error| format!("decode releases: {error}"))?;
        Ok::<_, String>((project(peers, now_ms / 1000), releases))
    })();
    let payload = match result {
        Ok((peers, mut releases)) => {
            releases.truncate(16);
            for release in &mut releases {
                release.component = bounded(std::mem::take(&mut release.component));
                release.version = bounded(std::mem::take(&mut release.version));
                release.channel = bounded(std::mem::take(&mut release.channel));
                release.target = bounded(std::mem::take(&mut release.target));
            }
            json!({ "peers": peers, "releases": releases, "degraded": false })
        }
        Err(error) => {
            json!({ "peers": [], "releases": [], "degraded": true, "error": bounded(error) })
        }
    };
    fit_snapshot(json!({
        "schema_version": 1, "owner": OWNER, "stream": STREAM,
        "contract": "canvas.widget.snapshot.v1", "sequence": now_ms, "emitted_at_ms": now_ms,
        "payload": payload
    }))
}

fn fit_snapshot(mut event: serde_json::Value) -> serde_json::Value {
    let budget = canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD;
    while serde_json::to_vec(&event).map_or(usize::MAX, |bytes| bytes.len()) > budget {
        let removed = event
            .pointer_mut("/payload/releases")
            .and_then(|value| value.as_array_mut())
            .is_some_and(|releases| releases.pop().is_some())
            || event
                .pointer_mut("/payload/peers")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|peers| peers.len() > 1 && peers.pop().is_some());
        if !removed {
            break;
        }
        event["payload"]["truncated"] = json!(true);
    }
    event
}

fn main() -> ExitCode {
    let binary = std::env::var("MYCELIUM_BIN").unwrap_or_else(|_| "mycelium".into());
    loop {
        let now = canvas_feeds::now_ms();
        if let Err(error) = canvas_feeds::send_raw(&snapshot(&binary, now)) {
            eprintln!("canvas-mycelium-observer: publish: {error}");
        }
        thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_is_bounded_and_keeps_only_canvas_health_facts() {
        let peer: PeerView = serde_json::from_value(json!({
            "hello": {"hostname":"home-pi","site":"home","platform":"linux","architecture":"aarch64","daemon_version":"0.1.0"},
            "health": {"load_average":[1.5,1.0,0.5],"logical_cpus":4,"memory_total_bytes":100,"memory_available_bytes":25,
                "filesystems":[{"mount":"/","total_bytes":1000,"available_bytes":400}],"ssh_listening":true,
                "process_leaders":[{"name":"secret-process-detail"}]},
            "last_seen": 90
        })).unwrap();
        let projected = project(vec![peer], 100);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].age_seconds, 10);
        assert_eq!(projected[0].root_available_bytes, Some(400));
        assert_eq!(
            serde_json::to_value(&projected[0])
                .unwrap()
                .get("process_leaders"),
            None
        );
    }

    #[test]
    fn mesh_snapshot_fits_the_portable_datagram_budget() {
        let peer = json!({
            "hostname": "x".repeat(64), "site": "x".repeat(64),
            "platform": "x".repeat(64), "architecture": "x".repeat(64),
            "version": "x".repeat(64), "age_seconds": 1, "load_1": 1.0,
            "logical_cpus": 4, "memory_available_bytes": 1,
            "memory_total_bytes": 2, "root_available_bytes": 1,
            "root_total_bytes": 2, "ssh_listening": true
        });
        let event = json!({
            "schema_version": 1, "owner": OWNER, "stream": STREAM,
            "contract": "canvas.widget.snapshot.v1", "sequence": 1, "emitted_at_ms": 1,
            "payload": { "peers": vec![peer; 12], "releases": [], "degraded": false }
        });
        let fitted = fit_snapshot(event);
        assert!(
            serde_json::to_vec(&fitted).unwrap().len()
                <= canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD
        );
        assert_eq!(fitted.pointer("/payload/truncated"), Some(&json!(true)));
    }
}
