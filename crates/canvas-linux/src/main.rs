use serde_json::{Value, json};
use std::{
    env,
    path::Path,
    process::{Command, ExitCode},
};

mod compositor;
#[cfg(any(test, feature = "opencode"))]
mod control;
use canvas_core::native_control as control_transport;
mod desktop;
mod hypr;
mod launcher;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(all(test, feature = "compositor", target_os = "linux"))]
mod native_probe;
#[cfg(feature = "opencode")]
mod opencode;
#[cfg(all(feature = "compositor", target_os = "linux"))]
mod physical_input;
#[cfg(all(feature = "compositor", target_os = "linux"))]
mod runner;
#[cfg(any(test, all(feature = "compositor", target_os = "linux")))]
mod window_switcher;

fn unit(project: &str) -> Result<String, String> {
    if project.is_empty()
        || project.len() > 64
        || !project
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("project must contain 1–64 lowercase ASCII letters, digits or hyphens".into());
    }
    Ok(format!("dock-project-{project}.service"))
}

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().into())
}

fn status(name: &str) -> Result<Value, String> {
    let output = command(
        "systemctl",
        &[
            "--user",
            "show",
            name,
            "--property=LoadState,ActiveState,SubState,FreezerState,ExecMainStatus,Description",
        ],
    )?;
    let properties: serde_json::Map<String, Value> = output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.into(), json!(v)))
        .collect();
    if properties.get("LoadState").and_then(Value::as_str) == Some("not-found") {
        return Err(format!("project unit {name} does not exist"));
    }
    if properties.get("Description").and_then(Value::as_str) != Some("Dock project process") {
        return Err(format!(
            "refusing unit {name}: missing Dock ownership marker"
        ));
    }
    Ok(json!({"unit":name, "properties":properties}))
}

fn execute(args: &[String]) -> Result<Value, String> {
    if !cfg!(target_os = "linux") {
        return Err("canvas-linux requires Linux with a systemd user session".into());
    }
    let action = args.first().map(String::as_str).unwrap_or("capabilities");
    #[cfg(feature = "mcp")]
    {
        if action == "mcp-ready" && args.len() == 2 {
            return mcp::ready(&args[1]);
        }
        if action == "mcp-serve" {
            if args.len() != 3 {
                return Err("usage: canvas-linux mcp-serve PROJECT PORT".into());
            }
            let port = args[2].parse::<u16>().map_err(|_| "invalid MCP port")?;
            mcp::serve(&args[1], port)?;
            return Ok(json!({"stopped": true}));
        }
    }
    if action == "desktop-compositor" {
        return compositor::execute(args);
    }
    if action == "desktop-compositor-control" && args.len() == 3 {
        let request = serde_json::from_str(&args[2])
            .map_err(|e| format!("invalid compositor command: {e}"))?;
        let response = launcher::send_command(Path::new(&args[1]), &request)?;
        return serde_json::from_str(&response)
            .map_err(|e| format!("invalid compositor response: {e}"));
    }
    if action.starts_with("desktop-") {
        return desktop::execute(args);
    }
    #[cfg(feature = "opencode")]
    {
        if action.starts_with("session-") {
            return opencode::execute(args);
        }
    }
    if action.starts_with("hypr-") {
        return hypr::execute(args);
    }
    if action == "prepare-stream" {
        if args.len() != 2 {
            return Err("usage: canvas-linux prepare-stream SECONDS (30–900)".into());
        }
        let seconds = stream_duration(&args[1])?;
        let limit = format!("--property=RuntimeMaxSec={seconds}");
        let duration = seconds.to_string();
        // A fixed unit prevents accidental accumulation of overlapping idle inhibitors.
        let lease = "dock-stream-preparation.service";
        command(
            "systemd-run",
            &[
                "--user",
                "--collect",
                "--service-type=exec",
                "--unit",
                lease,
                &limit,
                "--",
                "/usr/bin/gnome-session-inhibit",
                "--app-id=dev.fpl.Dock",
                "--reason=Temporary Dock stream preparation",
                "--inhibit=idle",
                "/usr/bin/sleep",
                &duration,
            ],
        )?;
        let wake = command(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.gnome.Mutter.DisplayConfig",
                "--object-path",
                "/org/gnome/Mutter/DisplayConfig",
                "--method",
                "org.freedesktop.DBus.Properties.Set",
                "org.gnome.Mutter.DisplayConfig",
                "PowerSaveMode",
                "<0>",
            ],
        );
        if let Err(error) = wake {
            let cleanup = command("systemctl", &["--user", "stop", lease]);
            return Err(format!(
                "display wake failed: {error}; inhibitor cleanup: {cleanup:?}"
            ));
        }
        command("systemctl", &["--user", "is-active", lease])?;
        return Ok(
            json!({"unit":lease,"expires_after_seconds":seconds,"display_wake_requested":true,
            "note":"GNOME idle inhibition is temporary; does not unlock desktop or verify a working stream"}),
        );
    }
    if action == "capabilities" && args.len() <= 1 {
        let systemd = command("systemctl", &["--user", "show-environment"]);
        let sunshine = command("systemctl", &["--user", "is-active", "sunshine"]);
        let hyprland = hypr::execute(&["hypr-status".into()]).is_ok();
        let mut result = json!({
            "platform":"linux", "systemd_user_available":systemd.is_ok(),
            "systemd_error":systemd.err(), "project_process_control":"systemd user units; freeze/thaw require cgroup v2",
            "desktop_window_control":hyprland, "desktop_window_control_scope":"registered Dock Hyprland instance only",
            "computer_use":false, "computer_use_scope":"legacy Sway capture/input retired; native control is explicit via desktop-compositor-control",
            "native_compositor_compiled":cfg!(all(feature = "compositor", target_os = "linux")),
            "sunshine_running":sunshine.is_ok(),
            "pause_scope":"only processes launched in this project's unit; not the desktop or external sessions"
        });
        #[cfg(feature = "opencode")]
        {
            result["interactive_sessions"] = json!(
                "OpenCode session-ensure/status/prompt/abort/resume/attach; provider availability checked by session-status"
            );
        }
        #[cfg(not(feature = "opencode"))]
        {
            result["interactive_sessions"] = json!(null);
        }
        #[cfg(feature = "mcp")]
        {
            result["mcp_server"] = json!(
                "enabled; canvas-linux mcp-serve PROJECT PORT; canvas-linux mcp-ready PROJECT"
            );
        }
        #[cfg(not(feature = "mcp"))]
        {
            result["mcp_server"] = json!(null);
        }
        return Ok(result);
    }
    let project = args.get(1).ok_or("usage: canvas-linux capabilities | run PROJECT DIRECTORY -- PROGRAM [ARG...] | status/pause/resume/logs PROJECT")?;
    let name = unit(project)?;
    match action {
        "run" => {
            if args.len() < 5 || args[3] != "--" {
                return Err("usage: canvas-linux run PROJECT DIRECTORY -- PROGRAM [ARG...]".into());
            }
            if !Path::new(&args[2]).is_absolute() || !Path::new(&args[2]).is_dir() {
                return Err("DIRECTORY must be an existing absolute directory".into());
            }
            let mut cmd = vec![
                "--user",
                "--collect",
                "--service-type=exec",
                "--description=Dock project process",
                "--property=KillMode=control-group",
                "--unit",
                &name,
                "--working-directory",
                &args[2],
                "--",
            ];
            cmd.extend(args[4..].iter().map(String::as_str));
            command("systemd-run", &cmd)?;
            Ok(
                json!({"unit":name,"accepted":true,"note":"launch accepted; query status for current state"}),
            )
        }
        "status" if args.len() == 2 => status(&name),
        "pause" | "resume" if args.len() == 2 => {
            let before = status(&name)?;
            if before["properties"]["ActiveState"] != "active" {
                return Err("project process must be active".into());
            }
            command(
                "systemctl",
                &[
                    "--user",
                    if action == "pause" { "freeze" } else { "thaw" },
                    &name,
                ],
            )?;
            let after = status(&name)?;
            let expected = if action == "pause" {
                "frozen"
            } else {
                "running"
            };
            if after["properties"]["FreezerState"] != expected {
                return Err(format!(
                    "requested {action}, but systemd did not report {expected}: {after}"
                ));
            }
            Ok(after)
        }
        "logs" if args.len() == 2 => Ok(
            json!({"unit":name,"logs":command("journalctl", &["--user", "--unit", &name, "--no-pager", "--lines=100", "--output=cat"])?}),
        ),
        _ => Err("unknown action or incorrect arguments".into()),
    }
}

