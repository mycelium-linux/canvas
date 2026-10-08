//! Explicit source-health adapter -> revision-bound surface lease. No discovery,
//! no automatic workspace takeover, and no credentials in the descriptor.
use canvas_core::stream_lease::{ProgressTracker, failure_content, hls_progress, waiting_content};
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

fn local_source(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err("Producer playlist must be a regular file".into());
    }
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > 65_536 {
        return Err("Producer playlist exceeds limit".into());
    }
    if metadata.modified()?.elapsed()? > Duration::from_secs(5) {
        return Err("Producer playlist is stale".into());
    }
    let mut raw = String::new();
    file.take(65_537).read_to_string(&mut raw)?;
    if raw.len() > 65_536 {
        return Err("Producer playlist exceeds limit".into());
    }
    Ok(raw)
}

fn source_failure(message: &str) -> Value {
    let (cause, recovery) = match message {
        "Producer playlist is stale" => (
            "Replay producer stopped advancing",
            "Check producer logs and free disk space; wake the replay host. Playback resumes after fresh media advances.",
        ),
        "Producer ended" => (
            "Replay producer ended",
            "Restart the looping replay producer. Playback resumes after fresh media advances.",
        ),
        "Source observation timed out" => (
            "Replay source did not respond",
            "Check the source host and SSH connection. The observer retries automatically.",
        ),
        _ => (
            "Replay source unavailable or invalid",
            "Check the configured playlist path, host connection and producer logs. The observer retries automatically; details are in its log.",
        ),
    };
    failure_content(
        cause,
        "Fresh media cannot be confirmed; video is hidden.",
        recovery,
    )
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
    let stdout = child.stdout.take().ok_or("No control output pipe")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            let _ = reader.join();
            return Err("TV control timed out; check ADB connection".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader.join().map_err(|_| "Control reader failed")??;
    if !status.success() || bytes.len() > 4 * 1024 * 1024 {
        return Err("TV control request failed".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
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
    // Missing ssh_target explicitly selects a local regular-file observation.
    let host = config
        .get("ssh_target")
        .map(|value| value.as_str().ok_or("ssh_target must be a string"))
        .transpose()?;
    let path = field("playlist_path")?;
    let url = field("url")?;
    let project = field("project_id")?;
    let id = field("surface_id")?;
    if !safe_path(path)
        || host.is_some_and(|host| {
            host.is_empty()
                || host.starts_with('-')
                || host.len() > 256
                || !host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
        })
    {
        return Err("Invalid explicit SSH source".into());
    }
    canvas_android::validate_stream(url)?;
    let mut tracker = ProgressTracker::default();
    let mut previous_error = String::new();
    loop {
        let content = match host
            .map_or_else(|| local_source(path), |host| source(host, path))
            .and_then(|raw| hls_progress(&raw).map_err(Into::into))
        {
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
                    previous_error = message.clone();
                }
                source_failure(&message)
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
    fn failures_explain_cause_impact_and_recovery() {
        let stale = source_failure("Producer playlist is stale");
        let text = stale["scene"]["nodes"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("stopped advancing"));
        assert!(text.contains("video is hidden"));
        assert!(text.contains("disk space"));
        assert_ne!(stale, source_failure("Source observation timed out"));
    }
    #[test]
    fn local_source_is_bounded_and_recognizes_ended_media() {
        let path = std::env::temp_dir().join(format!(
            "canvas-playlist-{}-{}.m3u8",
            std::process::id(),
            now_ms().unwrap()
        ));
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, "#EXTM3U\n#EXT-X-ENDLIST\n").unwrap();
        assert_eq!(
            hls_progress(&local_source(path_str).unwrap()).unwrap_err(),
            "Producer ended"
        );
        std::fs::write(&path, vec![b'a'; 65_537]).unwrap();
        assert!(
            local_source(path_str)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        std::fs::remove_file(path).unwrap();
    }
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
