//! Independent Sway desktop, real capture/input, and cooperative human takeover.
use crate::{command, control, unit};
#[path = "desktop_pointer.rs"]
mod pointer;
use base64::Engine;
use canvas_core::desktop::{Wallpaper, WallpaperBackend, WallpaperReceipt, WallpaperRequest};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn home() -> Result<PathBuf, String> {
    Ok(PathBuf::from(env::var("HOME").map_err(|_| "HOME missing")?))
}

fn start() -> Result<Value, String> {
    let root = PathBuf::from(
        env::var("DOCK_PARENT_RUNTIME")
            .or_else(|_| env::var("XDG_RUNTIME_DIR"))
            .map_err(|_| "XDG_RUNTIME_DIR missing")?,
    );
    let runtime = root.join("dock-desktop");
    fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let config = home()?.join(".config/dock/sway/config");
    if !config.is_file() {
        return Err("Sway desktop config is not installed".into());
    }
    if command("systemctl", &["--user", "is-active", "dock-desktop"]).is_err() {
        command(
            "systemd-run",
            &[
                "--user",
                "--collect",
                "--unit=dock-desktop",
                "--property=TimeoutStopSec=5",
                &format!("--setenv=XDG_RUNTIME_DIR={}", runtime.display()),
                &format!("--setenv=DOCK_PARENT_RUNTIME={}", root.display()),
                "--setenv=WLR_BACKENDS=headless",
                "--setenv=WLR_LIBINPUT_NO_DEVICES=1",
                "--setenv=WLR_RENDERER=pixman",
                "--setenv=XDG_CURRENT_DESKTOP=sway",
                "--",
                "/usr/bin/dbus-run-session",
                "--",
                "/usr/bin/sway",
                "--unsupported-gpu",
                "--config",
                config.to_str().ok_or("config path not UTF8")?,
            ],
        )?;
    }
    for _ in 0..100 {
        let entries = fs::read_dir(&runtime)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let socket = entries.iter().find(|entry| {
            entry.file_name().to_string_lossy().starts_with("sway-ipc.")
                && entry.file_name().to_string_lossy().ends_with(".sock")
        });
        let display = entries.iter().find(|entry| {
            entry.file_name().to_string_lossy().starts_with("wayland-")
                && !entry.file_name().to_string_lossy().ends_with(".lock")
        });
        if let (Some(socket), Some(display)) = (socket, display) {
            let path = home()?.join(".local/state/dock/desktop.json");
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let value = json!({"runtime":runtime,"sway_socket":socket.path(),"wayland_display":display.file_name().to_string_lossy()});
            fs::write(&path, value.to_string()).map_err(|e| e.to_string())?;
            sway("get_version", None)?;
            let vnc_config = home()?.join(".config/dock/wayvnc/config");
            if !vnc_config.is_file() {
                return Err("authenticated wayvnc config not installed; desktop started but transport not configured".into());
            }
            if command("systemctl", &["--user", "is-active", "dock-wayvnc"]).is_err() {
                let upgraded = home()?.join(".local/bin/dock-wayvnc");
                let server = if upgraded.is_file() {
                    upgraded.to_str().ok_or("transport path not UTF8")?
                } else {
                    "/usr/bin/wayvnc"
                };
                command(
                    "systemd-run",
                    &[
                        "--user",
                        "--collect",
                        "--unit=dock-wayvnc",
                        "--property=TimeoutStopSec=5",
                        &format!("--setenv=XDG_RUNTIME_DIR={}", runtime.display()),
                        &format!(
                            "--setenv=WAYLAND_DISPLAY={}",
                            display.file_name().to_string_lossy()
                        ),
                        "--",
                        server,
                        "--config",
                        vnc_config.to_str().ok_or("VNC config path not UTF8")?,
                    ],
                )?;
            }
            return Ok(
                json!({"desktop":value,"active_workspace":recall("dock")?,"transport":"wayvnc over SSH localhost:5901"}),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("desktop startup timed out; inspect journalctl --user -u dock-desktop".into())
}
fn state() -> Result<Value, String> {
    let raw = fs::read_to_string(home()?.join(".local/state/dock/desktop.json"))
        .map_err(|e| format!("desktop not registered: {e}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    for key in ["runtime", "sway_socket", "wayland_display"] {
        if value[key].as_str().is_none_or(str::is_empty) {
            return Err(format!("desktop state lacks {key}"));
        }
    }
    if !Path::new(value["runtime"].as_str().unwrap()).is_absolute()
        || !Path::new(value["sway_socket"].as_str().unwrap()).is_absolute()
    {
        return Err("registered desktop paths must be absolute".into());
    }
    Ok(value)
}
fn sway(kind: &str, message: Option<&str>) -> Result<Value, String> {
    let s = state()?;
    let mut args = vec!["-s", s["sway_socket"].as_str().unwrap(), "-r", "-t", kind];
    if let Some(message) = message {
        args.push(message);
    }
    let raw = command("swaymsg", &args)?;
    let result: Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid Sway response: {e}"))?;
    if kind == "command"
        && result.as_array().is_none_or(|items| {
            items.is_empty() || items.iter().any(|item| item["success"] != true)
        })
    {
        return Err(format!("Sway rejected command: {result}"));
    }
    Ok(result)
}
fn wayland(program: &str, args: &[&str]) -> Result<String, String> {
    let s = state()?;
    let runtime = format!("XDG_RUNTIME_DIR={}", s["runtime"].as_str().unwrap());
    let display = format!("WAYLAND_DISPLAY={}", s["wayland_display"].as_str().unwrap());
    let mut all = vec!["10", "env", &runtime, &display, program];
    all.extend(args);
    command("timeout", &all)
}
fn active(project: &str) -> Result<Value, String> {
    unit(project)?;
    let workspaces = sway("get_workspaces", None)?;
    let current = workspaces
        .as_array()
        .and_then(|items| items.iter().find(|w| w["focused"] == true))
        .ok_or("no active project workspace")?;
    if current["name"] != project {
        return Err(format!(
            "active workspace is {}, requested {project}; recall it explicitly first",
            current["name"]
        ));
    }
    Ok(current.clone())
}
fn project_output<'a>(workspace: &Value, outputs: &'a Value) -> Result<&'a Value, String> {
    let name = workspace["output"]
        .as_str()
        .ok_or("project output missing")?;
    outputs
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|output| output["name"] == name && output["active"] == true)
        })
        .ok_or_else(|| "project output missing or inactive".into())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn sway_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn recall(project: &str) -> Result<Value, String> {
    unit(project)?;
    let theme_root = home()?.join(".config/dock/sway/themes");
    let (theme, theme_source) = theme_path(&theme_root, project)?;
    let source = fs::read_to_string(&theme).map_err(|e| e.to_string())?;
    let plan = recall_commands(project, &source)?;

    sway("command", Some(&plan.commands.join("; ")))?;
    let active_workspace = active(project)?;
    let output = active_workspace["output"]
        .as_str()
        .ok_or("project output missing")?;
    let wallpaper = SwayDesktop.set_wallpaper(&WallpaperRequest {
        project: project.into(),
        display_id: output.into(),
        wallpaper: plan.wallpaper,
    })?;
    let tree = sway("get_tree", None)?;
    verify_workspace_layout(&tree, project, "tabbed")?;
    Ok(
        json!({"project":project,"active_workspace":active_workspace,"theme_applied":true,"theme_source":theme_source,"wallpaper":wallpaper,"layout":"tabbed","compositor":"sway","transport":"wayvnc via SSH"}),
    )
}

