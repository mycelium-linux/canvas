//! Bounded owner snapshots; network I/O never blocks a video frame.
use anyhow::{Context, Result, ensure};
use canvas::map::spatial::{SpatialFeatureSet, decode_panopticon_snapshot};
use serde::Deserialize;
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub key_file: String,
}

pub fn expire(features: &mut SpatialFeatureSet, now: u64) {
    features.features.retain(|feature| {
        feature
            .summary
            .get("expires_at_unix")
            .and_then(serde_json::Value::as_u64)
            .is_none_or(|expiry| expiry.saturating_mul(1_000) > now)
    });
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn fresh(features: &SpatialFeatureSet, now: u64) -> bool {
    features.generated_at_unix_ms <= now.saturating_add(5_000)
        && now.saturating_sub(features.generated_at_unix_ms)
            < features.max_age_secs.saturating_mul(1_000).min(60_000)
}

fn fetch(url: &str, key_file: &str) -> Result<SpatialFeatureSet> {
    // Keep the credential out of argv, logs and the renderer. No redirects.
    ensure!(
        url.starts_with("http://127.0.0.1:"),
        "owner URL must use loopback"
    );
    ensure!(!url.contains(['\n', '\r', '"', '\\']), "invalid owner URL");
    let key = std::fs::read_to_string(key_file).context("read owner credential")?;
    let key = key.trim();
    ensure!(
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "invalid owner credential"
    );
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "--silent",
            "--fail",
            "--noproxy",
            "*",
            "--max-time",
            "4",
            "--max-filesize",
            "16777216",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    writeln!(
        child.stdin.take().context("curl stdin")?,
        "url = \"{url}\"\nheader = \"X-Internal-Key: {key}\""
    )?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "owner request failed ({})",
        output.status
    );
    let mut features = decode_panopticon_snapshot(&output.stdout).map_err(anyhow::Error::msg)?;
    ensure!(fresh(&features, now_ms()), "owner snapshot is stale");
    expire(&mut features, now_ms());
    Ok(features)
}

pub fn start(source: &Source) -> mpsc::Receiver<Result<SpatialFeatureSet>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let url = source.url.clone();
    let key_file = source.key_file.clone();
    std::thread::spawn(move || {
        loop {
            let update = fetch(&url, &key_file);
            match sender.try_send(update) {
                Err(mpsc::TrySendError::Disconnected(_)) => break,
                _ => {}
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_owner_semantic_metadata_without_changing_point_geometry() {
        let bytes = include_bytes!("../../canvas/tests/fixtures/spatial_snapshot_v1.json");
        let expected = decode_panopticon_snapshot(bytes).unwrap();
        let mut wire: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        wire["features"][0]["semantic"] = serde_json::json!({"kind":"vehicle","class":"vessel"});
        assert_eq!(
            decode_panopticon_snapshot(&serde_json::to_vec(&wire).unwrap()).unwrap(),
            expected
        );
    }
    #[test]
    fn refuses_remote_and_injected_owner_urls_before_reading_credentials() {
        assert!(
            fetch("https://example.org/snapshot", "/missing")
                .unwrap_err()
                .to_string()
                .contains("loopback")
        );
        assert!(
            fetch("http://127.0.0.1:18581/\nheader", "/missing")
                .unwrap_err()
                .to_string()
                .contains("invalid owner URL")
        );
    }
    #[test]
    fn rejects_expired_and_future_snapshots() {
        let mut set = decode_panopticon_snapshot(include_bytes!(
            "../../canvas/tests/fixtures/spatial_snapshot_v1.json"
        ))
        .unwrap();
        set.generated_at_unix_ms = 100_000;
        set.max_age_secs = 30;
        assert!(fresh(&set, 129_999));
        assert!(!fresh(&set, 130_000));
        assert!(!fresh(&set, 94_999));
        set.max_age_secs = u64::MAX;
        assert!(!fresh(&set, 160_000));
    }

    #[test]
    fn expires_individual_entities_at_the_boundary() {
        let mut set = decode_panopticon_snapshot(include_bytes!(
            "../../canvas/tests/fixtures/spatial_snapshot_v1.json"
        ))
        .unwrap();
        set.features.truncate(1);
        set.features[0]
            .summary
            .insert("expires_at_unix".into(), serde_json::json!(100));
        let mut before = set.clone();
        expire(&mut before, 99_999);
        assert_eq!(before.features.len(), 1);
        expire(&mut set, 100_000);
        assert!(set.features.is_empty());
    }
}
