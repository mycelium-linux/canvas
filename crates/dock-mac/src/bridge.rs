//! Thin client of FPL Desktop Bridge's owner CLI. No token storage or issuer logic.
use serde_json::Value;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy)]
pub enum Owner {
    Mac,
    Agora,
}

pub(crate) fn command(owner: Owner, operation: &str) -> Command {
    match owner {
        Owner::Mac => {
            let executable = std::env::var_os("DOCK_DESKTOP_BRIDGE_BIN")
                .unwrap_or_else(|| "fpl-desktop-bridge".into());
            let mut command = Command::new(executable);
            command.arg(operation);
            command
        }
        Owner::Agora => {
            let mut command = Command::new("ssh");
            command.args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "agora-one",
                ".local/bin/fpl-desktop-bridge",
                operation,
            ]);
            command
        }
    }
}

/// Read-only owner verification. Missing CLI is an installation gap, never authenticated.
pub fn status(owner: Owner) -> Result<Value, String> {
    let output = crate::bounded_output(&mut command(owner, "status"), 30)?;
    if !output.status.success() {
        return Err(
            "FPL Desktop Bridge status unavailable; install or repair the owner CLI".into(),
        );
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "FPL Desktop Bridge returned invalid status JSON")?;
    if value["schema_version"] != 1 || !value["session"].is_string() {
        return Err("Unsupported FPL Desktop Bridge status contract".into());
    }
    Ok(value)
}

/// Caller consumes flushed JSON lines and opens the approval URL in the intended
/// persistent browser. Keep this child alive until paired/failed;
/// explicitly killing and waiting for it cancels polling. Dropping Child alone does
/// not stop its process. A subsequent attempt starts fresh owner pairing.
/// This never opens a browser or consents on the user's behalf.
pub fn start_pair(owner: Owner) -> Result<Child, String> {
    command(owner, "pair")
        .args(["--open", "false"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start FPL Desktop Bridge pairing".into())
}

/// Run the owner's enrollment and navigate only its public approval URL.
/// The user's SSO interaction remains in the project browser.
pub fn pair_in_project(owner: Owner, project: &str) -> Result<Value, String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::time::{Duration, Instant};
    if !crate::session_identifier(project) {
        return Err("Invalid project identifier".into());
    }
    struct Pairing(Child);
    impl Drop for Pairing {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Pairing(start_pair(owner)?);
    let stdout = child.0.stdout.take().ok_or("Pairing output unavailable")?;
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let input = BufReader::new(stdout.take(65_537));
        for line in input.lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(330);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Desktop sign-in expired; start it again")?;
        let line = rx
            .recv_timeout(remaining)
            .map_err(|_| "Desktop sign-in ended or expired")?
            .map_err(|_| "Desktop sign-in output failed")?;
        let event = pairing_event(&line)?;
        match event["event"].as_str() {
            Some("approval_required") => {
                let url = event["verification_uri_complete"]
                    .as_str()
                    .ok_or("Missing approval URL")?;
                crate::remote::rpc(project, "desktop-browser-url", &[url.into()], None)?;
                println!("{event}");
                std::io::stdout()
                    .flush()
                    .map_err(|_| "Could not report sign-in request")?;
            }
            Some("paired") => return Ok(event),
            Some("failed") => return Err("Owner desktop enrollment failed; retry sign-in".into()),
            _ => return Err("Unexpected desktop sign-in state".into()),
        }
    }
}

/// Only the owner-produced public handoff contract may cross into Dock state.
pub fn pairing_event(line: &str) -> Result<Value, String> {
    if line.len() > 16 * 1024 {
        return Err("Owner pairing event exceeds limit".into());
    }
    let value: Value = serde_json::from_str(line).map_err(|_| "Invalid owner pairing event")?;
    if value["schema_version"] != 1 {
        return Err("Unsupported owner pairing event version".into());
    }
    let fields: &[&str] = match value["event"].as_str() {
        Some("approval_required")
            if value["verification_uri_complete"].is_string() && value["expires_in"].is_u64() =>
        {
            &[
                "schema_version",
                "event",
                "verification_uri_complete",
                "expires_in",
            ]
        }
        Some("paired") if value["device_id"].is_string() && value["email"].is_string() => &[
            "schema_version",
            "event",
            "device_id",
            "email",
            "browser_identity",
        ],
        Some("failed") => &["schema_version", "event", "error"],
        _ => return Err("Unrecognized owner pairing event".into()),
    };
    let mut safe = serde_json::Map::new();
    for key in fields {
        if let Some(entry) = value.get(key) {
            safe.insert((*key).into(), entry.clone());
        }
    }
    Ok(Value::Object(safe))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_fields_cannot_enter_dock_state() {
        let event=pairing_event(r#"{"schema_version":1,"event":"paired","device_id":"agora","email":"a@example.test","session_token":"private"}"#).unwrap();
        assert!(event.get("session_token").is_none());
    }
    #[test]
    fn invalid_event_is_rejected() {
        assert!(pairing_event(r#"{"schema_version":1,"event":"approval_required"}"#).is_err());
        assert!(pairing_event(&"x".repeat(16385)).is_err());
    }
}
