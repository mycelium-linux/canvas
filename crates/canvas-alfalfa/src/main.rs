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
    collections::BTreeMap,
    env,
    io::{Read, Write},
    process::{Command, ExitCode, Stdio},
    thread,
    time::Duration,
};

const ARG_ERR: &str = "usage: canvas-alfalfa [--url URL] [--bridge-bin PATH] [--alfalfa-port PORT] [--blitz-bin PATH --blitz-template PATH --blitz-workspaces id,id --blitz-width PX --blitz-height PX --blitz-scale N] [--every SECS] [--marbles N] [--prs N] [--pulses id,id,...] [--runtime-dir PATH]";
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

fn bridge_read(bridge: &str, port: u16, action: &str) -> Result<Value, String> {
    let mut child = Command::new(bridge)
        .args(["project-read", action, "--port", &port.to_string()])
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
        .get("data")
        .cloned()
        .ok_or_else(|| format!("Desktop Bridge {action} omitted data"))
}

fn catalog_dashboards(workspaces: &[Value], projects: &[Value]) -> Vec<Value> {
    let mut dashboards: BTreeMap<String, Value> = workspaces
        .iter()
        .filter_map(|workspace| {
            let id = workspace.get("id")?.as_str()?;
            Some((
                id.to_string(),
                json!({
                    "workspace_id": id,
                    "name": workspace.get("name").and_then(Value::as_str).unwrap_or(id),
                    "project_count": 0,
                    "project_ids": [],
                    "goals": {"total": 0, "active": 0, "queued": 0, "ideas": 0, "items": []},
                    "beads": {"open": 0, "in_progress": 0, "blocked": 0, "ready": 0, "linked": 0, "items": []},
                    "agents": {"total": 0, "running": 0, "items": []},
                    "reviewers": {"total": 0, "running": 0},
                    "activity_state": "unavailable",
                }),
            ))
        })
        .collect();
    for project in projects {
        let Some(workspace_id) = project.pointer("/workspace/id").and_then(Value::as_str) else {
            continue;
        };
        let Some(dashboard) = dashboards.get_mut(workspace_id) else {
            continue;
        };
        if let Some(project_id) = project.pointer("/project/id").and_then(Value::as_str) {
            dashboard["project_count"] =
                json!(dashboard["project_count"].as_u64().unwrap_or(0) + 1);
            let ids = dashboard["project_ids"]
                .as_array_mut()
                .expect("catalog projection project_ids remains an array");
            if ids.len() < 12 {
                ids.push(json!(project_id));
            }
        }
        for goal in project
            .get("goals")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            dashboard["goals"]["total"] =
                json!(dashboard["goals"]["total"].as_u64().unwrap_or(0) + 1);
            let status = goal
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if matches!(status, "active" | "queued") {
                let count = dashboard["goals"][status].as_u64().unwrap_or(0) + 1;
                dashboard["goals"][status] = json!(count);
            }
            if status == "idea" {
                dashboard["goals"]["ideas"] =
                    json!(dashboard["goals"]["ideas"].as_u64().unwrap_or(0) + 1);
            }
            let linked = goal
                .get("beads")
                .and_then(Value::as_array)
                .map_or(0, |beads| beads.len() as u64);
            dashboard["beads"]["linked"] =
                json!(dashboard["beads"]["linked"].as_u64().unwrap_or(0) + linked);
            let items = dashboard["goals"]["items"]
                .as_array_mut()
                .expect("catalog projection goal items remains an array");
            if items.len() < 12 {
                items.push(json!({
                    "id": goal.get("id").and_then(Value::as_str).unwrap_or("?"),
                    "outcome": goal.get("outcome").and_then(Value::as_str).unwrap_or("Goal"),
                    "status": status,
                }));
            }
        }
    }
    dashboards.into_values().collect()
}

