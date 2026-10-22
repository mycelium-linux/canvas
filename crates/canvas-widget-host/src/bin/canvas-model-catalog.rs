//! Capture an explicitly selected owner's immutable reconstruction inventory.
//! This is presentation metadata, not discovery or dataset admission.
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    receipt: String,
    step: String,
    sha256: String,
    geometry: f64,
    training_admitted: bool,
}
#[derive(Serialize)]
struct Catalog {
    schema: &'static str,
    source_root: String,
    observed_at: String,
    assets: Vec<Asset>,
}
fn validate(mut assets: Vec<Asset>, root: &Path) -> Result<Vec<Asset>, String> {
    if assets.is_empty() || assets.len() > 100_000 {
        return Err("inventory must contain 1..100000 entries".into());
    }
    for asset in &assets {
        for path in [&asset.step, &asset.receipt] {
            if !Path::new(path).starts_with(root)
                || path.contains("..")
                || !path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
            {
                return Err(
                    "owner artifact escapes the selected root or has unsafe characters".into(),
                );
            }
        }
        if !asset.step.ends_with("/rebuilt.step")
            || !asset.receipt.ends_with("/verified.json")
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || !asset.geometry.is_finite()
        {
            return Err("invalid reconstructed model identity/evidence".into());
        }
    }
    assets.sort_by(|a, b| a.step.cmp(&b.step));
    if assets.windows(2).any(|pair| pair[0].step == pair[1].step) {
        return Err("duplicate source model path".into());
    }
    Ok(assets)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let destination = PathBuf::from(args.next().ok_or("destination required")?);
    let root = PathBuf::from(args.next().ok_or("explicit owner root required")?);
    if !root.is_absolute() {
        return Err("owner root must be absolute".into());
    }
    let program = args.next().ok_or("trusted inventory command required")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("source stdout missing")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let bytes = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => result?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("owner inventory read exceeded 20-second deadline".into());
        }
    };
    if bytes.len() as u64 > MAX_BYTES {
        let _ = child.kill();
        return Err("inventory exceeds allocation bound".into());
    }
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err("owner inventory command failed".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("owner inventory process exceeded deadline".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let assets = std::str::from_utf8(&bytes)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Asset>)
        .collect::<Result<Vec<_>, _>>()?;
    let assets = validate(assets, &root)?;
    let count = assets.len();
    let catalog = Catalog {
        schema: "canvas.model-catalog.v1",
        source_root: root.to_string_lossy().into_owned(),
        observed_at: chrono::Utc::now().to_rfc3339(),
        assets,
    };
    fs::create_dir_all(destination.parent().ok_or("destination parent required")?)?;
    let staged = destination.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&staged, serde_json::to_vec(&catalog)?)?;
    fs::rename(staged, destination)?;
    println!("captured {count} owner-verified model entries");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn asset(id: &str) -> Asset {
        Asset {
            step: format!("/owner/{id}/rebuilt.step"),
            receipt: format!("/owner/{id}/verified.json"),
            sha256: "a".repeat(64),
            geometry: 0.0,
            training_admitted: false,
        }
    }
    #[test]
    fn preserves_all_parts_in_stable_order() {
        let items = validate(
            vec![asset("d"), asset("b"), asset("a"), asset("c")],
            Path::new("/owner"),
        )
        .unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].step, "/owner/a/rebuilt.step");
        assert_eq!(items[3].step, "/owner/d/rebuilt.step");
    }
    #[test]
    fn refuses_escape_and_duplicates() {
        assert!(validate(vec![asset("../outside")], Path::new("/owner")).is_err());
        assert!(validate(vec![asset("a"), asset("a")], Path::new("/owner")).is_err());
    }
}
