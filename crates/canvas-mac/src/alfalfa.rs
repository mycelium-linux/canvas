//! Read project context from the existing local Alfalfa owner.
//! No copied board, project writer, scheduler, or agent runtime lives here.
use serde_json::{json, Value};
use std::process::{Command, Stdio};

fn identifier(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid Alfalfa project identifier".into());
    }
    Ok(())
}

fn origin() -> Result<String, String> {
    let port = std::env::var("DOCK_ALFALFA_PORT")
        .unwrap_or_else(|_| "8791".into())
        .parse::<u16>()
        .map_err(|_| "DOCK_ALFALFA_PORT must be a local TCP port")?;
    if port == 0 {
        return Err("Alfalfa port must be nonzero".into());
    }
    // Hosted authentication belongs to fpl-auth/desktop bridge. Never send an
    // unauthenticated request or an invented identity header to a hosted origin.
    Ok(format!("http://127.0.0.1:{port}"))
}

pub const AUTHORIZATION_DENIED: &str =
    "Alfalfa authorization denied; sign in again with the credential owner";
static AUTHENTICATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn owner_available(command: &Command) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = std::path::Path::new(command.get_program());
    let runnable = |p: &std::path::Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if executable.components().count() > 1 {
        return runnable(executable);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| runnable(&dir.join(executable)))
    })
}

fn envelope(value: Value) -> Result<Option<Value>, String> {
    if value["schema_version"] != 1 {
        return Err("Unsupported credential owner project-read contract".into());
    }
    match value["ok"].as_bool() {
        Some(true) if value.get("data").is_some() => Ok(Some(value["data"].clone())),
        Some(false) => match value["error"]["code"].as_str() {
            Some("credential_unavailable") => Ok(None),
            Some("authorization_denied") => Err(AUTHORIZATION_DENIED.into()),
            _ => Err(
                "Authenticated Alfalfa read failed; check the credential owner and project service"
                    .into(),
            ),
        },
        _ => Err("Invalid credential owner project-read response".into()),
    }
}

fn get(path: &str) -> Result<Value, String> {
    use std::sync::atomic::Ordering;
    let origin = origin()?;
    let mut command = super::bridge::command(super::bridge::Owner::Mac, "project-read");
    if owner_available(&command) {
        match path {
            "/api/v1/projects" => {
                command.arg("list");
            }
            "/api/v1/project-companies" => {
                command.arg("companies");
            }
            "/api/v1/activity" => {
                command.arg("activity");
            }
            _ => {
                let id = path
                    .strip_prefix("/api/v1/projects/")
                    .and_then(|p| p.strip_suffix("/context"))
                    .ok_or("Unsupported Alfalfa route")?;
                identifier(id)?;
                command.args(["context", id]);
            }
        }
        command
            .args([
                "--port",
                origin.rsplit(':').next().ok_or("Missing local port")?,
            ])
            .stdin(Stdio::null());
        let output = super::bounded_output(&mut command, 12)?;
        if !output.status.success() {
            return Err("Credential owner project-read unavailable; update the owner CLI".into());
        }
        let value =
            serde_json::from_slice(&output.stdout).map_err(|_| "Invalid credential owner JSON")?;
        match envelope(value) {
            Ok(Some(data)) => {
                AUTHENTICATED.store(true, Ordering::SeqCst);
                return Ok(data);
            }
            Ok(None) => {}
            Err(error) => {
                AUTHENTICATED.store(true, Ordering::SeqCst);
                return Err(error);
            }
        }
    }
    if AUTHENTICATED.load(Ordering::SeqCst) {
        return Err(AUTHORIZATION_DENIED.into());
    }
    if path == "/api/v1/project-companies" {
        return Err(
            "Company memberships require an installed and enrolled FPL Desktop Bridge".into(),
        );
    }
    legacy_get(path)
}

