//! Shared, explicitly targeted transport for Android owner controls.
use super::*;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn exchange(app: &AppState, request: Value) -> Result<Value> {
    exchange_payload(app, &serde_json::to_vec(&request)?)
}

// The owner CLI obtains its required endpoint CAS on the same connection used
// for switching. Reuse that interface instead of carrying a renderer revision
// across multiple ADB process launches.
pub(super) fn switch_workspace(app: &AppState, project_id: &str) -> Result<Value> {
    ensure!(
        !project_id.is_empty() && !project_id.chars().any(char::is_whitespace),
        "TV workspace identifier must be one exact token"
    );
    exchange_payload(app, format!("workspace switch {project_id}").as_bytes())
}

fn exchange_payload(app: &AppState, payload: &[u8]) -> Result<Value> {
    let serial = app
        .android_serial
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Android target unavailable"))?;
    let executable = std::env::var("CANVAS_TVCTL_BIN").unwrap_or_else(|_| "canvas-tvctl".into());
    let mut child = Command::new(executable)
        .arg(serial.as_ref())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Err(error) = std::io::Write::write_all(child.stdin.as_mut().unwrap(), payload) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    child.stdin.take();
    // Inspect includes the active document; drain concurrently so the pipe
    // cannot fill while the child is still running.
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut raw = Vec::new();
        stdout.take(4 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
        Ok(raw)
    });
    let stderr = child.stderr.take().unwrap();
    let error_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut raw = Vec::new();
        stderr.take(4097).read_to_end(&mut raw)?;
        Ok(raw)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Android control timeout; outcome unknown; do not replay");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let raw = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Android reply reader failed"))??;
    let error_raw = error_reader
        .join()
        .map_err(|_| anyhow::anyhow!("Android error reader failed"))??;
    ensure!(raw.len() <= 4 * 1024 * 1024, "Android reply exceeds limit");
    ensure!(error_raw.len() <= 4096, "Android error exceeds limit");
    ensure!(
        status.success(),
        "Android control failed: {}",
        String::from_utf8_lossy(&error_raw).trim()
    );
    let reply: Value = serde_json::from_slice(&raw)?;
    ensure!(
        reply["ok"] == true,
        "Android owner rejected request: {}",
        reply["error"]
    );
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_command_rejects_multiple_tokens_before_owner_dispatch() {
        let (app, _) = super::super::tests::sample();
        for id in [
            "",
            "clock\nworkspace switch studio",
            "clock studio",
            "clock\tstudio",
        ] {
            let error = switch_workspace(&app, id).unwrap_err();
            assert!(error.to_string().contains("one exact token"));
        }
    }
}
