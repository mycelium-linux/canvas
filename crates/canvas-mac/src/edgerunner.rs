//! Read and bind existing Edgerunner sessions through the user's existing SSH route.
//!
//! Edgerunner owns sessions, models, credentials, runtime provisioning and policy.
//! This client has no create-run/create-session/model override API. Binding only
//! persists a local reference to an explicitly selected existing owner session.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub ssh_host: String,
    pub base_url: String,
}
impl Default for Owner {
    fn default() -> Self {
        // Existing deployment: edgerunner/deploy/batty/docker-compose.yml.
        Self {
            ssh_host: "batty".into(),
            base_url: "http://192.168.1.40:8087".into(),
        }
    }
}
impl Owner {
    fn validate(&self) -> Result<(), String> {
        if self.ssh_host.is_empty()
            || self.ssh_host.starts_with('-')
            || !self
                .ssh_host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        {
            return Err("Edgerunner SSH host must be an existing plain SSH host alias".into());
        }
        let authority = self
            .base_url
            .strip_prefix("http://")
            .or_else(|| self.base_url.strip_prefix("https://"))
            .ok_or("Edgerunner base_url must be an HTTP(S) origin")?;
        if authority.is_empty()
            || !authority
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
        {
            return Err("Edgerunner base_url must be an origin without credentials, path, query, or fragment".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub version: u32,
    pub project: String,
    pub session_id: String,
    pub owner: Owner,
}
fn home() -> Result<PathBuf, String> {
    Ok(PathBuf::from(env::var_os("HOME").ok_or("HOME is missing")?))
}
fn owner() -> Result<Owner, String> {
    let file = home()?.join(".config/dock/edgerunner.json");
    let config = if file.exists() {
        serde_json::from_slice(&fs::read(file).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid Edgerunner connection config: {e}"))?
    } else {
        Owner::default()
    };
    config.validate()?;
    Ok(config)
}
fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid project/session identifier".into());
    }
    Ok(())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn request(owner: &Owner, path: &str) -> Result<Value, String> {
    owner.validate()?;
    if path != "/v1/sessions" {
        let id = path
            .strip_prefix("/v1/sessions/")
            .ok_or("Unsupported Edgerunner read operation")?;
        identifier(id)?;
    }
    let url = format!("{}{path}", owner.base_url);
    // No Authorization/identity header is manufactured. The existing SSH account
    // provides authenticated access to the owner's internal read API.
    let remote=format!("curl --fail --silent --show-error --connect-timeout 3 --max-time 12 --max-filesize {MAX_RESPONSE} --header 'accept: application/json' -- {}",quote(&url));
    let mut child = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            &owner.ssh_host,
            &remote,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Edgerunner SSH launch: {e}"))?;
    let stdout = child.stdout.take().ok_or("Edgerunner stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Edgerunner stderr unavailable")?;
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(16_385).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Edgerunner read timed out after 20 seconds".into());
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };
    let bytes = out
        .join()
        .map_err(|_| "Edgerunner output reader failed")?
        .map_err(|e| e.to_string())?;
    let errors = err
        .join()
        .map_err(|_| "Edgerunner error reader failed")?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "Edgerunner read through {} failed: {}",
            owner.ssh_host,
            String::from_utf8_lossy(&errors).trim()
        ));
    }
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Edgerunner response exceeds 4 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Edgerunner returned invalid JSON: {e}"))
}
fn summary(session: &Value) -> Result<Value, String> {
    let id = session["id"]
        .as_str()
        .ok_or("Edgerunner session has no id")?;
    identifier(id)?;
    let mut result = serde_json::Map::new();
    for key in [
        "id",
        "title",
        "repo_url",
        "ref",
        "status",
        "model",
        "agent",
        "run_id",
        "opencode_session_id",
        "vm_status",
        "agent_status",
        "idle_expires_at",
        "created_at",
        "updated_at",
    ] {
        if let Some(value) = session.get(key) {
            result.insert(key.into(), value.clone());
        }
    }
    Ok(Value::Object(result))
}
fn status_at(owner: &Owner, session_id: &str) -> Result<Value, String> {
    identifier(session_id)?;
    let session = request(owner, &format!("/v1/sessions/{session_id}"))?;
    if session["id"] != session_id {
        return Err("Edgerunner session identity acknowledgement mismatch".into());
    }
    Ok(json!({"owner":"edgerunner","connection":owner,"session":summary(&session)?}))
}
/// Lists existing owner sessions; does not create or claim any of them.
pub fn list() -> Result<Value, String> {
    let owner = owner()?;
    let result = request(&owner, "/v1/sessions")?;
    let sessions = result["sessions"]
        .as_array()
        .ok_or("Edgerunner returned no session list")?
        .iter()
        .map(summary)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"owner":"edgerunner","connection":owner,"sessions":sessions}))
}
pub fn status(session_id: &str) -> Result<Value, String> {
    status_at(&owner()?, session_id)
}
fn path(project: &str) -> Result<PathBuf, String> {
    identifier(project)?;
    Ok(home()?
        .join("Library/Application Support/Dock/edgerunner-bindings")
        .join(format!("{project}.json")))
}
/// Returns only an explicit user-selected local binding, never an inferred session.
pub fn binding(project: &str) -> Result<Option<Binding>, String> {
    let file = path(project)?;
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let binding: Binding =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid Edgerunner binding: {e}"))?;
    if binding.version != 1 || binding.project != project {
        return Err("Edgerunner binding identity/version mismatch".into());
    }
    identifier(&binding.session_id)?;
    binding.owner.validate()?;
    Ok(Some(binding))
}
/// Explicitly binds an existing owner session. Makes no remote mutation.
pub fn bind(project: &str, session_id: &str) -> Result<Value, String> {
    let file = path(project)?;
    identifier(session_id)?;
    let owner = owner()?;
    let current = status_at(&owner, session_id)?;
    let binding = Binding {
        version: 1,
        project: project.into(),
        session_id: session_id.into(),
        owner,
    };
    let directory = file.parent().ok_or("Invalid binding directory")?;
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let temp = file.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temp).map_err(|e| e.to_string())?;
    output
        .write_all(&serde_json::to_vec_pretty(&binding).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temp, file).map_err(|e| e.to_string())?;
    Ok(json!({"binding":binding,"session":current["session"],"remote_mutated":false}))
}
pub fn status_for_project(project: &str) -> Result<Value, String> {
    match binding(project)? {
        Some(binding) => status_at(&binding.owner, &binding.session_id),
        None => Ok(
            json!({"owner":"edgerunner","project":project,"bound":false,"note":"Select an existing Edgerunner session; Dock does not create an agent runtime"}),
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_remote_command_and_identity_injection() {
        for host in [
            "-oProxyCommand=bad",
            "batty;id",
            "batty\nwhoami",
            "user@batty",
        ] {
            assert!(Owner {
                ssh_host: host.into(),
                ..Owner::default()
            }
            .validate()
            .is_err());
        }
        for url in [
            "http://u:p@host:8087",
            "http://host/path",
            "http://host?key=value",
            "http://host;id",
        ] {
            assert!(Owner {
                base_url: url.into(),
                ..Owner::default()
            }
            .validate()
            .is_err());
        }
        for id in ["../runs", "x/messages", "x?key=value", ""] {
            assert!(identifier(id).is_err());
        }
    }
    #[test]
    fn projects_only_public_session_fields() {
        let value=summary(&json!({"id":"sess_123","model":"owner/model","password":"should-not-appear","labels":{"private":"value"}})).unwrap();
        assert_eq!(value["model"], "owner/model");
        assert!(value.get("password").is_none());
        assert!(value.get("labels").is_none());
    }
    #[test]
    fn quote_preserves_literal_shell_characters() {
        assert_eq!(quote("a'b $(id)"), "'a'\\''b $(id)'");
    }
}