fn fetch_dashboards(bridge: &str, port: u16) -> Result<Vec<Value>, String> {
    match bridge_read(bridge, port, "workspace-dashboards") {
        Ok(value) => value
            .get("workspaces")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| "dashboard projection omitted workspaces".into()),
        Err(owner_error) => {
            let workspaces = bridge_read(bridge, port, "workspaces")?;
            let projects = bridge_read(bridge, port, "list")?;
            let workspaces = workspaces
                .as_array()
                .ok_or("Alfalfa workspaces did not answer with an array")?;
            let projects = projects
                .as_array()
                .ok_or("Alfalfa projects did not answer with an array")?;
            eprintln!(
                "canvas-alfalfa: owner dashboard unavailable ({owner_error}); using catalog-only projection"
            );
            Ok(catalog_dashboards(workspaces, projects))
        }
    }
}

fn render_blitz(
    binary: &str,
    template: &str,
    workspace_id: &str,
    payload: &Value,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<(), String> {
    let source_id = format!(
        "{}-dashboard-html",
        dashboard_stream(workspace_id).trim_end_matches("-dashboard")
    );
    let mut child = Command::new(binary)
        .args([
            "dashboard",
            template,
            &source_id,
            &width.to_string(),
            &height.to_string(),
            &scale.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start Blitz renderer: {error}"))?;
    let bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Blitz stdin unavailable")?
        .write_all(&bytes)
        .map_err(|error| format!("write Blitz snapshot: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for Blitz renderer: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Blitz renderer failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
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
    let blitz = arg(&args, "--blitz-bin").zip(arg(&args, "--blitz-template"));
    let blitz_workspaces: Vec<String> = arg(&args, "--blitz-workspaces")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let blitz_width = arg(&args, "--blitz-width")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (320..=3840).contains(value))
        .unwrap_or(900);
    let blitz_height = arg(&args, "--blitz-height")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (320..=2160).contains(value))
        .unwrap_or(1080);
    let blitz_scale = arg(&args, "--blitz-scale")
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| (1.0..=3.0).contains(value))
        .unwrap_or(2.0);
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
    let mut rendered_dashboards: BTreeMap<String, (Value, Option<std::time::SystemTime>)> =
        BTreeMap::new();
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
                    if let Some((binary, template)) = &blitz
                        && (blitz_workspaces.is_empty()
                            || blitz_workspaces.iter().any(|wanted| wanted == id))
                        && rendered_dashboards.get(id)
                            != Some(&(
                                workspace.clone(),
                                std::fs::metadata(template)
                                    .and_then(|metadata| metadata.modified())
                                    .ok(),
                            ))
                        && let Err(error) = render_blitz(
                            binary,
                            template,
                            id,
                            &workspace,
                            blitz_width,
                            blitz_height,
                            blitz_scale,
                        )
                    {
                        eprintln!("canvas-alfalfa: {id} HTML render failed: {error}");
                    } else if blitz.is_some() {
                        rendered_dashboards.insert(
                            id.to_string(),
                            (
                                workspace.clone(),
                                blitz.as_ref().and_then(|(_, template)| {
                                    std::fs::metadata(template)
                                        .and_then(|metadata| metadata.modified())
                                        .ok()
                                }),
                            ),
                        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_fallback_shows_real_goals_and_marks_activity_unavailable() {
        let workspaces = json!([{"id":"ohm","name":"Ohm"}]);
        let projects = json!([
            {"project":{"id":"amp"},"workspace":{"id":"ohm"},"goals":[
                {"id":"ship","outcome":"Ship it","status":"active","beads":["amp-1"]},
                {"id":"later","outcome":"Later","status":"queued"},
                {"id":"idea","outcome":"Maybe","status":"idea"}
            ]},
            {"project":{"id":"meter"},"workspace":{"id":"ohm"},"goals":[]}
        ]);
        let dashboards =
            catalog_dashboards(workspaces.as_array().unwrap(), projects.as_array().unwrap());
        assert_eq!(dashboards.len(), 1);
        assert_eq!(dashboards[0]["project_ids"], json!(["amp", "meter"]));
        assert_eq!(dashboards[0]["goals"]["total"], 3);
        assert_eq!(dashboards[0]["goals"]["active"], 1);
        assert_eq!(dashboards[0]["goals"]["queued"], 1);
        assert_eq!(dashboards[0]["goals"]["ideas"], 1);
        assert_eq!(dashboards[0]["beads"]["linked"], 1);
        assert_eq!(dashboards[0]["activity_state"], "unavailable");
    }
}