fn legacy_get(path: &str) -> Result<Value, String> {
    let output = super::bounded_output(
        Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--noproxy",
                "*",
                "--connect-timeout",
                "2",
                "--max-time",
                "5",
                "--max-filesize",
                "4194304",
                "--write-out",
                "\n%{http_code}",
                "--header",
                "Accept: application/json",
                "--",
                &format!("{}{path}", origin()?),
            ])
            .stdin(Stdio::null()),
        7,
    )?;
    if !output.status.success() {
        return Err(
            "Alfalfa project service is unavailable; existing workspace remains available".into(),
        );
    }
    if output.stdout.len() > 4 * 1024 * 1024 {
        return Err("Alfalfa context exceeds 4 MiB".into());
    }
    let split = output
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("Missing Alfalfa response status")?;
    let (body, status) = output.stdout.split_at(split);
    match &status[1..] {
        b"401" | b"403" => {
            AUTHENTICATED.store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(AUTHORIZATION_DENIED.into());
        }
        b"200" => {}
        _ => return Err("Alfalfa project service returned an unsuccessful status".into()),
    }
    serde_json::from_slice(body).map_err(|_| "Alfalfa returned invalid project JSON".into())
}

pub fn list() -> Result<Value, String> {
    let value = get("/api/v1/projects")?;
    if !value.is_array() {
        return Err("Alfalfa project list has an unsupported shape".into());
    }
    Ok(value)
}

/// The portfolio's navigable workspaces — the folders a person names out loud.
///
/// This is deliberately not the project list. Syncing a desktop per repository
/// produced 380 of them, 367 named `gh-<number>`, which is not a set anyone
/// navigates: the workspace someone means by "Ohm" is the folder `ohm`, and
/// `gh-1288853134` is one paused repository inside it.
///
/// Fetched over the owner's local origin rather than `project-read`, because the
/// installed desktop bridge answers "Unsupported project read operation" for
/// this action and routing it through the CLI would hard-fail the sync. Teaching
/// the bridge a `workspaces` action is the follow-up; until then this is the
/// same local origin and trust posture as the existing catalog fallback.
pub fn workspaces() -> Result<Value, String> {
    let value = legacy_get("/api/v1/project-workspaces")?;
    if !value.is_array() {
        return Err("Alfalfa workspace list has an unsupported shape".into());
    }
    Ok(value)
}

/// Canonical company memberships verified by the project owner service.
pub fn companies() -> Result<Value, String> {
    get("/api/v1/project-companies")
}

pub fn activity() -> Result<Value, String> {
    get("/api/v1/activity")
}

fn validate_context(id: &str, value: Value) -> Result<Value, String> {
    if value
        .pointer("/definition/project/id")
        .and_then(Value::as_str)
        != Some(id)
    {
        return Err("Alfalfa returned a different project; context was not attached".into());
    }
    if !value
        .pointer("/definition/resource")
        .is_some_and(Value::is_array)
    {
        return Err("Alfalfa context is missing its resource list".into());
    }
    Ok(value)
}

pub fn context(id: &str) -> Result<Value, String> {
    identifier(id)?;
    validate_context(id, get(&format!("/api/v1/projects/{id}/context"))?)
}

/// Recall can attach exact project identity without guessing from a repo name.
/// Absence is a visible readiness state, not permission to create a project/run.
pub fn recall(id: &str) -> Value {
    match context(id) {
        Ok(context) => json!({"state":"ready", "owner":"alfalfa", "context":context}),
        Err(error) => json!({"state":"unavailable", "owner":"alfalfa", "reason":error}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_denial_never_allows_legacy_fallback() {
        assert_eq!(
            envelope(
                json!({"schema_version":1,"ok":false,"error":{"code":"authorization_denied"}})
            )
            .unwrap_err(),
            AUTHORIZATION_DENIED
        );
        assert!(envelope(
            json!({"schema_version":1,"ok":false,"error":{"code":"credential_unavailable"}})
        )
        .unwrap()
        .is_none());
        assert!(envelope(json!({"schema_version":2,"ok":true,"data":[]})).is_err());
        assert!(envelope(json!({"schema_version":1,"ok":true})).is_err());
        assert!(envelope(
            json!({"schema_version":1,"ok":false,"error":{"code":"owner_unavailable"}})
        )
        .is_err());
    }
    #[test]
    fn mismatched_project_cannot_supply_context() {
        assert!(validate_context(
            "dock",
            json!({"definition":{"project":{"id":"other"},"resource":[]}})
        )
        .is_err());
        assert!(identifier("../other").is_err());
        assert!(identifier("dock?project=other").is_err());
    }
    #[test]
    fn owner_board_links_and_source_context_are_preserved() {
        let value = json!({"definition":{"project":{"id":"dock"},"resource":[{"kind":"board","uri":"https://board.example/project/dock"}]},"sources":[{"content":"Owner briefing"}]});
        assert_eq!(validate_context("dock", value.clone()).unwrap(), value);
    }
}
