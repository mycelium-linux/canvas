//! Surf: a browsing agent driven by observe/execute.
//!
//! A separate adapter rather than a special case of edgerunner because the
//! verbs genuinely differ — surf takes a goal to execute and reports what it
//! sees, where edgerunner takes a message into an ACP session. Same session
//! noun, different protocol; that difference is exactly what `AdapterKind`
//! exists to name.

use crate::{Session, SessionState};

pub fn execute_path(session: &str) -> String {
    format!("/v1/sessions/{session}/execute")
}

pub fn observe_path(session: &str) -> String {
    format!("/v1/sessions/{session}/observe")
}

pub fn list_path() -> &'static str {
    "/v1/sessions"
}

/// A prompt becomes a goal: surf steps toward it rather than replying to it.
pub fn execute_body(text: &str) -> serde_json::Value {
    serde_json::json!({ "goal": text })
}

/// Read one surf session, tolerantly, using the same rules as elsewhere: only
/// the id is required, and an unrecognised state stays promptable.
pub fn parse_session(value: &serde_json::Value, now_ms: u64) -> Option<Session> {
    let id = value
        .get("id")
        .or_else(|| value.get("session_id"))
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let state = match value
        .get("state")
        .or_else(|| value.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "closed" | "ended" | "failed" | "terminated" => SessionState::Ended,
        "stepping" | "running" | "navigating" => SessionState::Working,
        _ => SessionState::Idle,
    };
    Some(Session {
        // A browsing session is best identified by where it is, which is what
        // someone watching the wall recognises.
        title: value
            .get("url")
            .or_else(|| value.get("title"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("browser session")
            .to_string(),
        project: value
            .get("project")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        state,
        updated_at_ms: value
            .get("updated_at_unix_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(now_ms),
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_body_match_the_worker_surface() {
        assert_eq!(execute_path("s1"), "/v1/sessions/s1/execute");
        assert_eq!(observe_path("s1"), "/v1/sessions/s1/observe");
        assert_eq!(list_path(), "/v1/sessions");
        // A goal, not a message: surf acts rather than answers.
        assert_eq!(execute_body("open the docs")["goal"], "open the docs");
    }

    #[test]
    fn a_browsing_session_is_titled_by_where_it_is() {
        let parsed = parse_session(
            &serde_json::json!({"id": "s1", "state": "navigating", "url": "https://fpl.dev"}),
            0,
        )
        .expect("parses");
        assert_eq!(parsed.title, "https://fpl.dev");
        assert_eq!(parsed.state, SessionState::Working);
    }

    #[test]
    fn an_unknown_surf_state_stays_promptable() {
        let parsed =
            parse_session(&serde_json::json!({"id": "s1", "state": "hydrating"}), 5).expect("parses");
        assert_eq!(parsed.state, SessionState::Idle);
        assert!(parsed.state.accepts_prompts());
        assert_eq!(parsed.updated_at_ms, 5);
    }

    #[test]
    fn a_closed_session_is_ended() {
        let parsed = parse_session(&serde_json::json!({"id": "s1", "status": "closed"}), 0)
            .expect("parses");
        assert!(!parsed.state.accepts_prompts());
    }

    #[test]
    fn a_session_without_an_id_is_skipped() {
        assert!(parse_session(&serde_json::json!({"url": "https://x"}), 0).is_none());
    }
}
