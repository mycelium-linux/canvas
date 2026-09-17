//! `canvas-alfalfa` binary: push Alfalfa fleet and per-project pulses onto
//! the Canvas stream socket.
//!
//!   canvas-alfalfa [--url http://127.0.0.1:8787] [--every 30]
//!                  [--marbles 6] [--prs 4] [--pulses JARVIS,dock]
//!
//! Overview every cycle; pulses for the chosen projects (default: everything
//! Alfalfa marks active). Sequence numbers seed from the wall clock so a
//! producer restart is never silently deduped — the lesson dock-8gr taught
//! the hard way. A cycle that fails leaves the last snapshot on the wall and
//! says so on stderr; widgets never show fabricated freshness.

use canvas_alfalfa::{OWNER, dashboard_stream, overview_payload, pulse_payload, pulse_stream};
use canvas_feeds::{
    CONTRACT, ENVELOPE_OVERHEAD, Http, MAX_DATAGRAM, ReqwestHttp, SCHEMA_VERSION, fit, now_ms,
    send_raw,
};
use serde_json::{Value, json};
use std::{
    env,
    io::Read,
    process::{Command, ExitCode, Stdio},
    thread,
    time::Duration,
};

const ARG_ERR: &str = "usage: canvas-alfalfa [--url URL] [--bridge-bin PATH] [--alfalfa-port PORT] [--every SECS] [--marbles N] [--prs N] [--pulses id,id,...] [--runtime-dir PATH]";
const MAX_BRIDGE_RESPONSE: u64 = 4 * 1024 * 1024;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn envelope(stream: &str, payload: Value, sequence: u64, emitted: u64) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "owner": OWNER,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": emitted,
        "payload": payload,
    })
}

fn send(stream: &str, payload: Value, sequence: u64) -> Result<(), String> {
    let event = envelope(stream, payload, sequence, now_ms());
    let (fitted, _) = fit(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD));
    send_raw(&fitted).map_err(|error| error.to_string())
}

fn dashboard_envelope(workspace_id: &str, payload: Value, sequence: u64) -> Value {
    let mut event = envelope(&dashboard_stream(workspace_id), payload, sequence, now_ms());
    event["project_id"] = Value::String(workspace_id.to_string());
    event
}

fn fetch_dashboards(bridge: &str, port: u16) -> Result<Vec<Value>, String> {
    let mut child = Command::new(bridge)
        .args([
            "project-read",
            "workspace-dashboards",
            "--port",
            &port.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start Desktop Bridge: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Desktop Bridge stdout unavailable")?;
    let mut bytes = Vec::new();
    stdout
        .take(MAX_BRIDGE_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Desktop Bridge: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for Desktop Bridge: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Desktop Bridge failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if bytes.len() as u64 > MAX_BRIDGE_RESPONSE {
        return Err("Desktop Bridge response exceeds 4 MiB".into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse Desktop Bridge response: {error}"))?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("ok").and_then(Value::as_bool) != Some(true)
    {
        return Err("Desktop Bridge returned an unsuccessful owner envelope".into());
    }
    value
        .pointer("/data/workspaces")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "dashboard projection omitted data.workspaces".into())
}

fn fetch_repos(http: &dyn Http, url: &str) -> Result<Vec<Value>, String> {
    let value = http
        .get_json(&format!("{url}/api/v1/repos"))
        .map_err(|error| error.to_string())?;
    value
        .as_array()
        .cloned()
        .ok_or_else(|| "Alfalfa /api/v1/repos did not answer with an array".to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{ARG_ERR}");
        return ExitCode::SUCCESS;
    }
    if arg(&args, "--runtime-dir").is_some() {
        // send_raw resolves through canvas_core; the flag is accepted for
        // symmetry with other producers and validated for absence of surprises.
        eprintln!(
            "canvas-alfalfa: use CANVAS_CONFIG_DIR for the stream socket; --runtime-dir is ignored"
        );
    }
    let url = arg(&args, "--url").unwrap_or_else(|| "http://127.0.0.1:8787".into());
    let bridge = arg(&args, "--bridge-bin").unwrap_or_else(|| "fpl-desktop-bridge".into());
    let alfalfa_port = arg(&args, "--alfalfa-port")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8791);
    let every = arg(&args, "--every")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 5)
        .unwrap_or(30);
    let marbles = arg(&args, "--marbles")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| (1..=12).contains(v))
        .unwrap_or(6);
    let prs = arg(&args, "--prs")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| (0..=8).contains(v))
        .unwrap_or(4);
    let wanted: Option<Vec<String>> = arg(&args, "--pulses").map(|list| {
        list.split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect()
    });
    let http = ReqwestHttp::default();
    let mut sequence: u64 = now_ms();
    eprintln!("canvas-alfalfa: {url} every {every}s (Ctrl-C to stop)");

    loop {
        sequence += 1;
        match fetch_dashboards(&bridge, alfalfa_port) {
            Ok(workspaces) => {
                for workspace in workspaces {
                    let Some(id) = workspace.get("workspace_id").and_then(Value::as_str) else {
                        eprintln!("canvas-alfalfa: dashboard omitted workspace_id");
                        continue;
                    };
                    let event = dashboard_envelope(id, workspace.clone(), sequence);
                    let (fitted, _) = fit(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD));
                    if let Err(error) = send_raw(&fitted) {
                        eprintln!("canvas-alfalfa: {id} dashboard send failed: {error}");
                    }
                }
            }
            Err(error) => eprintln!("canvas-alfalfa: dashboards failed: {error}"),
        }
        let repos = match fetch_repos(&http, &url) {
            Ok(repos) => repos,
            Err(error) => {
                eprintln!("canvas-alfalfa: roster failed: {error}");
                thread::sleep(Duration::from_secs(every));
                continue;
            }
        };
        let overview = overview_payload(&repos, 25);
        if let Err(error) = send("overview", overview.clone(), sequence) {
            eprintln!("canvas-alfalfa: overview send failed: {error}");
        }
        let pulses: Vec<&Value> = match &wanted {
            Some(ids) => repos
                .iter()
                .filter(|repo| {
                    repo.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| ids.iter().any(|want| want.eq_ignore_ascii_case(id)))
                })
                .collect(),
            // Default: every project gets a pulse stream — tiles bind to it
            // only in workspaces that actually mount one, so idle streams
            // cost nothing but a datagram, and opening any project's
            // workspace shows its marbles instantly.
            None => repos.iter().take(24).collect(),
        };
        let mut sent = 0;
        for repo in pulses {
            let Some(id) = repo.get("id").and_then(Value::as_str) else {
                continue;
            };
            // The roster list already embeds each repo's beads and PRs; no
            // second fetch, so overview and pulse cannot disagree.
            let payload = pulse_payload(repo, marbles, prs);
            match send(&pulse_stream(id), payload, sequence) {
                Ok(()) => sent += 1,
                Err(error) => eprintln!("canvas-alfalfa: {id} pulse send failed: {error}"),
            }
        }
        eprintln!(
            "canvas-alfalfa: {} projects, {sent} pulses refreshed",
            repos.len()
        );
        thread::sleep(Duration::from_secs(every));
    }
}
