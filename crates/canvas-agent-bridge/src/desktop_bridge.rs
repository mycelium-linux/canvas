//! Edgerunner sessions read through the desktop bridge, so no credential lands
//! here at all.
//!
//! `fpl-desktop-bridge` states its own invariant plainly — "the desktop
//! credential never leaves this owner" — and honours it by exposing narrow
//! operations rather than vending tokens: `owner-session list|status|readiness`
//! print JSON, having attached the owner's OIDC identity themselves. Shelling
//! out to that is strictly better than holding an mTLS identity and a bearer in
//! this process, because there is then nothing here to leak.
//!
//! The cost is that the surface is read-only. There is no brokered write, so
//! this adapter can populate a roster but cannot deliver a prompt; sending
//! still needs either a brokered write operation or a credential of its own.

use crate::{Session, SessionState};

/// Default binary name, resolved on PATH.
pub const DEFAULT_COMMAND: &str = "fpl-desktop-bridge";

/// Arguments that list the owner's sessions.
pub fn list_args() -> [&'static str; 2] {
    ["owner-session", "list"]
}

/// Map edgerunner's session status onto the four states a wall can act on.
///
/// Same reasoning as the direct adapter: an unrecognised status is idle rather
/// than ended, because guessing "ended" hides a live session.
fn classify(status: &str) -> SessionState {
    match status {
        "blocked" | "awaiting_approval" | "awaiting-approval" | "needs_approval" => {
            SessionState::Blocked
        }
        "running" | "active" | "working" | "prompting" => SessionState::Working,
        "completed" | "failed" | "cancelled" | "canceled" | "ended" | "terminated" => {
            SessionState::Ended
        }
        _ => SessionState::Idle,
    }
}

/// Read the sessions out of an `owner-session list` reply.
///
/// The bridge filters each session to an allowlist of fields and stamps a
/// schema version; an unexpected version is refused rather than guessed at,
/// because the fields this depends on could have moved.
pub fn parse_list(value: &serde_json::Value, now_ms: u64) -> anyhow::Result<Vec<Session>> {
    let version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    anyhow::ensure!(
        version == Some(1),
        "desktop bridge returned owner-session schema {version:?}, expected 1"
    );
    let sessions = value
        .get("sessions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("owner-session list returned no sessions array"))?;
    Ok(sessions
        .iter()
        .filter_map(|session| {
            let id = session
                .get("id")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            Some(Session {
                title: session
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .filter(|title| !title.is_empty())
                    .unwrap_or("session")
                    .to_string(),
                // The allowlist carries no project, so the caller labels these
                // by target rather than inventing one.
                project: None,
                state: classify(
                    session
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                ),
                // Nor any timestamp: every session is stamped with the moment it
                // was read, so the roster orders by id instead of pretending to
                // know which moved most recently.
                updated_at_ms: now_ms,
                id,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(sessions: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"schema_version": 1, "sessions": sessions})
    }

    #[test]
    fn the_command_is_the_narrow_read_the_bridge_exposes() {
        assert_eq!(list_args(), ["owner-session", "list"]);
        assert_eq!(DEFAULT_COMMAND, "fpl-desktop-bridge");
    }

    #[test]
    fn sessions_parse_from_the_allowlisted_fields() {
        let parsed = parse_list(
            &reply(serde_json::json!([
                {"id": "ses_a", "status": "running", "title": "fix the fan", "model": "opus"},
                {"id": "ses_b", "status": "completed", "title": ""}
            ])),
            500,
        )
        .expect("parses");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "ses_a");
        assert_eq!(parsed[0].state, SessionState::Working);
        assert_eq!(parsed[0].title, "fix the fan");
        assert_eq!(parsed[1].state, SessionState::Ended);
        assert_eq!(parsed[1].title, "session", "an empty title is not a title");
        assert!(
            parsed.iter().all(|session| session.updated_at_ms == 500),
            "no timestamp is exposed, so all are stamped at read time"
        );
    }

    /// The fields depended on here could move, so a version this build does not
    /// know is refused rather than parsed hopefully.
    #[test]
    fn an_unexpected_schema_version_is_refused() {
        let error = parse_list(
            &serde_json::json!({"schema_version": 2, "sessions": []}),
            0,
        )
        .expect_err("refused");
        assert!(error.to_string().contains("schema"), "{error}");
        assert!(parse_list(&serde_json::json!({"sessions": []}), 0).is_err());
    }

    #[test]
    fn a_missing_sessions_array_is_an_error_not_an_empty_roster() {
        // An empty roster and a broken read look identical on a wall, so they
        // must not be conflated.
        let error = parse_list(&serde_json::json!({"schema_version": 1}), 0)
            .expect_err("refused");
        assert!(error.to_string().contains("no sessions array"), "{error}");
    }

    #[test]
    fn an_empty_list_is_a_valid_empty_roster() {
        assert!(parse_list(&reply(serde_json::json!([])), 0).unwrap().is_empty());
    }

    #[test]
    fn a_session_without_an_id_is_skipped_not_fatal() {
        let parsed = parse_list(
            &reply(serde_json::json!([{"status": "running"}, {"id": "ses_ok", "status": "running"}])),
            0,
        )
        .expect("parses");
        assert_eq!(parsed.len(), 1, "one unusable entry must not lose the rest");
        assert_eq!(parsed[0].id, "ses_ok");
    }

    #[test]
    fn an_unknown_status_stays_promptable() {
        assert_eq!(classify("reticulating"), SessionState::Idle);
        assert!(classify("reticulating").accepts_prompts());
    }
}
