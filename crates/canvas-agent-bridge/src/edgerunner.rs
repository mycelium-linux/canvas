//! Edgerunner: ACP underneath, reached over its REST surface.
//!
//! Only request shaping and the session-resolution policy live here. The
//! transport is in `main`, so the decisions worth arguing about are testable
//! without a control plane to talk to.

use crate::{Session, SessionState};

/// Where a prompt is delivered for a given session.
pub fn message_path(session: &str) -> String {
    format!("/v1/sessions/{session}/messages")
}

/// Where sessions are listed, narrowed to a project when one is configured.
pub fn list_path(project: Option<&str>) -> String {
    match project {
        Some(project) => format!("/v1/sessions?project={project}"),
        None => "/v1/sessions".to_string(),
    }
}

/// The body of a prompt delivery.
pub fn message_body(text: &str) -> serde_json::Value {
    serde_json::json!({ "content": text, "role": "user" })
}

/// Read one session out of an edgerunner listing, tolerantly.
///
/// Tolerant on purpose: edgerunner reports more about a session than a wall
/// needs, and a field appearing or moving must not stop the roster rendering.
/// Only the id is required — a session without one cannot be addressed and so
/// cannot be shown.
pub fn parse_session(value: &serde_json::Value, now_ms: u64) -> Option<Session> {
    let id = value
        .get("id")
        .or_else(|| value.get("session_id"))
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let raw_state = value
        .get("state")
        .or_else(|| value.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let awaiting = value
        .get("pending_approval")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some(Session {
        title: value
            .get("title")
            .or_else(|| value.get("task"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("session")
            .to_string(),
        project: value
            .get("project")
            .or_else(|| value.get("repo"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        state: classify(raw_state, awaiting),
        updated_at_ms: value
            .get("updated_at_unix_ms")
            .or_else(|| value.get("updated_at_ms"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(now_ms),
        id,
    })
}

/// Map edgerunner's vocabulary onto the four states a wall can act on.
///
/// An explicit approval flag outranks the reported state: a session that is
/// "running" but waiting on a human is blocked as far as anyone watching is
/// concerned, and that is the one state worth walking across the room for.
fn classify(state: &str, awaiting_approval: bool) -> SessionState {
    if awaiting_approval {
        return SessionState::Blocked;
    }
    match state {
        "blocked" | "awaiting_approval" | "awaiting-approval" | "needs_approval" => {
            SessionState::Blocked
        }
        "running" | "active" | "working" | "prompting" => SessionState::Working,
        "completed" | "failed" | "cancelled" | "canceled" | "ended" | "terminated" => {
            SessionState::Ended
        }
        // Unknown states are idle rather than ended: guessing "ended" would
        // hide a live session and silently refuse prompts to it.
        _ => SessionState::Idle,
    }
}

/// Which session a prompt should go to.
///
/// A pinned session is used as given, even if the listing does not mention it —
/// the listing may be narrowed or stale, and refusing would override an
/// explicit instruction.
///
/// Unpinned resolves to the most recently updated session that still accepts
/// prompts. Deliberately not "create a new session": a prompt typed on a wall
/// is a continuation of what is being watched, and silently starting a fresh
/// agent would spend real money and lose the context the person could see.
pub fn choose_session(sessions: &[Session], pinned: Option<&str>) -> anyhow::Result<String> {
    if let Some(pinned) = pinned {
        return Ok(pinned.to_string());
    }
    let mut live: Vec<&Session> = sessions
        .iter()
        .filter(|session| session.state.accepts_prompts())
        .collect();
    live.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    live.first()
        .map(|session| session.id.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no live session to prompt: {} listed, all ended",
                sessions.len()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, state: SessionState, updated: u64) -> Session {
        Session {
            id: id.into(),
            title: "t".into(),
            project: None,
            state,
            updated_at_ms: updated,
        }
    }

    #[test]
    fn paths_and_body_match_the_rest_surface() {
        assert_eq!(message_path("ses_1"), "/v1/sessions/ses_1/messages");
        assert_eq!(list_path(None), "/v1/sessions");
        assert_eq!(list_path(Some("dock")), "/v1/sessions?project=dock");
        assert_eq!(message_body("hi")["content"], "hi");
        assert_eq!(message_body("hi")["role"], "user");
    }

    /// Waiting on a person is the state worth crossing the room for, so an
    /// approval flag outranks whatever the session calls itself.
    #[test]
    fn awaiting_approval_outranks_the_reported_state() {
        assert_eq!(classify("running", true), SessionState::Blocked);
        assert_eq!(classify("running", false), SessionState::Working);
        assert_eq!(classify("awaiting_approval", false), SessionState::Blocked);
    }

    /// A state this build has never heard of must not be read as finished:
    /// that would hide a live session and refuse prompts to it.
    #[test]
    fn an_unknown_state_is_idle_not_ended() {
        assert_eq!(classify("reticulating", false), SessionState::Idle);
        assert!(classify("reticulating", false).accepts_prompts());
        assert_eq!(classify("failed", false), SessionState::Ended);
    }

    #[test]
    fn a_pinned_session_is_used_even_if_the_listing_omits_it() {
        let listed = [session("ses_a", SessionState::Working, 10)];
        // The listing may be narrowed to a project or simply stale; an explicit
        // instruction outranks it.
        assert_eq!(
            choose_session(&listed, Some("ses_elsewhere")).unwrap(),
            "ses_elsewhere"
        );
    }

    #[test]
    fn unpinned_picks_the_most_recently_updated_live_session() {
        let listed = [
            session("ses_old", SessionState::Working, 10),
            session("ses_new", SessionState::Idle, 99),
            session("ses_newest_but_done", SessionState::Ended, 500),
        ];
        assert_eq!(choose_session(&listed, None).unwrap(), "ses_new");
    }

    /// No live session is an error, never a new one: starting a fresh agent
    /// would spend money and drop the context the person was looking at.
    #[test]
    fn nothing_live_is_an_error_rather_than_a_new_session() {
        let listed = [session("ses_done", SessionState::Ended, 10)];
        let error = choose_session(&listed, None).expect_err("should refuse");
        assert!(error.to_string().contains("no live session"), "{error}");
        assert!(choose_session(&[], None).is_err(), "empty listing too");
    }

    #[test]
    fn ties_break_deterministically_so_two_screens_agree() {
        let listed = [
            session("ses_b", SessionState::Working, 42),
            session("ses_a", SessionState::Working, 42),
        ];
        assert_eq!(choose_session(&listed, None).unwrap(), "ses_a");
    }

    #[test]
    fn a_session_parses_from_either_field_spelling() {
        let modern = serde_json::json!({
            "id": "ses_1", "state": "running", "title": "fix the fan",
            "project": "dock", "updated_at_unix_ms": 1234u64
        });
        let parsed = parse_session(&modern, 0).expect("parses");
        assert_eq!(parsed.id, "ses_1");
        assert_eq!(parsed.state, SessionState::Working);
        assert_eq!(parsed.project.as_deref(), Some("dock"));
        assert_eq!(parsed.updated_at_ms, 1234);

        let older = serde_json::json!({"session_id": "ses_2", "status": "completed", "task": "x"});
        let parsed = parse_session(&older, 77).expect("parses");
        assert_eq!(parsed.id, "ses_2");
        assert_eq!(parsed.state, SessionState::Ended);
        assert_eq!(parsed.updated_at_ms, 77, "falls back to now, not zero");
    }

    #[test]
    fn a_session_without_an_id_is_skipped() {
        assert!(parse_session(&serde_json::json!({"state": "running"}), 0).is_none());
    }
}