fn theme_path(root: &Path, project: &str) -> Result<(PathBuf, &'static str), String> {
    let project_theme = root.join(format!("{project}.conf"));
    if project_theme.is_file() {
        return Ok((project_theme, "project"));
    }
    let fallback = root.join("dock.conf");
    if fallback.is_file() {
        Ok((fallback, "default"))
    } else {
        Err("default generated project theme is missing".into())
    }
}

struct RecallPlan {
    commands: Vec<String>,
    wallpaper: Wallpaper,
}

fn recall_commands(project: &str, source: &str) -> Result<RecallPlan, String> {
    let mut commands = vec![
        format!("workspace {}", sway_quote(project)),
        "layout tabbed".into(),
    ];
    let mut wallpaper = None;
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if line.contains(';')
            || ![
                "client.focused ",
                "client.unfocused ",
                "client.focused_inactive ",
                "output ",
                "gaps ",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        {
            return Err("unsupported command in generated Sway theme".into());
        }
        if line.starts_with("output ") {
            if wallpaper.is_some() {
                return Err("generated Sway theme contains multiple wallpapers".into());
            }
            wallpaper = Some(parse_solid_wallpaper(line)?);
        } else if let Some(value) = line.strip_prefix("gaps inner ") {
            commands.push(format!("gaps inner current set {value}"));
        } else if let Some(value) = line.strip_prefix("gaps outer ") {
            commands.push(format!("gaps outer current set {value}"));
        } else {
            commands.push(line.to_owned());
        }
    }
    Ok(RecallPlan {
        commands,
        wallpaper: wallpaper.ok_or("generated Sway theme is missing its wallpaper")?,
    })
}

