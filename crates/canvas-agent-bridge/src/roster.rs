//! Sessions, shown as the agent roster the wall already knows how to draw.
//!
//! `canvas-agents` has been feeding that roster synthetic agents marked
//! `Provenance::Demo`, explicitly standing in "before any agent is wired to
//! it". This wires real ones to the same widget, so nothing new has to be
//! designed and the provenance badge disappears on its own once the values are
//! real — the badge going away *is* the signal that it worked.

use canvas_core::working_set::{AgentState, Attention, Freshness, Roster, Slot};
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

/// The payload of a `canvas.widget.snapshot.v1` event for these sessions.
///
/// `widget` carries the tagged widget *kind* and nothing else. A whole
/// `WidgetSpec` is refused — "unknown field `schema_version`, expected `title`
/// or `nodes`" — because Canvas owns the surface's identity, chrome and theme
/// and a producer may replace only the kind body and the source-health block.
/// Sending the spec was silently dropping every snapshot.
pub fn snapshot(id: &str, target: &str, sessions: &[Session], now_ms: u64) -> serde_json::Value {
    let spec = widget(id, target, sessions, now_ms);
    serde_json::json!({
        "widget": serde_json::to_value(&spec.kind).unwrap_or_default(),
        "data": serde_json::to_value(&spec.data).unwrap_or_default(),
    })
}

/// Build the roster widget for a set of live sessions.
///
/// `Provenance::Live` because these came from a control plane. Marking real
/// values as Demo would be as wrong as the reverse: the badge is the contract
/// that tells a viewer whether to believe the tile.
pub fn widget(id: &str, target: &str, sessions: &[Session], now_ms: u64) -> WidgetSpec {
    // The roster is built directly rather than through `WorkingSet`, whose
    // EVICT_AFTER_MS drops anything unheard-from for two minutes. That is right
    // for attention heartbeats and wrong for agent sessions, which live for
    // days: pushed through it, 72 real sessions rendered as an empty tile
    // because every one of them was last updated long ago.
    let mut ordered: Vec<&Session> = sessions.iter().collect();
    ordered.sort_by(|left, right| {
        // Live before finished, then newest first. A wall has room for a
        // handful, and eight running sessions matter more than the eight most
        // recently abandoned ones.
        right
            .state
            .accepts_prompts()
            .cmp(&left.state.accepts_prompts())
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });

    let known = ordered.len();
    let slots: Vec<Slot> = ordered
        .iter()
        .take(ROSTER_CAPACITY)
        .enumerate()
        .map(|(index, session)| Slot {
            index,
            attention: attention(target, session, index as u64),
            // The same threshold decides "is this current" — it just does not
            // delete. A session nobody has touched in minutes is shown, and
            // shown as stale, which is what the widget already draws neutrally
            // rather than shouting a state it cannot vouch for.
            freshness: if now_ms.saturating_sub(session.updated_at_ms)
                <= canvas_core::working_set::EVICT_AFTER_MS
            {
                Freshness::Fresh
            } else {
                Freshness::Stale
            },
        })
        .collect();

    let roster = Roster {
        not_shown: known.saturating_sub(slots.len()),
        known,
        slots,
    };
    node_summary(id, &roster, Provenance::Live, now_ms)
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


    /// Canvas owns the surface; a producer replaces the kind body only. Sending
    /// a whole WidgetSpec was rejected on every tick with "unknown field
    /// `schema_version`" and the tile sat on "loading" forever.
    #[test]
    fn a_snapshot_carries_the_tagged_kind_and_not_a_whole_spec() {
        let payload = snapshot(
            "operator-sessions",
            "operator",
            &[session("s1", SessionState::Working, 10)],
            20,
        );
        let widget = &payload["widget"];
        assert_eq!(widget["kind"], "status_list");
        assert!(widget["title"].is_string());
        assert!(widget["nodes"].is_array());

        for forbidden in ["schema_version", "id", "chrome", "theme", "refresh_ms", "data"] {
            assert!(
                widget.get(forbidden).is_none(),
                "{forbidden} belongs to the surface, not the snapshot: {widget}"
            );
        }
        // Source health travels beside the kind, not inside it.
        assert_eq!(payload["data"]["provenance"], "live");
    }

    #[test]
    fn a_snapshot_lists_the_sessions_it_was_given() {
        let payload = snapshot(
            "operator-sessions",
            "operator",
            &[
                session("s1", SessionState::Working, 10),
                session("s2", SessionState::Blocked, 20),
            ],
            30,
        );
        let nodes = payload["widget"]["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), 2);
        assert!(
            nodes.iter().any(|node| node["label"]
                .as_str()
                .is_some_and(|label| label.contains("s2"))),
            "{nodes:?}"
        );
    }

    /// The bug this replaced: pushed through WorkingSet, sessions last touched
    /// days ago were evicted and 72 real sessions rendered as an empty tile.
    #[test]
    fn sessions_older_than_the_eviction_window_are_still_shown() {
        let ancient = session("sess_old", SessionState::Working, 1_000);
        let now = 1_000 + canvas_core::working_set::EVICT_AFTER_MS * 100;
        let payload = snapshot("operator-sessions", "operator", &[ancient], now);
        let nodes = payload["widget"]["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), 1, "an old session is stale, not absent");
        assert_eq!(
            nodes[0]["state"], "neutral",
            "stale must not shout a state it cannot vouch for"
        );
    }

    /// A wall has room for a handful, so the handful must be the useful one.
    #[test]
    fn live_sessions_outrank_recently_finished_ones() {
        let mut sessions: Vec<Session> = (0..ROSTER_CAPACITY)
            .map(|index| session(&format!("done{index}"), SessionState::Ended, 9_000 + index as u64))
            .collect();
        sessions.push(session("sess_running", SessionState::Working, 10));

        let payload = snapshot("operator-sessions", "operator", &sessions, 9_100);
        let labels: Vec<&str> = payload["widget"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["label"].as_str().unwrap())
            .collect();
        assert!(
            labels[0].contains("sess_running"),
            "the one live session must lead, despite being the oldest: {labels:?}"
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
