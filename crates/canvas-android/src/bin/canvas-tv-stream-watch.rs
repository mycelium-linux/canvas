//! Explicit source-health adapter -> revision-bound surface lease. No discovery,
//! no automatic workspace takeover, and no credentials in the descriptor.
use canvas_core::stream_lease::{failure_content, hls_progress, waiting_content, ProgressTracker};
use serde_json::{json, Value};
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
    checked_receipt(&bytes)
}

fn checked_receipt(bytes: &[u8]) -> Result<Value, Box<dyn std::error::Error>> {
    let reply: Value = serde_json::from_slice(bytes)?;
    if reply["ok"] != true {
        return Err("TV rejected presentation request; inspect its current revision".into());
    }
    Ok(reply)
}

struct Target<'a> {
    project: &'a str,
    surface: &'a str,
    url: &'a str,
}

/// Inspect selection first, then confirm producer progress immediately before
/// delivery. Slow inspection must not consume a newly issued presentation lease.
fn watch_once(
    target: &Target<'_>,
    tracker: &mut ProgressTracker,
    read_playlist: impl FnOnce() -> Result<String, Box<dyn std::error::Error>>,
    mut send: impl FnMut(&str) -> Result<Value, Box<dyn std::error::Error>>,
    clock: impl FnOnce() -> Result<u64, Box<dyn std::error::Error>>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let observed = send("inspect\n")?;
    if observed["data"]["catalog"]["selected_project"] != target.project {
        tracker.reset();
        return Ok(None);
    }
    let document = &observed["data"]["document"];
    let mut surface = document["surfaces"]
        .as_array()
        .ok_or("Missing surfaces")?
        .iter()
        .find(|surface| surface["id"] == target.surface)
        .ok_or("Surface not installed")?
        .clone();
    let mut failure = None;
    let content = match read_playlist().and_then(|raw| hls_progress(&raw).map_err(Into::into)) {
        Ok(marker) => tracker
            .observe(marker, target.url, clock()?)
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or_else(waiting_content),
        Err(error) => {
            tracker.reset();
            let message = error.to_string();
            let content = source_failure(&message);
            failure = Some(message);
            content
        }
    };
    if surface["content"] != content {
        surface["content"] = content;
        let revision = document["revision"].as_u64().ok_or("Missing revision")?;
        let request = json!({"schema_version":1,"request_id":"stream-availability","expected_revision":observed["revision"],
            "command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":document["id"],
                "base_revision":revision,"revision":revision.checked_add(1).ok_or("Revision overflow")?,
                "operations":[{"op":"upsert_surface","surface":surface}]}}});
        send(&request.to_string())?;
    }
    Ok(failure)
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
    let target = Target {
        project,
        surface: id,
        url,
    };
    loop {
        let started = Instant::now();
        let result = watch_once(
            &target,
            &mut tracker,
            || host.map_or_else(|| local_source(path), |host| source(host, path)),
            |request| control(&args[1], request),
            now_ms,
        );
        match result {
            Ok(Some(message)) => {
                if previous_error != message {
                    eprintln!("canvas-tv-stream-watch: {message}");
                    previous_error = message;
                }
            }
            Ok(None) => previous_error.clear(),
            Err(error) => eprintln!("canvas-tv-stream-watch: {error}"),
        }
        if let Some(delay) = Duration::from_secs(1).checked_sub(started.elapsed()) {
            std::thread::sleep(delay);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn inspection(project: &str) -> Value {
        json!({"ok":true,"revision":10,"data":{
            "catalog":{"selected_project":project},
            "document":{"id":"reversal-live","revision":2,"surfaces":[{
                "id":"preview","title":"Preview","content":{"kind":"scene","scene":{"nodes":[]}}
            }]}
        }})
    }

    fn playlist(sequence: u64) -> String {
        format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:{sequence}\nsegment-{sequence}.ts\n")
    }

    #[test]
    fn slow_inspection_does_not_age_new_leases_or_restart_renewals() {
        let target = Target {
            project: "reversal",
            surface: "preview",
            url: "http://192.168.2.67:18089/preview.m3u8",
        };
        let clock = Cell::new(0);
        let requests = RefCell::new(Vec::<Value>::new());
        let mut tracker = ProgressTracker::default();
        for sequence in 0..3 {
            watch_once(
                &target,
                &mut tracker,
                || Ok(playlist(sequence)),
                |raw| {
                    if raw == "inspect\n" {
                        clock.set(clock.get() + 4000);
                        Ok(inspection("reversal"))
                    } else {
                        requests.borrow_mut().push(serde_json::from_str(raw)?);
                        Ok(json!({"ok":true}))
                    }
                },
                || Ok(clock.get()),
            )
            .unwrap();
        }
        let requests = requests.borrow();
        let content = |index: usize| {
            &requests[index]["command"]["patch"]["operations"][0]["surface"]["content"]
        };
        assert_eq!(content(0)["kind"], "scene"); // Two observations establish progress.
        assert_eq!(content(1)["issued_at_unix_ms"], 8000);
        assert_eq!(content(1)["expires_at_unix_ms"], 13000);
        assert_eq!(content(2)["issued_at_unix_ms"], 12000);
        assert_eq!(content(1)["generation"], content(2)["generation"]);
        assert_eq!(
            canvas_core::stream_lease::present_content(content(1), 8000),
            canvas_core::stream_lease::present_content(content(2), 12000)
        );
        assert_eq!(
            canvas_core::stream_lease::present_content(content(2), 17000)["kind"],
            "scene"
        );
    }

    #[test]
    fn hidden_workspace_never_reads_or_writes_preview_source() {
        let target = Target {
            project: "reversal",
            surface: "preview",
            url: "http://192.168.2.67:18089/preview.m3u8",
        };
        let mut tracker = ProgressTracker::default();
        let result = watch_once(
            &target,
            &mut tracker,
            || panic!("Hidden source must not be polled"),
            |raw| {
                assert_eq!(raw, "inspect\n");
                Ok(inspection("verk-showcase"))
            },
            || panic!("Hidden source must not be timestamped"),
        );
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn rejected_and_malformed_owner_receipts_fail_loudly() {
        assert!(checked_receipt(br#"{"ok":false,"error":"stale revision"}"#).is_err());
        assert!(checked_receipt(br#"{"revision":11}"#).is_err());
        assert!(checked_receipt(b"not-json").is_err());
        assert!(checked_receipt(br#"{"ok":true,"revision":11}"#).is_ok());
        let target = Target {
            project: "reversal",
            surface: "preview",
            url: "http://192.168.2.67:18089/preview.m3u8",
        };
        let mut tracker = ProgressTracker::default();
        assert!(watch_once(
            &target,
            &mut tracker,
            || Ok(playlist(0)),
            |raw| {
                if raw == "inspect\n" {
                    Ok(inspection("reversal"))
                } else {
                    checked_receipt(br#"{"ok":false}"#)
                }
            },
            || Ok(1000)
        )
        .is_err());
    }
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
        assert!(local_source(path_str)
            .unwrap_err()
            .to_string()
            .contains("limit"));
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