fn parse_solid_wallpaper(line: &str) -> Result<Wallpaper, String> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5
        || fields[0] != "output"
        || fields[1] != "*"
        || fields[2] != "bg"
        || fields[4] != "solid_color"
        || fields[3].len() != 7
        || !fields[3].starts_with('#')
    {
        return Err("generated Sway wallpaper must be one solid RGB color".into());
    }
    let color = &fields[3][1..];
    let channel = |start| {
        u8::from_str_radix(&color[start..start + 2], 16)
            .map_err(|_| "generated Sway wallpaper contains invalid RGB")
    };
    Ok(Wallpaper::Solid {
        rgb: [channel(0)?, channel(2)?, channel(4)?],
    })
}

struct SwayDesktop;

impl WallpaperBackend for SwayDesktop {
    type Error = String;

    fn set_wallpaper(
        &mut self,
        request: &WallpaperRequest,
    ) -> Result<WallpaperReceipt, Self::Error> {
        unit(&request.project)?;
        let outputs = sway("get_outputs", None)?;
        if !outputs.as_array().is_some_and(|outputs| {
            outputs.iter().any(|output| {
                output["name"] == request.display_id && output["active"].as_bool() == Some(true)
            })
        }) {
            return Err("wallpaper display is missing or inactive".into());
        }
        let rgb = match &request.wallpaper {
            Wallpaper::Solid { rgb } => *rgb,
            Wallpaper::ManagedImage { .. } => {
                return Err(
                    "managed wallpaper images are not installed on this Sway backend".into(),
                );
            }
        };
        let color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
        sway(
            "command",
            Some(&format!(
                "output {} bg {} solid_color",
                sway_quote(&request.display_id),
                sway_quote(&color)
            )),
        )?;
        Ok(WallpaperReceipt {
            project: request.project.clone(),
            display_id: request.display_id.clone(),
            wallpaper: request.wallpaper.clone(),
        })
    }
}
fn nodes<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    if value["app_id"].is_string() || value["window"].is_number() {
        out.push(value);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = value[key].as_array() {
            for c in children {
                nodes(c, out);
            }
        }
    }
}

