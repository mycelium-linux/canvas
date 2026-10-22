//! Rendering a [`WorkingSet`](canvas_core::working_set::WorkingSet) roster as an
//! ordinary status list.
//!
//! There is deliberately no `AgentRoster` widget kind. A roster is a bounded list
//! of entities each carrying a status and a line of detail, and
//! [`WidgetKind::StatusList`] already is that shape — its own documentation says
//! "nodes, agents, or DAG stages". Adding a kind here would couple a widget to
//! one data structure, which is exactly what the widget library is meant to
//! avoid; instead the policy stays pure in `canvas-core` and the renderer stays
//! generic.
//!
//! What this module owns is the honesty of the translation, and there are two
//! places it could quietly lie:
//!
//! - **The fleet count.** The roster knows about agents it could not give a slot.
//!   The title carries that, because a list of three rendered without comment
//!   reads as the whole fleet.
//! - **Stale agents.** A stale agent's last-known state is not its current state.
//!   Painting a stale `Blocked` agent in warning colour tells a viewer to act on
//!   information we no longer have. Stale entries go neutral and say when they
//!   were last seen, so the colour never shouts about something unverified.

use crate::{
    NodeSummaryItem, Provenance, StatusLevel, WIDGET_SCHEMA_VERSION, WidgetChrome, WidgetData,
    WidgetDataState, WidgetKind, WidgetSpec,
};
use canvas_core::working_set::{AgentState, Freshness, Roster, Slot};

/// Build the widget for a roster.
///
/// `now_ms` is passed in rather than read, so the same roster renders the same
/// way in a test as on a wall.
pub fn node_summary(id: &str, roster: &Roster, provenance: Provenance, now_ms: u64) -> WidgetSpec {
    WidgetSpec {
        schema_version: WIDGET_SCHEMA_VERSION,
        id: id.to_string(),
        kind: WidgetKind::StatusList {
            title: title(roster),
            nodes: roster.slots.iter().map(|slot| item(slot, now_ms)).collect(),
        },
        theme: None,
        refresh_ms: None,
        chrome: WidgetChrome::default(),
        data: WidgetData {
            state: state(roster),
            source: Some("agent attention".into()),
            observed_at: None,
            window: None,
            detail: detail(roster),
            provenance,
        },
    }
}

/// Says what is on screen and what is not.
///
/// "Agents" alone would be a claim about the fleet. "3 of 30" is a claim about
/// the list, which is the only one this widget can support.
fn title(roster: &Roster) -> String {
    if roster.not_shown == 0 {
        return match roster.known {
            0 => "Agents".to_string(),
            known => format!("Agents ({known})"),
        };
    }
    format!("Agents ({} of {})", roster.slots.len(), roster.known)
}

/// The whole-widget state, which is about the roster rather than any one agent.
fn state(roster: &Roster) -> WidgetDataState {
    if roster.slots.is_empty() {
        return WidgetDataState::NoData;
    }
    if roster
        .slots
        .iter()
        .all(|slot| slot.freshness == Freshness::Stale)
    {
        // Every agent has gone quiet. That is not "no agents" and it is not
        // business as usual: it usually means the producer died, not that thirty
        // agents stopped at once.
        return WidgetDataState::Stale;
    }
    if roster
        .slots
        .iter()
        .any(|slot| slot.freshness == Freshness::Stale)
    {
        return WidgetDataState::Partial;
    }
    WidgetDataState::Ready
}

/// The empty case has to say why, not render an empty box.
fn detail(roster: &Roster) -> Option<String> {
    (roster.known == 0).then(|| "No agent has reported attention yet".to_string())
}

fn item(slot: &Slot, now_ms: u64) -> NodeSummaryItem {
    let attention = &slot.attention;
    match slot.freshness {
        Freshness::Fresh => NodeSummaryItem {
            label: attention.agent.clone(),
            state: level(attention.state),
            detail: Some(attention.title.clone()),
        },
        // Neutral regardless of the last-known state: the colour is the loud
        // signal on this widget, and shouting on unverified information is the
        // failure this whole surface exists to avoid.
        Freshness::Stale => NodeSummaryItem {
            label: attention.agent.clone(),
            state: StatusLevel::Neutral,
            detail: Some(format!(
                "was {} · last seen {}",
                described(attention.state),
                ago(now_ms.saturating_sub(attention.observed_at_ms))
            )),
        },
    }
}

fn level(state: AgentState) -> StatusLevel {
    match state {
        // Warning rather than Critical: an agent waiting on a human is the one
        // case where looking at the wall changes the outcome, but nothing is
        // broken. Critical is reserved for things that are.
        AgentState::Blocked => StatusLevel::Warning,
        AgentState::Working => StatusLevel::Good,
        AgentState::Idle => StatusLevel::Neutral,
    }
}

fn described(state: AgentState) -> &'static str {
    match state {
        AgentState::Blocked => "blocked",
        AgentState::Working => "working",
        AgentState::Idle => "idle",
    }
}

