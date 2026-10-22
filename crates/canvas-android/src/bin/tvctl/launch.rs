//! App lifecycle stays in the authorized ADB adapter, not the renderer protocol.
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const PACKAGE: &str = "dev.fpl.canvas.tv";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    schema_version: u32,
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    command: LaunchCommand,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchCommand {
    command: LaunchName,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchName {
    Launch,
}

fn component(text: &str) -> Option<String> {
    let (package, class) = text.split_once('/')?;
    if package != PACKAGE
        || class.is_empty()
        || !class
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$'))
    {
        return None;
    }
    let class = if class.starts_with('.') {
        format!("{package}{class}")
    } else {
        class.into()
    };
    Some(format!("{package}/{class}"))
}

fn resolved(text: &str) -> Result<String, String> {
    let components: Vec<_> = text
        .lines()
        .filter_map(|line| component(line.trim()))
        .collect();
    if components.len() != 1 {
        return Err("Canvas TV has no unique installed launcher activity".into());
    }
    Ok(components[0].clone())
}

fn focused(text: &str, activity: &str) -> bool {
    text.lines()
        .filter(|line| line.trim_start().starts_with("mCurrentFocus="))
        .any(|line| {
            line.split_whitespace()
                .filter_map(|word| component(word.trim_end_matches('}')))
                .any(|found| found == activity)
        })
}

pub fn execute(
    request: &Request,
    mut adb: impl FnMut(&[&str]) -> Result<Vec<u8>, String>,
) -> Value {
    let result = (|| -> Result<String, String> {
        if request.schema_version != 1
            || request.request_id.is_empty()
            || request.request_id.len() > 128
            || request.request_id.chars().any(char::is_control)
            || request.expected_revision.is_some()
        {
            return Err("Invalid owner-local launch request".into());
        }
        let _ = &request.command.command;
        let output = adb(&[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LEANBACK_LAUNCHER",
            PACKAGE,
        ])?;
        let activity = resolved(&String::from_utf8_lossy(&output))?;
        let output = adb(&[
            "shell",
            "am",
            "start",
            "-W",
            "--activity-single-top",
            "-n",
            &activity,
        ])?;
        let output = String::from_utf8_lossy(&output);
        if output
            .lines()
            .any(|line| line.trim_start().starts_with("Error"))
        {
            return Err("Android rejected the resolved Canvas activity".into());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let output = adb(&["shell", "dumpsys", "window", "windows"])?;
            if focused(&String::from_utf8_lossy(&output), &activity) {
                return Ok(activity);
            }
            if Instant::now() >= deadline {
                return Err("Canvas TV did not acquire foreground focus".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    let outcome = match result {
        Ok(activity) => {
            json!({"status":"ok","receipt":{"kind":"app_launched","activity":activity,"foreground":true}})
        }
        Err(message) => json!({"status":"error","code":"launch_failed","message":message}),
    };
    json!({"schema_version":1,"request_id":request.request_id,"outcome":outcome})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        serde_json::from_value(
            json!({"schema_version":1,"request_id":"test","command":{"command":"launch"}}),
        )
        .unwrap()
    }
    #[test]
    fn launcher_comes_from_package_manager_not_package_name() {
        assert_eq!(
            resolved("dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity\n").unwrap(),
            "dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity"
        );
        assert!(resolved("No activity found").is_err());
        assert!(resolved("evil/package.Activity").is_err());
        assert!(resolved("dev.fpl.canvas.tv/.Activity;reboot").is_err());
        assert!(resolved("dev.fpl.canvas.tv/.A\ndev.fpl.canvas.tv/.B").is_err());
    }
    #[test]
    fn focus_requires_current_exact_component_not_background_app() {
        let activity = "dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity";
        assert!(focused(
            "mCurrentFocus=Window{123 u0 dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity}",
            activity
        ));
        assert!(!focused(
            "mFocusedApp=dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity",
            activity
        ));
        assert!(!focused(
            "mCurrentFocus=Window{123 u0 dev.fpl.canvas.tv/dev.fpl.canvas.TvActivityOther}",
            activity
        ));
    }
    #[test]
    fn launch_is_correlated_and_verifies_focus() {
        let mut calls = 0;
        let reply = execute(&request(), |args| {
            calls += 1;
            Ok(match calls {
                1 => b"dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity\n".to_vec(),
                2 => {
                    assert_eq!(
                        args.last(),
                        Some(&"dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity")
                    );
                    b"Status: ok".to_vec()
                }
                _ => b"mCurrentFocus=Window{123 u0 dev.fpl.canvas.tv/dev.fpl.canvas.TvActivity}"
                    .to_vec(),
            })
        });
        assert_eq!(calls, 3);
        assert_eq!(reply["request_id"], "test");
        assert_eq!(reply["outcome"]["receipt"]["foreground"], true);
    }
    #[test]
    fn invalid_or_unavailable_launch_fails_without_starting() {
        assert!(serde_json::from_value::<Request>(json!({"schema_version":1,"request_id":"x","command":{"command":"launch","package":"other"}})).is_err());
        let mut bad = request();
        bad.expected_revision = Some(4);
        let reply = execute(&bad, |_| panic!("must not invoke ADB"));
        assert_eq!(reply["outcome"]["status"], "error");
        let reply = execute(&request(), |_| Err("ADB unauthorized".into()));
        assert_eq!(reply["outcome"]["message"], "ADB unauthorized");
        let mut calls = 0;
        let reply = execute(&request(), |_| {
            calls += 1;
            Ok(b"No activity found".to_vec())
        });
        assert_eq!(calls, 1);
        assert_eq!(reply["outcome"]["status"], "error");
    }
}