fn await_window(project: &str, app_id: &str) -> Result<Value, String> {
    for _ in 0..100 {
        let tree = sway("get_tree", None)?;
        if let Some(workspace) = find_workspace(&tree, project) {
            let mut clients = Vec::new();
            nodes(workspace, &mut clients);
            if let Some(client) = clients.iter().find(|c| c["app_id"] == app_id) {
                return Ok(
                    json!({"project":project,"window_id":client["id"],"app_id":app_id,"mapped":true}),
                );
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!(
        "launch accepted but {app_id} did not map in {project} within 10 seconds"
    ))
}

/// Human opening a session view does not submit a prompt; callers serialize navigation.
#[cfg(feature = "opencode")]
pub fn open_session(project: &str, tmux_name: &str, directory: &str) -> Result<Value, String> {
    unit(project)?;
    if !Path::new(directory).is_absolute() || !Path::new(directory).is_dir() {
        return Err("session directory must exist and be absolute".into());
    }
    let app_id = format!("dock-session-{project}");
    let tree = sway("get_tree", None)?;
    let mut clients = Vec::new();
    nodes(&tree, &mut clients);
    if let Some(client) = clients.iter().find(|c| c["app_id"] == app_id) {
        return Ok(json!({"project":project,"existing":true,"window_id":client["id"]}));
    }
    let config = home()?.join(format!(".config/dock/hypr/foot-{project}.ini"));
    if !config.is_file() {
        return Err("foot project theme missing".into());
    }
    let cmd = format!(
        "foot --app-id={} --title={} --config={} --working-directory={} tmux -L dock-opencode attach-session -t {}",
        quote(&app_id),
        quote(&format!("Dock {project} OpenCode")),
        quote(config.to_str().ok_or("config path not UTF8")?),
        quote(directory),
        quote(tmux_name)
    );
    sway(
        "command",
        Some(&format!(
            "for_window [app_id={}] move to workspace {}; exec {}",
            sway_quote(&app_id),
            sway_quote(project),
            sway_quote(&cmd)
        )),
    )?;
    await_window(project, &app_id)
}

fn capture(project: &str) -> Result<Value, String> {
    let workspace = active(project)?;
    let output = workspace["output"]
        .as_str()
        .ok_or("project output missing")?;
    let dir = home()?.join(".local/state/dock/captures");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let path = dir.join(format!("{project}.png"));
    wayland(
        "grim",
        &["-o", output, path.to_str().ok_or("capture path not UTF8")?],
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let (width, height) = png_size(&bytes)?;
    Ok(
        json!({"project":project,"output":output,"path":path,"width":width,"height":height,"png_base64":base64::engine::general_purpose::STANDARD.encode(bytes)}),
    )
}
fn browser(project: &str) -> Result<Value, String> {
    #[cfg(feature = "opencode")]
    let directory = crate::opencode::directory(project)?;
    #[cfg(not(feature = "opencode"))]
    let directory = home()?.join(format!(".local/state/dock/{project}"));
    let nested = directory.join("examples/workspace/index.html");
    let fixture = if nested.is_file() {
        nested
    } else {
        directory.join("index.html")
    };
    if !fixture.is_file() {
        return Err("project browser fixture does not exist".into());
    }
    let app_id = format!("dock-browser-{project}");
    let tree = sway("get_tree", None)?;
    let mut clients = Vec::new();
    nodes(&tree, &mut clients);
    if let Some(client) = clients.iter().find(|c| c["app_id"] == app_id) {
        return Ok(json!({"project":project,"existing":true,"window_id":client["id"]}));
    }
    let profile = home()?.join(format!(".local/state/dock/browser/{project}"));
    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let preferences = profile.join("user.js");
    if !preferences.exists() {
        fs::write(&preferences,"user_pref(\"browser.aboutwelcome.enabled\", false);\nuser_pref(\"browser.shell.checkDefaultBrowser\", false);\nuser_pref(\"browser.startup.homepage_override.mstone\", \"ignore\");\nuser_pref(\"datareporting.healthreport.uploadEnabled\", false);\nuser_pref(\"toolkit.telemetry.enabled\", false);\n").map_err(|e|e.to_string())?;
    }
    let binary = home()?.join(".local/lib/dock-browser/firefox/firefox");
    if !binary.is_file() {
        return Err("native Firefox runtime missing".into());
    }
    let launch = format!(
        "env MOZ_ENABLE_WAYLAND=1 MOZ_APP_REMOTINGNAME={} {} --no-remote --profile {} --name {} {}",
        quote(&app_id),
        quote(binary.to_str().ok_or("browser path not UTF8")?),
        quote(profile.to_str().ok_or("profile path not UTF8")?),
        quote(&app_id),
        quote(&format!("file://{}", fixture.display()))
    );
    sway(
        "command",
        Some(&format!(
            "for_window [app_id={}] move to workspace {}; exec {}",
            sway_quote(&app_id),
            sway_quote(project),
            sway_quote(&launch)
        )),
    )?;
    await_window(project, &app_id)
}
fn pairing_url(raw: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "invalid pairing URL")?;
    let query: Vec<_> = url.query_pairs().collect();
    if url.scheme() != "https"
        || url.host_str() != Some("sso.fpl.dev")
        || url.path() != "/desktop-bridges/pair"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() != Some(443)
        || query.len() != 1
        || query[0].0 != "id"
        || query[0].1.is_empty()
        || query[0].1.len() > 128
        || !query[0]
            .1
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("expected official HTTPS pairing URL with one public id".into());
    }
    Ok(url)
}

fn browser_url(project: &str, raw: &str) -> Result<Value, String> {
    let url = pairing_url(raw)?;
    open_web_url(project, &url)
}

/// Human-facing application links use the same persistent project browser as
/// enrollment. The pairing command retains its narrower public-ID contract.
fn web_url(raw: &str) -> Result<url::Url, String> {
    if raw.len() > 16 * 1024 || raw.chars().any(char::is_control) {
        return Err("invalid desktop web URL".into());
    }
    let url = url::Url::parse(raw).map_err(|_| "invalid desktop web URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("desktop application links require HTTPS without embedded credentials".into());
    }
    Ok(url)
}

fn open_web_url(project: &str, url: &url::Url) -> Result<Value, String> {
    unit(project)?;
    let profile = home()?.join(format!(".local/state/dock/web/{project}"));
    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let app_id = format!("dock-web-{project}");
    let binary = home()?.join(".local/lib/dock-browser/firefox/firefox");
    if !binary.is_file() {
        return Err("native Firefox runtime missing".into());
    }
    sway(
        "command",
        Some(&format!(
            "for_window [app_id={}] move to workspace {}",
            sway_quote(&app_id),
            sway_quote(project)
        )),
    )?;
    let registered = state()?;
    // Reuse the isolated compositor's D-Bus session, never GNOME's activation bus.
    let socket = Path::new(registered["sway_socket"].as_str().unwrap());
    let pid = socket
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split('.').nth(2))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("registered compositor PID missing")?;
    let environment = fs::read(format!("/proc/{pid}/environ")).map_err(|e| e.to_string())?;
    let bus = environment
        .split(|b| *b == 0)
        .find_map(|entry| {
            entry
                .strip_prefix(b"DBUS_SESSION_BUS_ADDRESS=")
                .and_then(|v| std::str::from_utf8(v).ok())
        })
        .ok_or("isolated compositor D-Bus session missing")?;
    use std::process::{Command, Stdio};
    // Preserve the remote-command endpoint so subsequent requests reuse this profile.
    let mut child = Command::new(binary)
        .env("XDG_RUNTIME_DIR", registered["runtime"].as_str().unwrap())
        .env(
            "WAYLAND_DISPLAY",
            registered["wayland_display"].as_str().unwrap(),
        )
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("MOZ_APP_REMOTINGNAME", &app_id)
        .args([
            "--profile",
            profile.to_str().ok_or("profile path not UTF8")?,
            "--name",
            &app_id,
            "--new-tab",
            url.as_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Firefox launch failed: {e}"))?;
    let mut result = await_window(project, &app_id)?;
    if let Some(status) = child.try_wait().map_err(|e| e.to_string())?
        && !status.success()
    {
        return Err(format!("Firefox navigation process failed: {status}"));
    }
    result["profile"] = json!(profile);
    result["navigation_requested"] = json!(true);
    result["authentication_confirmed"] = json!(false);
    result["browser_status"] = browser_status(project)?;
    Ok(result)
}

fn terms_accepted(prefs: &str) -> bool {
    prefs.lines().any(|line| {
        line.trim()
            .strip_prefix("user_pref(\"termsofuse.acceptedVersion\",")
            .and_then(|value| value.trim().strip_suffix(");"))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .is_some_and(|version| version > 0)
    })
}

fn browser_status(project: &str) -> Result<Value, String> {
    unit(project)?;
    let profile = home()?.join(format!(".local/state/dock/web/{project}"));
    let prefs = profile.join("prefs.js");
    let accepted = if prefs.is_file() {
        let metadata = fs::metadata(&prefs).map_err(|e| e.to_string())?;
        if metadata.len() > 4 * 1024 * 1024 {
            return Err("Firefox preferences exceed 4 MiB".into());
        }
        terms_accepted(&fs::read_to_string(&prefs).map_err(|e| e.to_string())?)
    } else {
        false
    };
    Ok(json!({
        "project":project,
        "profile_exists":profile.is_dir(),
        "terms_accepted":accepted,
        "attention_required":profile.is_dir() && !accepted
    }))
}

fn find_workspace<'a>(tree: &'a Value, project: &str) -> Option<&'a Value> {
    if tree["type"] == "workspace" && tree["name"] == project {
        return Some(tree);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = tree[key].as_array() {
            for child in children {
                if let Some(w) = find_workspace(child, project) {
                    return Some(w);
                }
            }
        }
    }
    None
}

