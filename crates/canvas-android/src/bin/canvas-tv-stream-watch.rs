//! Explicit source-health adapter -> revision-bound surface lease. No discovery,
//! no automatic workspace takeover, and no credentials in the descriptor.
use canvas_core::stream_lease::{ProgressTracker, hls_progress, waiting_content};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

fn now_ms() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis() as u64)
}

fn safe_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 1024
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
}

fn source(host: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let remote = format!("stat -c %Y -- {path}\ndate +%s\nhead -c 65537 -- {path}");
    let mut child = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "ServerAliveInterval=2",
            "-o",
            "ServerAliveCountMax=1",
            host,
            &remote,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    // Read bounded output concurrently, so a filled pipe cannot deadlock wait.
    let stdout = child.stdout.take().ok_or("No source pipe")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65_601).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(4);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            let _ = reader.join();
            return Err("Source observation timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader.join().map_err(|_| "Source reader failed")??;
    if !status.success() || bytes.len() > 65_600 {
        return Err("Source observation failed or oversized".into());
    }
    let raw = String::from_utf8(bytes)?;
    let mut lines = raw.splitn(3, '\n');
    let modified: i64 = lines.next().ok_or("Missing producer time")?.parse()?;
    let clock: i64 = lines.next().ok_or("Missing source clock")?.parse()?;
    if !(0..=5).contains(&clock.saturating_sub(modified)) {
        return Err("Producer playlist is stale".into());
    }
    Ok(lines.next().ok_or("Missing producer playlist")?.to_owned())
}

fn control(serial: &str, request: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let mut child = Command::new("canvas-tvctl")
        .arg(serial)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("No control pipe")?
        .write_all(request.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err("TV control request failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: canvas-tv-stream-watch ADB_SERIAL CONFIG.json".into());
    }
    let bytes = std::fs::read(&args[2])?;
    if bytes.len() > 16_384 {
        return Err("Config exceeds limit".into());
    }
    let config: Value = serde_json::from_slice(&bytes)?;
    let field = |name: &str| {
        config[name]
            .as_str()
            .ok_or_else(|| format!("Missing {name}"))
    };
    let host = field("ssh_target")?;
    let path = field("playlist_path")?;
    let url = field("url")?;
    let project = field("project_id")?;
    let id = field("surface_id")?;
    if !safe_path(path)
        || host.is_empty()
        || host.starts_with('-')
        || host.len() > 256
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
    {
        return Err("Invalid explicit SSH source".into());
    }
    canvas_android::validate_stream(url)?;
    let mut tracker = ProgressTracker::default();
    let mut previous_error = String::new();
    loop {
        let content =
            match source(host, path).and_then(|raw| hls_progress(&raw).map_err(Into::into)) {
                Ok(marker) => {
                    previous_error.clear();
                    tracker
                        .observe(marker, url, now_ms()?)
                        .map(serde_json::to_value)
                        .transpose()?
                        .unwrap_or_else(waiting_content)
                }
                Err(error) => {
                    tracker.reset();
                    let message = error.to_string();
                    if message != previous_error {
                        eprintln!("canvas-tv-stream-watch: {message}");
                        previous_error = message;
                    }
                    waiting_content()
                }
            };
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let observed = control(&args[1], "inspect\n")?;
            // Never steal selection to update a hidden workspace. Its retained
            // lease expires on the TV even while a different project is active.
            if observed["data"]["catalog"]["selected_project"] != project {
                return Ok(());
            }
            let document = &observed["data"]["document"];
            let mut surface = document["surfaces"]
                .as_array()
                .ok_or("Missing surfaces")?
                .iter()
                .find(|s| s["id"] == id)
                .ok_or("Surface not installed")?
                .clone();
            if surface["content"] == content {
                return Ok(());
            }
            surface["content"] = content;
            let revision = document["revision"].as_u64().ok_or("Missing revision")?;
            let request = json!({"schema_version":1,"request_id":"stream-availability","expected_revision":observed["revision"],
                "command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":document["id"],
                    "base_revision":revision,"revision":revision.checked_add(1).ok_or("Revision overflow")?,
                    "operations":[{"op":"upsert_surface","surface":surface}]}}});
            control(&args[1], &request.to_string())?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("canvas-tv-stream-watch: {error}");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_source_paths_cannot_inject_commands() {
        assert!(safe_path("/home/ajm/stream.m3u8"));
        for path in [
            "relative",
            "/tmp/a;id",
            "/tmp/$(id)",
            "/tmp/a b",
            "/tmp/a\nreboot",
        ] {
            assert!(!safe_path(path));
        }
    }
}
