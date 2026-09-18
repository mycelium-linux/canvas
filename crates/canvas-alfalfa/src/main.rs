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
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    io::{Read, Write},
    path::Path,
    process::{Command, ExitCode, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const ARG_ERR: &str = "usage: canvas-alfalfa [--url URL] [--bridge-bin PATH] [--alfalfa-port PORT] [--blitz-bin PATH --blitz-template PATH --blitz-workspaces id,id --blitz-width PX --blitz-height PX --blitz-scale N] [--every SECS] [--marbles N] [--prs N] [--pulses id,id,...] [--runtime-dir PATH]";
const MAX_BRIDGE_RESPONSE: u64 = 4 * 1024 * 1024;
const MAX_THEME_BYTES: u64 = 64 * 1024;

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

/// The stream contract carries the scalar state native widgets consume. Rich
/// lists and theme data stay in the per-workspace Blitz render input; sending
/// that whole document over AF_UNIX both duplicated ownership and exceeded the
/// datagram ceiling on larger projects.
fn dashboard_stream_payload(workspace: &Value) -> Value {
    let scalars = |value: Option<&Value>| {
        Value::Object(
            value
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter(|(_, value)| !value.is_array() && !value.is_object())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    };
    let mut agents = scalars(workspace.get("agents"));
    let lineage = workspace
        .pointer("/agents/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(12)
        .filter_map(|agent| {
            let object = agent.as_object()?;
            Some(Value::Object(
                [
                    "id",
                    "session_id",
                    "parent_id",
                    "parent_session_id",
                    "label",
                    "role",
                    "kind",
                    "state",
                    "status",
                ]
                .into_iter()
                .filter_map(|key| object.get(key).map(|value| (key.to_owned(), value.clone())))
                .filter(|(_, value)| !value.is_array() && !value.is_object())
                .collect(),
            ))
        })
        .collect::<Vec<_>>();
    if !lineage.is_empty() {
        agents["items"] = Value::Array(lineage);
    }
    json!({
        "workspace_id": workspace.get("workspace_id"),
        "name": workspace.get("name"),
        "project_count": workspace.get("project_count"),
        "activity_state": workspace.get("activity_state"),
        "goals": scalars(workspace.get("goals")),
        "beads": scalars(workspace.get("beads")),
        "agents": agents,
        "reviewers": scalars(workspace.get("reviewers")),
    })
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
    let source_id = blitz_source_id(workspace_id);
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

fn blitz_source_id(workspace_id: &str) -> String {
    format!(
        "{}-dashboard-html",
        dashboard_stream(workspace_id).trim_end_matches("-dashboard")
    )
}

fn blitz_signature(
    template: &str,
    payload: &Value,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"canvas.blitz.workspace.v1\0");
    digest.update(std::fs::read(template).map_err(|error| format!("read {template}: {error}"))?);
    digest.update(serde_json::to_vec(payload).map_err(|error| error.to_string())?);
    digest.update(width.to_le_bytes());
    digest.update(height.to_le_bytes());
    digest.update(scale.to_bits().to_le_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

fn file_signature(path: &str) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?);
    Ok(format!("{:x}", digest.finalize()))
}

fn blitz_cache_file(root: &Path, workspace_id: &str) -> std::path::PathBuf {
    root.join("cache")
        .join("blitz")
        .join(format!("{workspace_id}.sha256"))
}

fn blitz_cache_hit(root: &Path, workspace_id: &str, signature: &str) -> bool {
    let source_id = blitz_source_id(workspace_id);
    canvas_core::frame_file(root, &source_id).is_some_and(|path| path.is_file())
        && std::fs::read_to_string(blitz_cache_file(root, workspace_id))
            .is_ok_and(|cached| cached == signature)
}

fn record_blitz_cache(root: &Path, workspace_id: &str, signature: &str) -> Result<(), String> {
    canvas_core::write_atomic(&blitz_cache_file(root, workspace_id), signature.as_bytes())
        .map_err(|error| format!("persist {workspace_id} Blitz cache: {error}"))
}

fn attach_canvas_theme(workspace_id: &str, workspace: &mut Value) -> Result<(), String> {
    let root = canvas_core::runtime_root().ok_or("Canvas runtime root unavailable")?;
    attach_canvas_theme_from(&root, workspace_id, workspace)
}

fn attach_canvas_theme_from(
    root: &Path,
    workspace_id: &str,
    workspace: &mut Value,
) -> Result<(), String> {
    if !canvas_core::is_safe_frame_id(workspace_id) {
        return Err("unsafe workspace id for theme snapshot".into());
    }
    let path = root.join("themes").join(format!("{workspace_id}.json"));
    let file =
        std::fs::File::open(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_THEME_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err("Canvas theme snapshot exceeds 64 KiB".into());
    }
    workspace["theme"] =
        serde_json::from_slice(&bytes).map_err(|error| format!("parse Canvas theme: {error}"))?;
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
    let mut rendered_dashboards: BTreeMap<String, String> = BTreeMap::new();
    let latest_blitz_payloads: Arc<Mutex<BTreeMap<String, Value>>> = Default::default();
    if let Some((binary, template)) = blitz.clone() {
        let payloads = Arc::clone(&latest_blitz_payloads);
        let allowed = blitz_workspaces.clone();
        thread::spawn(move || {
            let mut seen = file_signature(&template).ok();
            loop {
                thread::sleep(Duration::from_millis(250));
                let Ok(signature) = file_signature(&template) else {
                    continue;
                };
                if seen.as_ref() == Some(&signature) {
                    continue;
                }
                seen = Some(signature);
                let snapshot = payloads
                    .lock()
                    .map(|payloads| payloads.clone())
                    .unwrap_or_else(|_| {
                        eprintln!("canvas-alfalfa: Blitz live-reload payload cache poisoned");
                        BTreeMap::new()
                    });
                let active = canvas_core::runtime_root()
                    .and_then(|root| {
                        std::fs::read_to_string(root.join("state/active-project")).ok()
                    })
                    .map(|id| id.trim().to_string());
                let mut snapshot = snapshot.into_iter().collect::<Vec<_>>();
                snapshot.sort_by_key(|(id, _)| active.as_ref().is_none_or(|active| active != id));
                if blitz_scale > 1.0
                    && let Some((id, payload)) = snapshot.first()
                    && (allowed.is_empty() || allowed.iter().any(|wanted| wanted == id))
                    && let Err(error) = render_blitz(
                        &binary,
                        &template,
                        id,
                        payload,
                        blitz_width,
                        blitz_height,
                        1.0,
                    )
                {
                    eprintln!("canvas-alfalfa: {id} HTML preview reload failed: {error}");
                }
                for (id, payload) in snapshot {
                    if !allowed.is_empty() && !allowed.iter().any(|wanted| wanted == &id) {
                        continue;
                    }
                    match render_blitz(
                        &binary,
                        &template,
                        &id,
                        &payload,
                        blitz_width,
                        blitz_height,
                        blitz_scale,
                    ) {
                        Ok(()) => {
                            if let (Some(root), Ok(key)) = (
                                canvas_core::runtime_root(),
                                blitz_signature(
                                    &template,
                                    &payload,
                                    blitz_width,
                                    blitz_height,
                                    blitz_scale,
                                ),
                            ) && let Err(error) = record_blitz_cache(&root, &id, &key)
                            {
                                eprintln!("canvas-alfalfa: {error}");
                            }
                        }
                        Err(error) => {
                            eprintln!("canvas-alfalfa: {id} HTML live reload failed: {error}")
                        }
                    }
                }
            }
        });
    }
    eprintln!("canvas-alfalfa: {url} every {every}s (Ctrl-C to stop)");

    loop {
        sequence += 1;
        match fetch_dashboards(&bridge, alfalfa_port) {
            Ok(workspaces) => {
                for mut workspace in workspaces {
                    let Some(id) = workspace.get("workspace_id").and_then(Value::as_str) else {
                        eprintln!("canvas-alfalfa: dashboard omitted workspace_id");
                        continue;
                    };
                    let id = id.to_string();
                    if let Err(error) = attach_canvas_theme(&id, &mut workspace) {
                        eprintln!("canvas-alfalfa: {id} theme unavailable: {error}");
                    }
                    if let Ok(mut payloads) = latest_blitz_payloads.lock() {
                        payloads.insert(id.clone(), workspace.clone());
                    }
                    let event =
                        dashboard_envelope(&id, dashboard_stream_payload(&workspace), sequence);
                    let (fitted, _) = fit(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD));
                    if let Err(error) = send_raw(&fitted) {
                        eprintln!("canvas-alfalfa: {id} dashboard send failed: {error}");
                    }
                    if let Some((binary, template)) = &blitz
                        && (blitz_workspaces.is_empty()
                            || blitz_workspaces.iter().any(|wanted| wanted == &id))
                    {
                        match blitz_signature(
                            template,
                            &workspace,
                            blitz_width,
                            blitz_height,
                            blitz_scale,
                        ) {
                            Ok(signature) => {
                                let root = canvas_core::runtime_root();
                                let cached = rendered_dashboards.get(&id) == Some(&signature)
                                    || root
                                        .as_ref()
                                        .is_some_and(|root| blitz_cache_hit(root, &id, &signature));
                                if cached {
                                    rendered_dashboards.insert(id, signature);
                                } else {
                                    match render_blitz(
                                        binary,
                                        template,
                                        &id,
                                        &workspace,
                                        blitz_width,
                                        blitz_height,
                                        blitz_scale,
                                    ) {
                                        Ok(()) => {
                                            if let Some(root) = &root
                                                && let Err(error) =
                                                    record_blitz_cache(root, &id, &signature)
                                            {
                                                eprintln!("canvas-alfalfa: {error}");
                                            }
                                            rendered_dashboards.insert(id, signature);
                                        }
                                        Err(error) => eprintln!(
                                            "canvas-alfalfa: {id} HTML render failed: {error}"
                                        ),
                                    }
                                }
                            }
                            Err(error) => {
                                eprintln!("canvas-alfalfa: {id} cache key failed: {error}")
                            }
                        }
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
    fn attaches_only_the_requested_workspace_theme() {
        let root = std::env::temp_dir().join(format!(
            "canvas-alfalfa-theme-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("themes")).unwrap();
        std::fs::write(
            root.join("themes/ohm.json"),
            br##"{"schema_version":1,"project_id":"ohm","palette":{"primary":"#9aafff"}}"##,
        )
        .unwrap();
        let mut workspace = json!({"id":"ohm"});

        attach_canvas_theme_from(&root, "ohm", &mut workspace).unwrap();

        assert_eq!(workspace["theme"]["project_id"], "ohm");
        assert_eq!(workspace["theme"]["palette"]["primary"], "#9aafff");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blitz_cache_is_workspace_scoped_and_requires_its_frame() {
        let root = std::env::temp_dir().join(format!(
            "canvas-alfalfa-blitz-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ohm_source = blitz_source_id("ohm");
        let ohm_frame = canvas_core::frame_file(&root, &ohm_source).unwrap();
        std::fs::create_dir_all(ohm_frame.parent().unwrap()).unwrap();
        std::fs::write(&ohm_frame, b"jpeg").unwrap();
        record_blitz_cache(&root, "ohm", "abc").unwrap();

        assert!(blitz_cache_hit(&root, "ohm", "abc"));
        assert!(!blitz_cache_hit(&root, "agora", "abc"));
        assert!(!blitz_cache_hit(&root, "ohm", "changed"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dashboard_stream_projection_keeps_metrics_and_bounded_agent_lineage() {
        let payload = json!({
            "workspace_id":"ohm", "name":"Ohm", "activity_state":"observed",
            "goals":{"total":5,"active":2,"items":[{"outcome":"large"}]},
            "beads":{"open":7,"items":[1,2,3]},
            "agents":{"running":1,"items":[{
                "id":"child", "parent_id":"root", "role":"reviewer",
                "state":"working", "private_prompt":"must not cross the bus"
            }]},
            "reviewers":{"running":2},
            "theme":{"palette":{"primary":"#31e6ff"}}
        });
        let projected = dashboard_stream_payload(&payload);
        assert_eq!(projected["goals"]["total"], 5);
        assert_eq!(projected["agents"]["running"], 1);
        assert_eq!(projected["agents"]["items"][0]["parent_id"], "root");
        assert_eq!(projected["agents"]["items"][0]["role"], "reviewer");
        assert!(
            projected["agents"]["items"][0]
                .get("private_prompt")
                .is_none()
        );
        assert!(projected["goals"].get("items").is_none());
        assert!(projected.get("theme").is_none());
        assert!(serde_json::to_vec(&projected).unwrap().len() < 512);
    }

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
