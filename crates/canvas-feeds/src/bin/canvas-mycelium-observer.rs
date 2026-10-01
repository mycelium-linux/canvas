use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
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
    node_id: String,
    hostname: String,
    site: String,
    platform: String,
    architecture: String,
}

#[derive(Debug, Deserialize)]
struct PeerHealth {
    load_average: [f64; 3],
    logical_cpus: u32,
    memory_total_bytes: u64,
    memory_available_bytes: u64,
    filesystems: Vec<FilesystemHealth>,
}

#[derive(Debug, Deserialize)]
struct FilesystemHealth {
    mount: String,
    available_bytes: u64,
}

#[derive(Debug, Serialize)]
struct PeerSummary {
    id: String,
    hostname: String,
    site: String,
    platform: String,
    architecture: String,
    age_seconds: u64,
    load_1: f64,
    logical_cpus: u32,
    memory_available_bytes: u64,
    memory_total_bytes: u64,
    root_available_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TopologyView {
    #[serde(default)]
    segments: BTreeMap<String, TopologySegment>,
}

#[derive(Debug, Deserialize)]
struct TopologySegment {
    id: String,
    kind: String,
    vlan: Option<u16>,
    subnet: Option<(String, u8)>,
    gw: Option<String>,
    domain_name: Option<String>,
    #[serde(default)]
    origins: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SegmentSummary {
    id: String,
    label: String,
    kind: String,
    site: Option<String>,
    vlan: Option<u16>,
    subnet: Option<String>,
    gateway: Option<String>,
}

#[derive(Debug, Serialize)]
struct VantageNodeSummary {
    id: String,
    label: String,
    site: Option<String>,
    kind: String,
    device: bool,
    ips: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AdvertisementSummary {
    id: String,
    label: String,
    site: Option<String>,
    service_type: String,
    target: Option<String>,
    port: Option<u16>,
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

fn bounded_id(text: String) -> String {
    text.chars().take(16).collect()
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
                id: bounded_id(hello.node_id),
                hostname: bounded(hello.hostname),
                site: bounded(hello.site),
                platform: bounded(hello.platform),
                architecture: bounded(hello.architecture),
                age_seconds: now_seconds.saturating_sub(peer.last_seen),
                load_1: health.load_average[0],
                logical_cpus: health.logical_cpus,
                memory_available_bytes: health.memory_available_bytes,
                memory_total_bytes: health.memory_total_bytes,
                root_available_bytes: root.map(|filesystem| filesystem.available_bytes),
            })
        })
        .collect::<Vec<_>>();
    peers.sort_by(|left, right| (&left.site, &left.hostname).cmp(&(&right.site, &right.hostname)));
    peers.truncate(12);
    peers
}

fn project_segments(topology: TopologyView) -> Vec<SegmentSummary> {
    let mut segments = topology
        .segments
        .into_values()
        .map(|segment| {
            let site = segment.origins.iter().find_map(|origin| {
                origin
                    .split_once('/')
                    .map(|(site, _)| bounded(site.to_owned()))
            });
            SegmentSummary {
                label: bounded(segment.domain_name.unwrap_or_else(|| segment.id.clone())),
                id: bounded(segment.id),
                kind: bounded(segment.kind),
                site,
                vlan: segment.vlan,
                subnet: segment
                    .subnet
                    .map(|(address, prefix)| format!("{address}/{prefix}")),
                gateway: segment.gw.map(bounded),
            }
        })
        .collect::<Vec<_>>();
    segments.sort_by(|left, right| left.id.cmp(&right.id));
    segments.truncate(32);
    segments
}