/// Coarse on purpose. A wall is read from across a room, and "4m" carries every
/// bit of the decision that "4m12s" does.
fn ago(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    if seconds < 90 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 90 {
        return format!("{minutes}m ago");
    }
    format!("{}h ago", minutes / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::working_set::{Attention, STALE_AFTER_MS, WorkingSet};

    fn report(agent: &str, state: AgentState, at: u64) -> Attention {
        Attention {
            agent: agent.into(),
            project: "dock".into(),
            observing: Some(format!("{agent}-artifact")),
            title: format!("reading {agent}.pdf"),
            state,
            observed_at_ms: at,
            sequence: at + 1,
        }
    }

    fn nodes(spec: &WidgetSpec) -> &[NodeSummaryItem] {
        match &spec.kind {
            WidgetKind::StatusList { nodes, .. } => nodes,
            other => panic!("expected a node summary, got {other:?}"),
        }
    }

    fn heading(spec: &WidgetSpec) -> &str {
        match &spec.kind {
            WidgetKind::StatusList { title, .. } => title,
            other => panic!("expected a node summary, got {other:?}"),
        }
    }

    #[test]
    fn a_roster_becomes_one_row_per_agent() {
        let mut set = WorkingSet::new(4);
        set.observe(report("scout", AgentState::Working, 0), 0);
        set.observe(report("judge", AgentState::Blocked, 0), 0);
        let spec = node_summary("agents", &set.roster(0), Provenance::Live, 0);
        let rows = nodes(&spec);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.label == "scout"));
        assert_eq!(spec.data.state, WidgetDataState::Ready);
    }

    #[test]
    fn blocked_is_the_only_state_that_colours_for_attention() {
        let mut set = WorkingSet::new(4);
        set.observe(report("a", AgentState::Blocked, 0), 0);
        set.observe(report("b", AgentState::Working, 0), 0);
        set.observe(report("c", AgentState::Idle, 0), 0);
        let spec = node_summary("agents", &set.roster(0), Provenance::Live, 0);
        let by = |name: &str| {
            nodes(&spec)
                .iter()
                .find(|row| row.label == name)
                .expect("row")
                .state
        };
        assert_eq!(by("a"), StatusLevel::Warning);
        assert_eq!(by("b"), StatusLevel::Good);
        assert_eq!(by("c"), StatusLevel::Neutral);
        assert!(
            !nodes(&spec)
                .iter()
                .any(|row| row.state == StatusLevel::Critical),
            "nothing here is broken; critical is reserved for things that are"
        );
    }

    #[test]
    fn the_title_says_how_many_agents_are_not_on_screen() {
        let mut set = WorkingSet::new(3);
        for n in 0..30 {
            set.observe(report(&format!("agent-{n:02}"), AgentState::Working, 0), 0);
        }
        let spec = node_summary("agents", &set.roster(0), Provenance::Live, 0);
        assert_eq!(heading(&spec), "Agents (3 of 30)");
    }

    #[test]
    fn a_full_roster_does_not_pretend_to_be_partial() {
        let mut set = WorkingSet::new(8);
        set.observe(report("only", AgentState::Working, 0), 0);
        assert_eq!(
            heading(&node_summary("agents", &set.roster(0), Provenance::Live, 0)),
            "Agents (1)"
        );
    }

    #[test]
    fn a_stale_agent_goes_neutral_and_says_when_it_was_last_seen() {
        let mut set = WorkingSet::new(4);
        set.observe(report("quiet", AgentState::Blocked, 0), 0);
        let now = STALE_AFTER_MS + 45_000;
        let spec = node_summary("agents", &set.roster(now), Provenance::Live, now);
        let row = &nodes(&spec)[0];
        assert_eq!(
            row.state,
            StatusLevel::Neutral,
            "a stale blocked agent must not keep shouting for attention"
        );
        let detail = row.detail.as_deref().expect("detail");
        assert!(detail.starts_with("was blocked"), "{detail}");
        assert!(detail.contains("ago"), "{detail}");
        assert_eq!(spec.data.state, WidgetDataState::Stale);
    }

    #[test]
    fn one_stale_agent_among_fresh_ones_makes_the_widget_partial() {
        let mut set = WorkingSet::new(4);
        set.observe(report("quiet", AgentState::Working, 0), 0);
        let now = STALE_AFTER_MS + 1;
        set.observe(report("busy", AgentState::Working, now), now);
        let spec = node_summary("agents", &set.roster(now), Provenance::Live, now);
        assert_eq!(spec.data.state, WidgetDataState::Partial);
    }

    #[test]
    fn an_empty_roster_says_why_rather_than_rendering_an_empty_box() {
        let set = WorkingSet::new(4);
        let spec = node_summary("agents", &set.roster(0), Provenance::Live, 0);
        assert_eq!(spec.data.state, WidgetDataState::NoData);
        assert_eq!(
            spec.data.detail.as_deref(),
            Some("No agent has reported attention yet")
        );
        assert_eq!(heading(&spec), "Agents");
    }

    #[test]
    fn demo_provenance_survives_into_the_spec() {
        let mut set = WorkingSet::new(2);
        set.observe(report("a", AgentState::Working, 0), 0);
        let spec = node_summary("agents", &set.roster(0), Provenance::Demo, 0);
        assert_eq!(
            spec.data.provenance,
            Provenance::Demo,
            "a demo wall must be able to say so"
        );
    }

    #[test]
    fn the_spec_the_mapping_produces_is_one_the_library_accepts() {
        let mut set = WorkingSet::new(4);
        set.observe(report("a", AgentState::Blocked, 0), 0);
        let spec = node_summary("agents", &set.roster(0), Provenance::Live, 0);
        spec.validate()
            .expect("the mapping must not emit a spec the renderer would reject");
    }

    #[test]
    fn elapsed_time_is_coarse_enough_to_read_across_a_room() {
        assert_eq!(ago(0), "0s ago");
        assert_eq!(ago(45_000), "45s ago");
        assert_eq!(ago(120_000), "2m ago");
        assert_eq!(ago(3 * 3_600_000), "3h ago");
    }
}
