//! Cooperative backend ownership. Same-user direct OS access remains authoritative.
use serde_json::{json, Value};
use std::{
    fs,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};

struct Gate {
    directory: PathBuf,
    _lock: fs::File,
}
impl Gate {
    fn acquire(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join("ownership.lock"))
            .map_err(|e| e.to_string())?;
        // One lock orders desktop inputs, submissions and revocations across projects.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self {
            directory: directory.into(),
            _lock: lock,
        })
    }
    fn state(&self, project: &str) -> Result<Value, String> {
        crate::unit(project)?;
        match fs::read_to_string(self.directory.join(format!("{project}.json"))) {
            Ok(raw) => {
                let mut state: Value = serde_json::from_str(&raw)
                    .map_err(|e| format!("Invalid ownership state: {e}"))?;
                // Existing v1 registrations predate epochs; migrate them at zero.
                if state.get("epoch").is_none() && state.is_object() {
                    state["epoch"] = json!(0);
                }
                match state["mode"].as_str() {
                    Some("human" | "agent")
                        if state["version"] == 1 && state["epoch"].as_u64().is_some() =>
                    {
                        Ok(state)
                    }
                    _ => Err("Invalid ownership state; refusing agent action".into()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(json!({"version":1,"project":project,"mode":"human","epoch":0}))
            }
            Err(e) => Err(e.to_string()),
        }
    }
    fn mode(&self, project: &str) -> Result<String, String> {
        Ok(self.state(project)?["mode"].as_str().unwrap().into())
    }
    fn grant(&self, project: &str, expected: u64) -> Result<Value, String> {
        if self.state(project)?["epoch"].as_u64() != Some(expected) {
            return Err(
                "Control changed after this task was prepared; delayed agent grant refused".into(),
            );
        }
        self.set(project, "agent")
    }
    fn set(&self, project: &str, mode: &str) -> Result<Value, String> {
        crate::unit(project)?;
        if !matches!(mode, "human" | "agent") {
            return Err("control mode must be human or agent".into());
        }
        let mut epoch = self.state(project)?["epoch"].as_u64().unwrap();
        if mode == "human" {
            epoch = epoch.checked_add(1).ok_or("Control epoch exhausted")?;
        }
        let state = json!({"version":1,"project":project,"mode":mode,"epoch":epoch});
        let temporary = self.directory.join(format!("{project}.pending"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(state.to_string().as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(temporary, self.directory.join(format!("{project}.json")))
            .map_err(|e| e.to_string())?;
        fs::File::open(&self.directory)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(state)
    }
}
fn directory() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var("HOME").map_err(|_| "HOME missing")?)
            .join(".local/state/dock/control"),
    )
}
pub fn status(project: &str) -> Result<Value, String> {
    let gate = Gate::acquire(&directory()?)?;
    gate.state(project)
}
pub fn grant(project: &str, expected_epoch: u64) -> Result<Value, String> {
    Gate::acquire(&directory()?)?.grant(project, expected_epoch)
}
pub fn set(project: &str, mode: &str) -> Result<Value, String> {
    Gate::acquire(&directory()?)?.set(project, mode)
}
pub fn with_agent(
    project: &str,
    action: impl FnOnce() -> Result<Value, String>,
) -> Result<Value, String> {
    let gate = Gate::acquire(&directory()?)?;
    if gate.mode(project)? != "agent" {
        return Err(format!("Human owns {project}; agent action refused"));
    }
    action()
}
/// Explicit human navigation shares the input lock, preventing focus/input races.
pub fn with_navigation(action: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    let _gate = Gate::acquire(&directory()?)?;
    action()
}
/// Revoke first; keep submissions excluded until the session owner acknowledges abort.
/// On abort failure the human latch remains persisted and the error is surfaced.
pub fn takeover(
    project: &str,
    abort: impl FnOnce() -> Result<Value, String>,
) -> Result<Value, String> {
    let gate = Gate::acquire(&directory()?)?;
    let state = gate.set(project, "human")?;
    let session = abort()?;
    Ok(
        json!({"project":project,"mode":"human","epoch":state["epoch"],"control":state,"session":session}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_human_and_revocation_persists_and_corruption_fails_closed() {
        let path = std::env::temp_dir().join(format!("dock-control-test-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        {
            let gate = Gate::acquire(&path).unwrap();
            assert_eq!(gate.mode("dock").unwrap(), "human");
            gate.set("dock", "agent").unwrap();
            assert_eq!(gate.mode("dock").unwrap(), "agent");
            gate.set("dock", "human").unwrap();
            assert!(gate.set("../outside", "agent").is_err());
        }
        {
            let gate = Gate::acquire(&path).unwrap();
            assert_eq!(gate.mode("dock").unwrap(), "human");
            fs::write(path.join("dock.json"), "{}").unwrap();
            assert!(gate.mode("dock").is_err());
        }
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn delayed_grant_cannot_undo_takeover_even_when_already_human() {
        let path = std::env::temp_dir().join(format!("dock-epoch-test-{}", std::process::id()));
        {
            let gate = Gate::acquire(&path).unwrap();
            gate.grant("dock", 0).unwrap();
            gate.set("dock", "human").unwrap();
            assert!(gate.grant("dock", 0).is_err());
            assert_eq!(gate.mode("dock").unwrap(), "human");
            gate.set("dock", "human").unwrap();
            assert!(gate.grant("dock", 1).is_err());
            assert_eq!(gate.grant("dock", 2).unwrap()["mode"], "agent");
        }
        fs::remove_dir_all(path).unwrap();
    }
}