fn project_vantage(
    topology: &serde_json::Value,
) -> (Vec<VantageNodeSummary>, Vec<AdvertisementSummary>, usize) {
    let mut nodes = topology["nodes"]
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
        .filter_map(|node| {
            let id = node["id"].as_str()?.to_owned();
            let annotation = &node["annotation"];
            let host = node["hostnames"]
                .as_array()
                .and_then(|names| names.first())
                .and_then(|name| name.as_str());
            let label = annotation["name"]
                .as_str()
                .or(host)
                .unwrap_or(&id)
                .to_owned();
            let site = node["sites"]
                .as_array()
                .and_then(|sites| sites.first())
                .and_then(|site| site.as_str())
                .map(|site| bounded(site.to_owned()));
            let device = node["device"].as_bool().unwrap_or(false);
            let kind =
                annotation["kind"]
                    .as_str()
                    .unwrap_or(if device { "device" } else { "observed" });
            let ips = node["ips"]
                .as_object()
                .into_iter()
                .flat_map(|ips| ips.keys())
                .take(4)
                .cloned()
                .collect();
            Some(VantageNodeSummary {
                id: bounded(id),
                label: bounded(label),
                site,
                kind: bounded(kind.to_owned()),
                device,
                ips,
            })
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| (&left.site, &left.label).cmp(&(&right.site, &right.label)));
    nodes.truncate(160);
    let mut advertisements = topology["advertisements"]
        .as_object()
        .into_iter()
        .flat_map(|items| items.iter())
        .map(|(id, item)| {
            let site = item["origins"]
                .as_array()
                .and_then(|origins| origins.first())
                .and_then(|origin| origin.as_str())
                .and_then(|origin| origin.split_once('/'))
                .map(|(site, _)| bounded(site.to_owned()));
            AdvertisementSummary {
                id: bounded(id.clone()),
                label: bounded(item["instance"].as_str().unwrap_or(id).to_owned()),
                site,
                service_type: bounded(
                    item["service_type"]
                        .as_str()
                        .unwrap_or("service")
                        .to_owned(),
                ),
                target: item["target"]
                    .as_str()
                    .map(|value| bounded(value.to_owned())),
                port: item["port"]
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok()),
            }
        })
        .collect::<Vec<_>>();
    advertisements.sort_by(|left, right| left.label.cmp(&right.label));
    advertisements.truncate(48);
    let conflicts = topology["conflicts"].as_array().map_or(0, Vec::len);
    (nodes, advertisements, conflicts)
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
        let mut topology =
            serde_json::from_slice::<TopologyView>(&query(binary, &["topology", "--json"])?)
                .map_err(|error| format!("decode topology: {error}"))?;
        let mut raw_topology =
            serde_json::from_slice::<serde_json::Value>(&query(binary, &["topology", "--json"])?)
                .map_err(|error| format!("decode vantage: {error}"))?;
        if raw_topology["nodes"]
            .as_object()
            .is_none_or(|nodes| nodes.is_empty())
        {
            if let Some(path) = std::env::var_os("MYCELIUM_VANTAGE_SNAPSHOT") {
                let bytes = std::fs::read(path)
                    .map_err(|error| format!("read vantage snapshot: {error}"))?;
                topology = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("decode vantage topology: {error}"))?;
                raw_topology = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("decode vantage snapshot: {error}"))?;
            }
        }
        let (nodes, advertisements, conflicts) = project_vantage(&raw_topology);
        Ok::<_, String>((
            project(peers, now_ms / 1000),
            releases,
            project_segments(topology),
            nodes,
            advertisements,
            conflicts,
        ))
    })();
    let payload = match result {
        Ok((peers, mut releases, segments, nodes, advertisements, conflicts)) => {
            releases.truncate(16);
            for release in &mut releases {
                release.component = bounded(std::mem::take(&mut release.component));
                release.version = bounded(std::mem::take(&mut release.version));
                release.channel = bounded(std::mem::take(&mut release.channel));
                release.target = bounded(std::mem::take(&mut release.target));
            }
            json!({ "peers": peers, "segments": segments, "nodes": nodes, "advertisements": advertisements, "conflicts": conflicts, "releases": releases, "degraded": false })
        }
        Err(error) => {
            json!({ "peers": [], "segments": [], "nodes": [], "advertisements": [], "conflicts": 0, "releases": [], "degraded": true, "error": bounded(error) })
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
                .pointer_mut("/payload/advertisements")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|items| items.pop().is_some())
            || event
                .pointer_mut("/payload/nodes")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|nodes| nodes.len() > 1 && nodes.pop().is_some())
            || event
                .pointer_mut("/payload/segments")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|segments| segments.pop().is_some())
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
            "hello": {"node_id":"abc","hostname":"home-pi","site":"home","platform":"linux","architecture":"aarch64","daemon_version":"0.1.0"},
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
            "id": "x".repeat(64), "hostname": "x".repeat(64), "site": "x".repeat(64),
            "platform": "x".repeat(64), "architecture": "x".repeat(64),
            "version": "x".repeat(64), "age_seconds": 1, "load_1": 1.0,
            "logical_cpus": 4, "memory_available_bytes": 1,
            "memory_total_bytes": 2, "root_available_bytes": 1,
            "root_total_bytes": 2, "ssh_listening": true
        });
        let event = json!({
            "schema_version": 1, "owner": OWNER, "stream": STREAM,
            "contract": "canvas.widget.snapshot.v1", "sequence": 1, "emitted_at_ms": 1,
            "payload": { "peers": vec![peer; 12], "segments": [], "releases": [], "degraded": false }
        });
        let fitted = fit_snapshot(event);
        assert!(
            serde_json::to_vec(&fitted).unwrap().len()
                <= canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD
        );
        assert_eq!(fitted.pointer("/payload/truncated"), Some(&json!(true)));
    }
}
