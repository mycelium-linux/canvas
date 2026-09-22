//! Sessions, shown as the agent roster the wall already knows how to draw.
//!
//! `canvas-agents` has been feeding that roster synthetic agents marked
//! `Provenance::Demo`, explicitly standing in "before any agent is wired to
//! it". This wires real ones to the same widget, so nothing new has to be
//! designed and the provenance badge disappears on its own once the values are
//! real — the badge going away *is* the signal that it worked.

use canvas_core::working_set::{AgentState, Attention, WorkingSet};
use canvas_widgets::{Provenance, WidgetSpec, roster::node_summary};

use crate::{Session, SessionState};

/// How many sessions a roster shows before it reports the rest as a count.
///
/// A wall tile has finite room, and `Roster` already carries `not_shown` so the
/// overflow is stated rather than silently dropped.
pub const ROSTER_CAPACITY: usize = 8;

fn agent_state(state: SessionState) -> AgentState {
    match state {
        // Blocked is the one that should pull the eye: something is waiting on
        // a person.
        SessionState::Blocked => AgentState::Blocked,
        SessionState::Working => AgentState::Working,
        // An ended session is still worth showing for a moment — it is how you
        // see that a run finished — and the working set evicts it on age.
        SessionState::Idle | SessionState::Ended => AgentState::Idle,
    }
}

/// One session as an entry in the roster.
pub fn attention(target: &str, session: &Session, sequence: u64) -> Attention {
    Attention {
        // Named by target and session so two control planes cannot collide on a
        // shared id, and so a viewer can tell which agent they are looking at.
        agent: format!("{target}/{}", session.id),
        project: session
            .project
            .clone()
            .unwrap_or_else(|| target.to_string()),
        observing: Some(session.id.clone()),
        title: session.title.clone(),
        state: agent_state(session.state),
        observed_at_ms: session.updated_at_ms,
        sequence,
    }
}

/// Build the roster widget for a set of live sessions.
///
/// `Provenance::Live` because these came from a control plane. Marking real
/// values as Demo would be as wrong as the reverse: the badge is the contract
/// that tells a viewer whether to believe the tile.
pub fn widget(id: &str, target: &str, sessions: &[Session], now_ms: u64) -> WidgetSpec {
    let mut working = WorkingSet::new(ROSTER_CAPACITY);
    // Most recently updated first, so the capacity cut keeps what is moving
    // rather than whatever the control plane happened to list first.
    let mut ordered: Vec<&Session> = sessions.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    for (sequence, session) in ordered.iter().enumerate() {
        working.observe(attention(target, session, sequence as u64), now_ms);
    }
    node_summary(id, &working.roster(now_ms), Provenance::Live, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, state: SessionState, updated: u64) -> Session {
        Session {
            id: id.into(),
            title: format!("work on {id}"),
            project: None,
            state,
            updated_at_ms: updated,
        }
    }

    #[test]
    fn a_blocked_session_is_blocked_on_the_roster() {
        let attention = attention("operator", &session("s1", SessionState::Blocked, 10), 0);
        assert_eq!(attention.state, AgentState::Blocked);
        assert_eq!(attention.agent, "operator/s1");
        assert_eq!(
            attention.project, "operator",
            "falls back to the target when the session names no project"
        );
    }

    /// Two control planes may mint the same session id; the roster must still
    /// show them as two agents.
    #[test]
    fn identical_ids_from_different_targets_do_not_collide() {
        let left = attention("operator", &session("s1", SessionState::Working, 1), 0);
        let right = attention("surf", &session("s1", SessionState::Working, 1), 0);
        assert_ne!(left.agent, right.agent);
    }

    #[test]
    fn real_sessions_are_marked_live_so_no_badge_is_drawn() {
        let spec = widget(
            "agents",
            "operator",
            &[session("s1", SessionState::Working, 10)],
            20,
        );
        assert_eq!(spec.data.provenance, Provenance::Live);
        assert_eq!(
            Provenance::Live.badge(),
            None,
            "live values must not wear a stand-in badge"
        );
    }

    #[test]
    fn the_roster_keeps_what_is_moving_when_it_overflows() {
        let mut sessions: Vec<Session> = (0..ROSTER_CAPACITY + 4)
            .map(|index| {
                session(
                    &format!("s{index:02}"),
                    SessionState::Working,
                    index as u64 * 10,
                )
            })
            .collect();
        sessions.reverse(); // arrive oldest-last, to prove ordering is applied
        let spec = widget("agents", "operator", &sessions, 1_000);

        let canvas_widgets::WidgetKind::StatusList { nodes, .. } = &spec.kind else {
            panic!("the roster widget is a status list");
        };
        assert!(nodes.len() <= ROSTER_CAPACITY, "capacity holds");
        let shown: Vec<&str> = nodes.iter().map(|node| node.label.as_str()).collect();
        assert!(
            shown.iter().any(|name| name.contains("s11")),
            "the newest session must be shown: {shown:?}"
        );
    }

    #[test]
    fn an_empty_roster_still_renders() {
        let spec = widget("agents", "operator", &[], 10);
        let canvas_widgets::WidgetKind::StatusList { nodes, .. } = &spec.kind else {
            panic!("status list");
        };
        assert!(nodes.is_empty());
    }
}