fn stream_duration(value: &str) -> Result<u32, String> {
    match value.parse::<u32>() {
        Ok(seconds) if (30..=900).contains(&seconds) => Ok(seconds),
        _ => Err("stream preparation duration must be 30–900 seconds".into()),
    }
}

fn main() -> ExitCode {
    match execute(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(result) => {
            println!("{}", json!({"schema_version":1,"ok":true,"result":result}));
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({"schema_version":1,"ok":false,"error":error}));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unit_injection_and_ambiguous_names() {
        for name in [
            "",
            "../other",
            "a.service",
            "-x y",
            "a\n",
            "A",
            "é",
            "$(id)",
        ] {
            assert!(unit(name).is_err(), "{name}");
        }
        assert!(unit(&"a".repeat(65)).is_err());
    }
    #[test]
    fn units_have_dock_namespace() {
        assert_eq!(unit("dock-42").unwrap(), "dock-project-dock-42.service");
    }
    #[test]
    fn subprocess_failure_is_an_error() {
        assert!(command("/usr/bin/false", &[]).is_err());
        assert!(command("/nonexistent/dock-test", &[]).is_err());
    }

    #[test]
    fn stream_preparation_is_bounded() {
        for value in ["0", "29", "901", "-1", "forever", "1;true"] {
            assert!(stream_duration(value).is_err());
        }
        assert_eq!(stream_duration("30").unwrap(), 30);
        assert_eq!(stream_duration("900").unwrap(), 900);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a live systemd user session and python3"]
    fn live_process_really_stops_and_resumes() {
        use std::{
            fs, thread,
            time::{Duration, SystemTime, UNIX_EPOCH},
        };
        let project = format!(
            "test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let name = unit(&project).unwrap();
        let dir = env::temp_dir().join(&project);
        fs::create_dir(&dir).unwrap();
        struct Cleanup(String, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = command("systemctl", &["--user", "stop", &self.0]);
                let _ = fs::remove_dir_all(&self.1);
            }
        }
        let _cleanup = Cleanup(name, dir.clone());
        let invoke = |v: Vec<&str>| execute(&v.into_iter().map(String::from).collect::<Vec<_>>());
        invoke(vec!["run", &project, dir.to_str().unwrap(), "--", "/usr/bin/python3", "-u", "-c", "import time\nf=open('heartbeat','a',buffering=1)\nwhile True:\n f.write('tick\\n'); time.sleep(.05)"]).unwrap();
        let file = dir.join("heartbeat");
        for _ in 0..100 {
            if file.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        invoke(vec!["pause", &project]).unwrap();
        let frozen = fs::read(&file).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(frozen, fs::read(&file).unwrap());
        invoke(vec!["resume", &project]).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!(fs::read(&file).unwrap().len() > frozen.len());
    }
}
