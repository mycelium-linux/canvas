//! Consume the credential owner's narrow Edgerunner API; no credentials or
//! private service routes are loaded or reconstructed in Dock.
use crate::bridge::{self, Owner};
use serde_json::Value;

fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn invoke(owner: Owner, action: &str, id: Option<&str>) -> Result<Value, String> {
    if id.is_some_and(|id| !identifier(id)) {
        return Err("Invalid existing session ID".into());
    }
    let mut command = bridge::command(owner, "owner-session");
    command.arg(action);
    if let Some(id) = id {
        command.arg(id);
    }
    let output = crate::bounded_output(&mut command, 30)?;
    if !output.status.success() {
        return Err("The credential owner could not authorize this session read; check its status and configured Edgerunner origin".into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Credential owner returned invalid session JSON")?;
    validate(action, id, value)
}
fn validate(action: &str, id: Option<&str>, value: Value) -> Result<Value, String> {
    if value["schema_version"] != 1 {
        return Err("Unsupported credential-owner session contract".into());
    }
    if action == "list" {
        if !value["sessions"].is_array() {
            return Err("Credential owner returned no session list".into());
        }
    } else if value["session"]["id"].as_str() != id {
        return Err("Credential owner returned another session".into());
    }
    Ok(value)
}
pub fn list(owner: Owner) -> Result<Value, String> {
    invoke(owner, "list", None)
}
pub fn status(owner: Owner, id: &str) -> Result<Value, String> {
    invoke(owner, "status", Some(id))
}
pub fn readiness(owner: Owner, id: &str) -> Result<Value, String> {
    invoke(owner, "readiness", Some(id))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cross_session_and_old_contract() {
        assert!(validate(
            "status",
            Some("a"),
            serde_json::json!({"schema_version":1,"session":{"id":"b"}})
        )
        .is_err());
        assert!(validate(
            "list",
            None,
            serde_json::json!({"schema_version":2,"sessions":[]})
        )
        .is_err());
        assert!(!identifier("$(bad)"));
    }
}
