//! Edgerunner: ACP underneath, reached over its REST surface.
//!
//! Only request shaping and the session-resolution policy live here. The
//! transport is in `main`, so the decisions worth arguing about are testable
//! without a control plane to talk to.

use crate::{EdgerunnerCreate, Prompt};
use crate::{Session, SessionState};

pub const CREATE_PATH: &str = "/v1/sessions";

pub fn create_body(prompt: &Prompt, template: &EdgerunnerCreate) -> serde_json::Value {
    let title = prompt
        .task
        .as_ref()
        .map(|task| {
            format!(
                "Canvas {} · {}",
                task.kind,
                prompt.project_id.as_deref().unwrap_or("task")
            )
        })
        .unwrap_or_else(|| "Canvas task".into());
    let mut body = serde_json::json!({
        "title": title,
        "repo_url": template.repo_url,
        "prompt": prompt.delivery_text(),
        "auto_start": true,
        "labels": {
            "project": prompt.project_id.as_deref().unwrap_or("unspecified"),
            "canvas.task_id": prompt.task.as_ref().map(|task| task.id.as_str()).unwrap_or("")
        }
    });
    if let Some(value) = &template.ref_name {
        body["ref"] = value.clone().into();
    }
    if let Some(value) = &template.runner {
        body["runner"] = value.clone().into();
    }
    if let Some(value) = &template.model {
        body["model"] = value.clone().into();
    }
    body
}

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
        // `updated_at` is what edgerunner actually sends; the other spellings
        // are kept because guessing wrong here is silent — every session would
        // be stamped "now", ties would break by id, and unpinned selection
        // would pick an arbitrary session while looking like it chose the
        // newest.
        updated_at_ms: value
            .get("updated_at")
            .or_else(|| value.get("updated_at_unix_ms"))
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

    #[test]
    fn canvas_creation_task_becomes_a_labeled_edgerunner_session() {
        let prompt = Prompt {
            target: "transmog".into(),
            text: "make a fan duct".into(),
            session: None,
            project_id: Some("transmog".into()),
            task: Some(canvas_core::task::TaskRef {
                id: "42".into(),
                kind: "Model".into(),
                state: Some("queued".into()),
            }),
        };
        let body = create_body(
            &prompt,
            &EdgerunnerCreate {
                repo_url: "https://github.com/FuturePresentLabs/transmog.git".into(),
                ref_name: None,
                runner: None,
                model: None,
            },
        );
        assert_eq!(
            body["repo_url"],
            "https://github.com/FuturePresentLabs/transmog.git"
        );
        assert_eq!(body["labels"]["project"], "transmog");
        assert_eq!(body["labels"]["canvas.task_id"], "42");
        assert_eq!(body["auto_start"], true);
        assert!(body["prompt"].as_str().unwrap().contains("Kind: Model"));
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

    /// Shaped from a real `/v1/sessions` reply rather than invented, because the
    /// field this build first guessed at (`updated_at_unix_ms`) is not the one
    /// edgerunner sends, and the guess failed silently: everything stamped
    /// "now", ordering lost, unpinned selection arbitrary.
    #[test]
    fn a_live_edgerunner_payload_parses_with_its_real_field_names() {
        let listed = serde_json::json!({
            "completed_at": 1789452772431u64,
            "created_at": 1789452135311u64,
            "id": "sess_1a0ac5e4a5e0003",
            "labels": {"edgerunner.session_id": "sess_1a0ac5e4a5e0003"},
            "run_id": "1a0ac5e4a5e0003",
            "status": "running",
            "title": "tui: Hello-World",
            "updated_at": 1789452777618u64,
            "vm_status": "online"
        });
        let parsed = parse_session(&listed, 1).expect("parses");
        assert_eq!(parsed.id, "sess_1a0ac5e4a5e0003");
        assert_eq!(parsed.state, SessionState::Working);
        assert_eq!(parsed.title, "tui: Hello-World");
        assert_eq!(
            parsed.updated_at_ms, 1789452777618,
            "the real timestamp must survive, not fall back to now"
        );
    }

    /// Most of a real listing is terminal, and "canceled" is the spelling in
    /// use — a near miss here would leave 38 dead sessions looking promptable.
    #[test]
    fn a_canceled_session_from_a_real_listing_is_ended() {
        let listed = serde_json::json!({
            "id": "sess_1a0a3a8ab8f0000", "status": "canceled",
            "updated_at": 1789452772431u64, "vm_status": "offline"
        });
        let parsed = parse_session(&listed, 0).expect("parses");
        assert!(!parsed.state.accepts_prompts());

        // A session with no title is normal in a real listing.
        assert_eq!(parsed.title, "session");
    }

    /// The selection that matters against real data: 72 sessions, 8 of them
    /// running, and the newest running one must win.
    #[test]
    fn the_newest_running_session_wins_against_a_mostly_dead_listing() {
        let mut listed: Vec<Session> = (0..38)
            .map(|index| {
                session(
                    &format!("dead{index:02}"),
                    SessionState::Ended,
                    9_000 + index,
                )
            })
            .collect();
        listed.push(session("sess_old_running", SessionState::Working, 100));
        listed.push(session("sess_new_running", SessionState::Working, 200));

        assert_eq!(
            choose_session(&listed, None).unwrap(),
            "sess_new_running",
            "a terminal session must never win on recency alone"
        );
    }
    #[test]
    fn a_session_without_an_id_is_skipped() {
        assert!(parse_session(&serde_json::json!({"state": "running"}), 0).is_none());
    }
}