fn verify_workspace_layout(tree: &Value, project: &str, expected: &str) -> Result<(), String> {
    let workspace = find_workspace(tree, project)
        .ok_or_else(|| format!("project workspace {project} missing"))?;
    let workspace_layout = workspace["layout"].as_str();
    let content_layout = workspace["nodes"]
        .as_array()
        .filter(|nodes| nodes.len() == 1)
        .and_then(|nodes| nodes[0]["layout"].as_str());
    if workspace_layout != Some(expected) && content_layout != Some(expected) {
        return Err(format!(
            "project workspace {project} layout is {}, expected {expected}",
            content_layout.or(workspace_layout).unwrap_or("missing")
        ));
    }
    Ok(())
}

fn png_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 24
        || bytes.len() > 16 * 1024 * 1024
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
    {
        return Err("invalid or oversized PNG capture".into());
    }
    Ok((
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    ))
}
fn key_args(key: &str) -> Result<Vec<String>, String> {
    let pieces: Vec<_> = key.split('+').collect();
    let mut result = Vec::new();
    for modifier in &pieces[..pieces.len() - 1] {
        let modifier = match modifier.to_ascii_lowercase().as_str() {
            "ctrl" => "ctrl",
            "alt" => "alt",
            "shift" => "shift",
            "super" => "logo",
            _ => return Err("unsupported modifier".into()),
        };
        result.extend(["-M".into(), modifier.into()]);
    }
    let key = pieces.last().unwrap();
    if key.is_empty()
        || key.len() > 32
        || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid keysym".into());
    }
    result.extend(["-k".into(), (*key).into()]);
    for modifier in pieces[..pieces.len() - 1].iter().rev() {
        let m = if modifier.eq_ignore_ascii_case("super") {
            "logo".into()
        } else {
            modifier.to_ascii_lowercase()
        };
        result.extend(["-m".into(), m]);
    }
    Ok(result)
}

