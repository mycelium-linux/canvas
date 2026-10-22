//! Bounded owner-local presentation exchange shared by widget and artifact hosts.
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
pub fn call(
    program: &str,
    endpoint: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut child = Command::new(program)
        .arg(endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing TV input")?
        .write_all(&serde_json::to_vec(request)?)?;
    let stdout = child.stdout.take().ok_or("missing TV output")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.take(64 * 1024 + 1).read_to_end(&mut bytes);
        let _ = tx.send(read.map(|_| bytes));
    });
    let bytes = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(value) => value?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("TV owner deadline exceeded".into());
        }
    };
    // A rejected revision intentionally exits nonzero but carries a structured
    // receipt. Keep that receipt so the caller can use its bounded retry policy.
    let status = child.wait()?;
    if bytes.len() > 64 * 1024 {
        return Err("TV owner response exceeds bound".into());
    }
    let receipt: serde_json::Value = serde_json::from_slice(&bytes)?;
    if !status.success() && receipt["ok"] != false {
        return Err("TV owner command failed without a rejection receipt".into());
    }
    Ok(receipt)
}
