//! Narrow commands for Dock's explicitly registered Hyprland instance.
use crate::{command, unit};
use serde_json::{json, Value};
use std::{env, fs, path::PathBuf};

fn root_runtime() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        env::var("DOCK_PARENT_RUNTIME")
            .or_else(|_| env::var("XDG_RUNTIME_DIR"))
            .map_err(|_| "XDG_RUNTIME_DIR missing")?,
    ))
}
fn control(args: &[&str]) -> Result<String, String> {
    let value = format!(
        "XDG_RUNTIME_DIR={}",
        root_runtime()?.join("dock-hyprland").display()
    );
    let mut command_args = vec![value.as_str(), "hyprctl"];
    command_args.extend(args);
    command("env", &command_args)
}
fn start() -> Result<Value, String> {
    let config = directory()?.join("hyprland.conf");
    command(
        "Hyprland",
        &[
            "--verify-config",
            "--config",
            config.to_str().ok_or("config path not UTF-8")?,
        ],
    )?;
    // Read the existing session environment without changing the user's manager.
    let environment = command("systemctl", &["--user", "show-environment"])?;
    let display = environment
        .lines()
        .find_map(|line| line.strip_prefix("WAYLAND_DISPLAY="))
        .ok_or("a running parent Wayland session is required")?;
    let runtime = root_runtime()?;
    let private_runtime = runtime.join("dock-hyprland");
    fs::create_dir_all(&private_runtime).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&private_runtime, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let parent_display = if PathBuf::from(display).is_absolute() {
        PathBuf::from(display)
    } else {
        runtime.join(display)
    };
    let display_env = format!("--setenv=WAYLAND_DISPLAY={}", parent_display.display());
    let runtime_env = format!("--setenv=XDG_RUNTIME_DIR={}", private_runtime.display());
    let parent_env = format!("--setenv=DOCK_PARENT_RUNTIME={}", runtime.display());
    if command("systemctl", &["--user", "is-active", "dock-hyprland"]).is_err() {
        command(
            "systemd-run",
            &[
                "--user",
                "--collect",
                "--unit=dock-hyprland",
                &display_env,
                &runtime_env,
                &parent_env,
                "--property=TimeoutStopSec=5",
                "--setenv=HYPRLAND_NO_SD_VARS=1",
                "--setenv=HYPRLAND_NO_SD_NOTIFY=1",
                "--setenv=XDG_CURRENT_DESKTOP=Hyprland",
                "--",
                "/usr/bin/dbus-run-session",
                "--",
                "/usr/bin/Hyprland",
                "--config",
                config.to_str().ok_or("config path not UTF-8")?,
            ],
        )?;
    }
    for _ in 0..100 {
        let raw = control(&["-j", "instances"])?;
        let list: Value = if raw.trim().is_empty()
            || raw.trim() == "]"
            || raw.to_lowercase().contains("no instances")
        {
            json!([])
        } else {
            serde_json::from_str(&raw).map_err(|e| format!("instance response {e}: {raw}"))?
        };
        if let Some(found) = list.as_array().and_then(|items| {
            items.iter().find(|item| {
                item["pid"].as_u64().is_some_and(|p| {
                    fs::read_to_string(format!("/proc/{p}/cgroup"))
                        .is_ok_and(|c| c.contains("/dock-hyprland.service"))
                })
            })
        }) {
            let signature = found["instance"]
                .as_str()
                .ok_or("invalid instance response")?;
            fs::write(
                runtime.join("dock-hyprland.instance"),
                format!("{signature}\n"),
            )
            .map_err(|e| e.to_string())?;
            execute(&["hypr-project".into(), "dock".into()])?;
            return Ok(
                json!({"instance":found,"unit":"dock-hyprland.service","nested":true,"note":"parent desktop remains active; unlock and focus its Hyprland window for human input"}),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("Hyprland startup not confirmed after 10 seconds; inspect journalctl --user -u dock-hyprland".into())
}

fn directory() -> Result<PathBuf, String> {
    Ok(PathBuf::from(env::var("HOME").map_err(|_| "HOME is missing")?).join(".config/dock/hypr"))
}
fn instance() -> Result<String, String> {
    let id = fs::read_to_string(root_runtime()?.join("dock-hyprland.instance"))
        .map_err(|e| format!("Dock Hyprland instance not registered: {e}"))?;
    let id = id.trim();
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return Err("invalid registered Hyprland instance".into());
    }
    Ok(id.into())
}
fn query(id: &str, verb: &str) -> Result<Value, String> {
    serde_json::from_str(&control(&["-i", id, "-j", verb])?)
        .map_err(|e| format!("Hyprland {verb} response: {e}"))
}
fn mutate(id: &str, args: &[&str]) -> Result<(), String> {
    let mut command_args = vec!["-i", id];
    command_args.extend(args);
    let response = control(&command_args)?;
    if response.trim() != "ok" {
        return Err(format!("Hyprland rejected request: {response}"));
    }
    Ok(())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn project_theme(project: &str) -> Result<PathBuf, String> {
    unit(project)?;
    let theme = directory()?.join("themes").join(format!("{project}.conf"));
    if !theme.is_file() {
        return Err(format!("project theme is missing: {}", theme.display()));
    }
    Ok(theme)
}
pub fn execute(args: &[String]) -> Result<Value, String> {
    if args[0] == "hypr-start" && args.len() == 1 {
        return start();
    }
    let id = instance()?;
    match args[0].as_str() {
        "hypr-terminal" if args.len() == 1 => {
            let current = query(&id, "activeworkspace")?;
            let project = current["name"].as_str().ok_or("workspace name missing")?;
            execute(&["hypr-open".into(), project.into(), env::var("HOME").map_err(|_| "HOME missing")?])
        }
        "hypr-status" if args.len() == 1 => Ok(json!({"instance":id,"active_workspace":query(&id,"activeworkspace")?,
            "workspaces":query(&id,"workspaces")?,"clients":query(&id,"clients")?,"config_errors":query(&id,"configerrors")?})),
        "hypr-project" if args.len() == 2 => {
            let project = &args[1];
            let theme = project_theme(project)?;
            mutate(&id,&["keyword","source",theme.to_str().ok_or("theme path is not UTF-8")?])?;
            let errors = query(&id,"configerrors")?;
            if errors.as_array().is_none_or(|v| v.iter().any(|e| e.as_str().is_none_or(|s| !s.trim().is_empty()))) { return Err(format!("Hyprland config errors: {errors}")); }
            mutate(&id,&["dispatch","workspace",&format!("name:{project}")])?;
            let current = query(&id,"activeworkspace")?;
            if current["name"] != project.as_str() { return Err(format!("workspace activation unconfirmed: {current}")); }
            Ok(json!({"instance":id,"active_workspace":current,"theme_source":theme,"theme_applied":true}))
        }
        "hypr-open" if args.len() == 3 => {
            let project = &args[1];
            project_theme(project)?;
            let cwd = PathBuf::from(&args[2]);
            if !cwd.is_absolute() || !cwd.is_dir() { return Err("directory must exist and be absolute".into()); }
            let config = directory()?.join(format!("foot-{project}.ini"));
            if !config.is_file() { return Err(format!("terminal theme missing: {}",config.display())); }
            let launch = format!("[workspace name:{project} silent] foot --config={} --working-directory={}",quote(config.to_str().ok_or("config path not UTF-8")?),quote(&args[2]));
            mutate(&id,&["dispatch","exec",&launch])?;
            Ok(json!({"instance":id,"project":project,"launch_accepted":true,"note":"query hypr-status to verify the client appeared"}))
        }
        "hypr-focus" if args.len() == 2 => {
            let address = &args[1];
            let clients = query(&id,"clients")?;
            if !clients.as_array().ok_or("invalid client list")?.iter().any(|c|c["address"] == address.as_str()) { return Err("address is not a live client in the Dock compositor".into()); }
            mutate(&id,&["dispatch","focuswindow",&format!("address:{address}")])?;
            let active = query(&id,"activewindow")?;
            if active["address"] != address.as_str() { return Err("focus acknowledgement mismatch".into()); }
            Ok(active)
        }
        _ => Err("usage: hypr-status | hypr-project PROJECT | hypr-open PROJECT DIRECTORY | hypr-focus ADDRESS".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_arguments_preserve_metacharacters() {
        assert_eq!(quote("/tmp/a'b $(id)"), "'/tmp/a'\\''b $(id)'");
    }
}
