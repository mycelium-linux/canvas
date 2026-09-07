//! Narrow macOS Accessibility bridge, callable by the GUI or an agent CLI.
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    #[serde(default)]
    pub id: String,
    pub pid: u32,
    pub app: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Split an explicit work area. Never assumes a screen size or moves unrelated windows.
pub fn tiles(area: Rect, count: usize) -> Result<Vec<Rect>, String> {
    if count == 0 || count > 16 || area.width < 160 * count as u32 || area.height < 120 {
        return Err(
            "Choose 1–16 windows and a work area at least 160px wide per window and 120px tall"
                .into(),
        );
    }
    validate_rect(area)?;
    let width = area.width / count as u32;
    Ok((0..count)
        .map(|i| Rect {
            x: area.x + (i as i32 * width as i32),
            y: area.y,
            width: if i + 1 == count {
                area.width - width * i as u32
            } else {
                width
            },
            height: area.height,
        })
        .collect())
}

pub fn windows() -> Result<Vec<Window>, String> {
    use dock_control::{Operation, Response};
    match dock_control::request(Operation::Snapshot)? {
        Response::Snapshot { windows, .. } => Ok(windows
            .into_iter()
            .map(|w| Window {
                id: w.id,
                pid: w.pid,
                app: w.app,
                title: w.title,
            })
            .collect()),
        _ => Err("Unexpected Dock Control response".into()),
    }
}
fn native_id(window: &Window) -> Result<String, String> {
    if !window.id.is_empty() {
        return Ok(window.id.clone());
    }
    Err("Window attachment predates Dock Control or the helper restarted. Refresh and attach this window again.".into())
}
pub fn focus(window: &Window) -> Result<(), String> {
    match dock_control::request(dock_control::Operation::Focus {
        id: native_id(window)?,
    })? {
        dock_control::Response::Applied { .. } => Ok(()),
        _ => Err("Dock Control did not acknowledge window focus".into()),
    }
}
pub fn tile(window: &Window, rect: Rect) -> Result<(), String> {
    validate_rect(rect)?;
    if rect.width < 160 || rect.height < 120 {
        return Err("Window size must be at least 160 × 120".into());
    }
    let frame = dock_control::Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    };
    match dock_control::request(dock_control::Operation::SetFrame {
        id: native_id(window)?,
        frame,
    })? {
        dock_control::Response::Applied { window } if window.frame == frame => Ok(()),
        _ => Err("Application constrained requested window geometry".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiles_preserve_work_area_and_negative_screen_origin() {
        let area = Rect {
            x: -1001,
            y: 25,
            width: 1001,
            height: 700,
        };
        let result = tiles(area, 3).unwrap();
        assert_eq!(result[0].x, -1001);
        assert_eq!(result[2].x + result[2].width as i32, 0);
        assert_eq!(result.iter().map(|r| r.width).sum::<u32>(), 1001);
    }
    #[test]
    fn invalid_work_areas_fail_before_os_calls() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 320,
            height: 700,
        };
        assert!(tiles(area, 0).is_err());
        assert!(tiles(area, 3).is_err());
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RemoteAction {
    Status,
    Pause,
    Resume,
}
impl RemoteAction {
    fn argument(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}
fn session_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
/// SSH is a narrow authenticated transport to an installed, deterministic host adapter.
/// Target is validated because SSH serializes command arguments through the remote shell.
pub fn remote_session(target: &str, action: RemoteAction) -> Result<String, String> {
    if !session_identifier(target) {
        return Err("Invalid managed session identifier".into());
    }
    let out = bounded_output(
        Command::new("ssh").args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "agora-one",
            ".local/bin/dock-linux",
            action.argument(),
            target,
        ]),
        20,
    )
    .map_err(|e| format!("Cannot reach Agora One: {e}"))?;
    parse_remote_response(&out.stdout, out.status.success(), action)
}