pub fn execute(args: &[String]) -> Result<Value, String> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or("desktop action required")?;
    if action == "desktop-start" && args.len() == 1 {
        return control::with_navigation(start);
    }
    if action == "desktop-terminal" && args.len() == 1 {
        let spaces = sway("get_workspaces", None)?;
        let project = spaces
            .as_array()
            .and_then(|v| v.iter().find(|w| w["focused"] == true))
            .and_then(|w| w["name"].as_str())
            .ok_or("no active workspace")?;
        let config = home()?.join(format!(".config/dock/hypr/foot-{project}.ini"));
        return sway(
            "command",
            Some(&format!(
                "exec {}",
                sway_quote(&format!(
                    "foot --config={}",
                    quote(config.to_str().ok_or("config path not UTF8")?)
                ))
            )),
        );
    }
    let project = args.get(1).ok_or("project required")?;
    unit(project)?;
    match action {
        "desktop-status" if args.len()==2 => Ok(json!({"project":project,"outputs":sway("get_outputs",None)?,"workspaces":sway("get_workspaces",None)?,"tree":sway("get_tree",None)?,"ownership":control::status(project)?,"compositor":"sway","transport":"wayvnc via SSH localhost:5901"})),
        "desktop-recall" if args.len()==2 => control::with_navigation(||recall(project)),
        "desktop-recall-agent" if args.len()==2 => control::with_agent(project,||recall(project)),
        "desktop-browser" if args.len()==2 => control::with_navigation(||browser(project)),
        "desktop-browser-url" if args.len()==3 => control::with_navigation(||browser_url(project,&args[2])),
        "desktop-web-url" if args.len()==3 => control::with_navigation(||open_web_url(project,&web_url(&args[2])?)),
        "desktop-web-status" if args.len()==2 => browser_status(project),
        "desktop-capture-file" if args.len()==2 => control::with_navigation(||{let mut v=capture(project)?;v.as_object_mut().unwrap().remove("png_base64");Ok(v)}),
        "desktop-focus" if args.len()==3 => control::with_agent(project,||{
            active(project)?;let id=args[2].parse::<u64>().map_err(|_|"invalid window id")?;
            let tree=sway("get_tree",None)?;let workspace=find_workspace(&tree,project).ok_or("project workspace missing")?;
            let mut clients=Vec::new();nodes(workspace,&mut clients);
            if !clients.iter().any(|c|c["id"]==id){return Err("window does not belong to target project".into());}
            sway("command",Some(&format!("[con_id={id}] focus")))?;
            let observed=sway("get_tree",None)?;let mut all=Vec::new();nodes(&observed,&mut all);
            if !all.iter().any(|c|c["id"]==id && c["focused"]==true){return Err("focus not acknowledged".into());}
            Ok(json!({"project":project,"window_id":id,"focused":true}))
        }),
        "desktop-capture" if args.len()==2 => control::with_navigation(||capture(project)),
        "desktop-control" if args.len()==4 && args[2]=="agent" => {
            let epoch=args[3].parse::<u64>().map_err(|_|"expected epoch must be an unsigned integer")?;
            Ok(json!({"ownership":control::grant(project,epoch)?}))
        }
        "desktop-control" if args.len()==3 => {
            if args[2]=="human" {
                #[cfg(feature = "opencode")]
                let abort = || crate::opencode::abort(project);
                #[cfg(not(feature = "opencode"))]
                let abort = || Ok(json!({"note":"OpenCode integration not compiled in; no session to abort"}));
                control::takeover(project, abort)
            } else { Ok(json!({"ownership":control::set(project,&args[2])?})) }

        }
        "desktop-text" if args.len()==3 => control::with_agent(project,||{active(project)?;if args[2].len()>16384{return Err("text exceeds 16KiB".into());}wayland("wtype",&["--",&args[2]])?;Ok(json!({"project":project,"input_sent":true,"kind":"text"}))}),
        "desktop-key" if args.len()==3 => control::with_agent(project,||{active(project)?;let keys=key_args(&args[2])?;wayland("wtype",&keys.iter().map(String::as_str).collect::<Vec<_>>())?;Ok(json!({"project":project,"input_sent":true,"kind":"key"}))}),
        "desktop-click" if args.len()==5 => control::with_agent(project,||{
            let workspace=active(project)?;
            let output_name=workspace["output"].as_str().ok_or("project output missing")?;
            let x=args[2].parse::<u32>().map_err(|_|"invalid x")?;let y=args[3].parse::<u32>().map_err(|_|"invalid y")?;
            let outputs=sway("get_outputs",None)?;let output=project_output(&workspace,&outputs)?;
            if x as u64>=output["rect"]["width"].as_u64().unwrap_or(0) || y as u64>=output["rect"]["height"].as_u64().unwrap_or(0){return Err("click outside output bounds".into());}
            let button=match args[4].as_str(){"left"=>0x110,"middle"=>0x112,"right"=>0x111,_=>return Err("button must be left, middle or right".into())};
            let registered=state()?;
            let socket=PathBuf::from(registered["runtime"].as_str().unwrap()).join(registered["wayland_display"].as_str().unwrap());
            pointer::click(&socket,output_name,x,y,output["rect"]["width"].as_u64().unwrap() as u32,output["rect"]["height"].as_u64().unwrap() as u32,button)?;
            Ok(json!({"project":project,"input_sent":true,"kind":"click","output":output_name,"x":x,"y":y}))
        }),
        _=>Err("usage: desktop-status/desktop-recall/desktop-recall-agent/desktop-capture/desktop-web-status PROJECT | desktop-control PROJECT human|agent | desktop-control PROJECT agent EXPECTED_EPOCH | desktop-text/desktop-key PROJECT VALUE | desktop-click PROJECT X Y left|middle|right".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn firefox_terms_status_requires_a_positive_accepted_version() {
        assert!(terms_accepted(
            "user_pref(\"termsofuse.acceptedVersion\", 4);\n"
        ));
        assert!(!terms_accepted(
            "user_pref(\"termsofuse.acceptedVersion\", 0);\n"
        ));
        assert!(!terms_accepted(
            "user_pref(\"browser.termsofuse.prefMigrationCheck\", true);\n"
        ));
    }
    #[test]
    fn recall_uses_a_full_width_tabbed_workspace_before_applying_theme() {
        let plan = recall_commands(
            "studio",
            "# generated\nclient.focused #fff #000 #000 #fff #fff\noutput * bg #18191c solid_color\ngaps inner 8\n",
        )
        .unwrap();
        assert_eq!(plan.commands[0], "workspace \"studio\"");
        assert_eq!(plan.commands[1], "layout tabbed");
        assert_eq!(plan.commands[2], "client.focused #fff #000 #000 #fff #fff");
        assert_eq!(plan.commands[3], "gaps inner current set 8");
        assert_eq!(plan.wallpaper, Wallpaper::Solid { rgb: [24, 25, 28] });
        assert!(
            !plan
                .commands
                .iter()
                .any(|command| command.contains("output *"))
        );
        assert!(recall_commands("studio", "exec rm -rf /;").is_err());
    }
    #[test]
    fn unstyled_alfalfa_projects_use_the_generated_default_theme() {
        let root = std::env::temp_dir().join(format!("dock-theme-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(theme_path(&root, "pedalkernel").is_err());
        std::fs::write(root.join("dock.conf"), "default").unwrap();
        assert_eq!(theme_path(&root, "pedalkernel").unwrap().1, "default");
        std::fs::write(root.join("pedalkernel.conf"), "project").unwrap();
        assert_eq!(theme_path(&root, "pedalkernel").unwrap().1, "project");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn project_layout_validation_fails_loudly_on_drift() {
        let direct = json!({
            "type":"root",
            "nodes":[{"type":"output","nodes":[
                {"type":"workspace","name":"studio","layout":"tabbed","nodes":[]}
            ]}]
        });
        let sway_wrapped = json!({
            "type":"root",
            "nodes":[{"type":"output","nodes":[{
                "type":"workspace","name":"studio","layout":"splith","nodes":[
                    {"type":"con","layout":"tabbed","nodes":[
                        {"type":"con","app_id":"browser"},
                        {"type":"con","app_id":"terminal"}
                    ]}
                ]
            }]}]
        });
        assert!(verify_workspace_layout(&direct, "studio", "tabbed").is_ok());
        assert!(verify_workspace_layout(&sway_wrapped, "studio", "tabbed").is_ok());
        assert!(verify_workspace_layout(&direct, "studio", "splith").is_err());
        assert!(verify_workspace_layout(&direct, "missing", "tabbed").is_err());
    }
    #[test]
    fn project_output_resolves_named_output_without_primary_fallback() {
        let outputs = json!([
            {"name":"HEADLESS-1","active":true,"rect":{"x":0,"width":1600}},
            {"name":"HEADLESS-2","active":true,"rect":{"x":1600,"width":1200}},
            {"name":"HEADLESS-3","active":false}
        ]);
        let second = project_output(&json!({"output":"HEADLESS-2"}), &outputs).unwrap();
        assert_eq!(second["rect"]["width"], 1200);
        assert_eq!(second["rect"]["x"], 1600);
        assert!(project_output(&json!({"output":"missing"}), &outputs).is_err());
        assert!(project_output(&json!({"output":"HEADLESS-3"}), &outputs).is_err());
    }
    #[test]
    fn pairing_url_rejects_wrong_origin_credentials_and_private_codes() {
        assert!(pairing_url("https://sso.fpl.dev/desktop-bridges/pair?id=abc_123-xyz").is_ok());
        for url in [
            "http://sso.fpl.dev/desktop-bridges/pair?id=abc",
            "https://sso.fpl.dev.evil.test/desktop-bridges/pair?id=abc",
            "https://user@sso.fpl.dev/desktop-bridges/pair?id=abc",
            "https://sso.fpl.dev/desktop-bridges/pair?id=abc&device_code=secret",
            "https://sso.fpl.dev/desktop-bridges/pair?id=abc#fragment",
            "https://sso.fpl.dev/desktop-bridges/pair?id=",
        ] {
            assert!(pairing_url(url).is_err(), "accepted {url}");
        }
    }
    #[test]
    fn application_links_preserve_https_paths_but_cannot_launch_local_handlers() {
        let link = "https://sso.fpl.dev/connections?app=dock#devices";
        assert_eq!(web_url(link).unwrap().as_str(), link);
        for raw in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://sso.fpl.dev/",
            "https://user:password@sso.fpl.dev/",
            "https://sso.fpl.dev/\n",
        ] {
            assert!(web_url(raw).is_err(), "accepted {raw}");
        }
        assert!(web_url(&format!("https://sso.fpl.dev/{}", "x".repeat(16 * 1024))).is_err());
        assert!(pairing_url(link).is_err());
    }
    #[test]
    fn malformed_png_is_rejected() {
        assert!(png_size(b"not an image").is_err());
    }
    #[test]
    fn keyboard_modifiers_are_balanced() {
        assert_eq!(
            key_args("CTRL+SHIFT+l").unwrap(),
            vec![
                "-M", "ctrl", "-M", "shift", "-k", "l", "-m", "shift", "-m", "ctrl"
            ]
        );
        assert!(key_args("ctrl+l;exec").is_err());
    }
}