fn validate_rect(rect: Rect) -> Result<(), String> {
    let width =
        i32::try_from(rect.width).map_err(|_| "Window width exceeds screen coordinate range")?;
    let height =
        i32::try_from(rect.height).map_err(|_| "Window height exceeds screen coordinate range")?;
    rect.x
        .checked_add(width)
        .ok_or("Window right edge exceeds screen coordinate range")?;
    rect.y
        .checked_add(height)
        .ok_or("Window bottom edge exceeds screen coordinate range")?;
    Ok(())
}
fn parse_remote_response(
    bytes: &[u8],
    success: bool,
    action: RemoteAction,
) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Agora One adapter did not return JSON: {e}"))?;
    if value["schema_version"].as_u64() != Some(1) {
        return Err("Unsupported Agora One response schema".into());
    }
    if !success || value["ok"].as_bool() != Some(true) {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Agora One rejected the action")
            .into());
    }
    let result = value
        .get("result")
        .filter(|v| v.is_object())
        .ok_or("Agora One response is missing its result")?;
    if !matches!(action, RemoteAction::Status) {
        let expected = if matches!(action, RemoteAction::Pause) {
            "frozen"
        } else {
            "running"
        };
        if result["properties"]["FreezerState"] != expected {
            return Err("Agora One did not acknowledge the requested process state".into());
        }
    }
    serde_json::to_string_pretty(result).map_err(|e| e.to_string())
}

fn bounded_output(
    command: &mut Command,
    timeout_seconds: u64,
) -> Result<std::process::Output, String> {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        stdout.read_to_end(&mut v).map(|_| v)
    });
    let err_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        stderr.read_to_end(&mut v).map(|_| v)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(timeout_seconds) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Timed out after {timeout_seconds}s; the command was stopped"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = out_thread
        .join()
        .map_err(|_| "stdout reader failed")?
        .map_err(|e| e.to_string())?;
    let stderr = err_thread
        .join()
        .map_err(|_| "stderr reader failed")?
        .map_err(|e| e.to_string())?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn overflowing_geometry_is_rejected() {
        assert!(tiles(
            Rect {
                x: i32::MAX,
                y: 0,
                width: 160,
                height: 120
            },
            1
        )
        .is_err());
        assert!(tiles(
            Rect {
                x: 0,
                y: 0,
                width: u32::MAX,
                height: 120
            },
            1
        )
        .is_err());
    }
    #[test]
    fn no_pause_ack_for_wrong_version_missing_result_or_missing_freezer_state() {
        for input in [
            r#"{"schema_version":2,"ok":true,"result":{}}"#,
            r#"{"schema_version":1,"ok":true}"#,
            r#"{"schema_version":1,"ok":true,"result":{}}"#,
            r#"{"schema_version":1,"ok":true,"result":{"properties":{"FreezerState":"running"}}}"#,
        ] {
            assert!(parse_remote_response(input.as_bytes(), true, RemoteAction::Pause).is_err());
        }
        assert!(parse_remote_response(
            br#"{"schema_version":1,"ok":true,"result":{"properties":{"FreezerState":"frozen"}}}"#,
            true,
            RemoteAction::Pause
        )
        .is_ok());
    }
    #[test]
    fn remote_shell_metacharacters_are_rejected_before_transport() {
        for target in ["dock;id", "$(id)", "-option", "foo bar", ""] {
            assert!(remote_session(target, RemoteAction::Status).is_err());
        }
    }
}

/// Read the primary display's usable frame (excluding menu bar and Dock), then
/// convert AppKit's bottom-left coordinates to Accessibility's top-left space.
pub fn work_area() -> Result<Rect, String> {
    match dock_control::request(dock_control::Operation::Snapshot)? {
        dock_control::Response::Snapshot { displays, .. } => {
            let display = displays
                .iter()
                .find(|d| d.primary)
                .ok_or("Primary display unavailable")?;
            let r = display.visible_frame;
            Ok(Rect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
        }
        _ => Err("Dock Control did not return display geometry".into()),
    }
}

pub mod reservation;

pub fn recall_linux(project: &str) -> Result<String, String> {
    if !session_identifier(project) {
        return Err("Invalid project identifier".into());
    }
    let out = bounded_output(
        Command::new("ssh").args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "agora-one",
            ".local/bin/dock-linux",
            "hypr-project",
            project,
        ]),
        20,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("Invalid Agora One response: {e}"))?;
    if !out.status.success() || value["schema_version"] != 1 || value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Agora One rejected workspace recall")
            .into());
    }
    if value["result"]["active_workspace"]["name"] != project
        || value["result"]["theme_applied"] != true
    {
        return Err("Agora One did not acknowledge the requested workspace and theme".into());
    }
    Ok(format!(
        "Agora One recalled {project} and applied its Coherence theme."
    ))
}

pub mod alfalfa;
pub mod bridge;
pub mod edgerunner;
pub mod mcp;
pub mod remote;
pub mod resources;

pub mod edgerunner_authenticated;
