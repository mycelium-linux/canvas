//! A synthetic swarm, for demonstrating the agent roster before any agent is
//! wired to it.
//!
//! Everything this crate emits is marked [`Provenance::Demo`], and the widget
//! renders that marking. That is the whole reason it is allowed to exist: a
//! surface showing invented agents is fine when it says so, and is the worst
//! kind of bug when it does not.
//!
//! The generator is deterministic — it takes a tick number rather than reading a
//! clock or a random source — so a demo run is reproducible, a test can assert
//! on frame 40, and two screens fed the same tick show the same thing.

use canvas_core::working_set::{AgentState, Attention, Roster, WorkingSet};
use canvas_widgets::{Provenance, WidgetSpec, roster::node_summary};

/// Names that read like a real fleet without impersonating one. Deliberately
/// not the user's actual agent names: a demo that is indistinguishable from
/// production is the thing the provenance rule exists to prevent.
const AGENTS: &[(&str, &str)] = &[
    ("scout-01", "sweeping crates/canvas for stream consumers"),
    ("scout-02", "reading docs/widget-quality.md"),
    ("judge-01", "verifying a finding against live.rs"),
    ("survey-01", "classifying bifrost metric metadata"),
    ("survey-02", "reading config/metrics.json"),
    ("scribe-01", "drafting docs/servo-and-web-surfaces-decision.md"),
    ("spike-01", "building spikes/plotters-iced"),
    ("spike-02", "rasterising fixtures/css-probe.html"),
    ("archivist", "indexing wiki artifacts"),
    ("courier", "fetching a vendor portal page"),
    ("auditor", "diffing two Prometheus survey candidates"),
    ("planner", "waiting on approval to push"),
];

/// How long one demo tick represents. Slower than a real agent loop on purpose:
/// the point of the demo is to watch the wall settle, not to prove it can
/// strobe.
pub const TICK_MS: u64 = 1_000;

/// A deterministic swarm.
pub struct Swarm {
    set: WorkingSet,
    /// Monotonic across the whole run, so a late datagram is detectable exactly
    /// as it would be from a real producer.
    sequence: u64,
}

impl Swarm {
    /// `capacity` is how many agents the frame can hold. Bounding here rather
    /// than truncating after mapping is what keeps the "N of M" in the title
    /// true: the roster's `not_shown` is derived from this number.
    pub fn new(capacity: usize) -> Self {
        Self {
            set: WorkingSet::new(capacity),
            sequence: 0,
        }
    }

    pub fn focus_on(&mut self, project: Option<String>) {
        self.set.focus_on(project);
    }

    /// Advance to `tick` and return what the wall should show.
    ///
    /// Not every agent reports on every tick — that is the point. A swarm where
    /// everything updates in lockstep never exercises staleness, and staleness
    /// is the behaviour most worth seeing before it matters.
    pub fn advance(&mut self, tick: u64) -> Roster {
        let now_ms = tick * TICK_MS;
        for (index, (agent, doing)) in AGENTS.iter().enumerate() {
            let index = index as u64;
            // Each agent reports on its own cadence, so some are always fresher
            // than others and one of them goes quiet for a long stretch.
            let cadence = 2 + index % 5;
            if tick % cadence != 0 {
                continue;
            }
            if agent == &"scout-02" && (20..45).contains(&tick) {
                // One agent deliberately stops reporting, so a viewer can watch
                // it go stale and then be dropped rather than vanish instantly.
                continue;
            }
            self.sequence += 1;
            self.set.observe(
                Attention {
                    agent: (*agent).to_string(),
                    project: project_of(index).to_string(),
                    observing: Some(format!("demo-artifact-{index:02}")),
                    title: (*doing).to_string(),
                    state: state_of(index, tick),
                    observed_at_ms: now_ms,
                    sequence: self.sequence,
                },
                now_ms,
            );
        }
        self.set.expire(now_ms);
        self.set.roster(now_ms)
    }

    /// The widget for the current tick, marked as demonstration data.
    pub fn widget(&mut self, id: &str, tick: u64) -> WidgetSpec {
        let now_ms = tick * TICK_MS;
        let roster = self.advance(tick);
        node_summary(id, &roster, Provenance::Demo, now_ms)
    }

    pub fn resize(&mut self, capacity: usize, tick: u64) {
        self.set.resize(capacity, tick * TICK_MS);
    }
}

fn project_of(index: u64) -> &'static str {
    match index % 3 {
        0 => "dock",
        1 => "bifrost",
        _ => "studio",
    }
}

/// Mostly working, occasionally blocked, sometimes idle — and `planner` is
/// blocked far more often than the rest, so there is usually exactly one thing
/// on the wall asking for a human.
fn state_of(index: u64, tick: u64) -> AgentState {
    if index == 11 {
        return if tick % 12 < 8 {
            AgentState::Blocked
        } else {
            AgentState::Working
        };
    }
    match (tick / 3 + index) % 7 {
        0 => AgentState::Idle,
        1 => AgentState::Blocked,
        _ => AgentState::Working,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_widgets::{WidgetDataState, WidgetKind};

    fn rows(spec: &WidgetSpec) -> usize {
        match &spec.kind {
            WidgetKind::NodeSummary { nodes, .. } => nodes.len(),
            other => panic!("expected a node summary, got {other:?}"),
        }
    }

    #[test]
    fn every_frame_is_marked_as_demonstration_data() {
        let mut swarm = Swarm::new(6);
        for tick in 0..60 {
            assert_eq!(
                swarm.widget("agents", tick).data.provenance,
                Provenance::Demo,
                "an unmarked synthetic wall is indistinguishable from a real one"
            );
        }
    }

    #[test]
    fn the_same_tick_always_produces_the_same_frame() {
        let mut a = Swarm::new(6);
        let mut b = Swarm::new(6);
        for tick in 0..40 {
            assert_eq!(
                a.widget("agents", tick),
                b.widget("agents", tick),
                "a demo that differs run to run cannot be reasoned about at tick {tick}"
            );
        }
    }

    #[test]
    fn the_roster_never_exceeds_the_frame_it_was_given() {
        let mut swarm = Swarm::new(6);
        for tick in 0..80 {
            assert!(rows(&swarm.widget("agents", tick)) <= 6);
        }
    }

    #[test]
    fn the_fleet_count_always_adds_up() {
        let mut swarm = Swarm::new(5);
        for tick in 0..80 {
            let roster = swarm.advance(tick);
            assert_eq!(
                roster.known,
                roster.slots.len() + roster.not_shown,
                "the count on the wall must be arithmetic, not an estimate"
            );
        }
    }

    #[test]
    fn an_agent_that_stops_reporting_goes_stale_before_it_disappears() {
        let mut swarm = Swarm::new(12);
        // scout-02 stops at tick 20. Run past the stale threshold but well
        // inside the eviction grace period.
        let mut seen_stale = false;
        for tick in 0..40 {
            let roster = swarm.advance(tick);
            if let Some(slot) = roster
                .slots
                .iter()
                .find(|slot| slot.attention.agent == "scout-02")
                && slot.freshness == canvas_core::working_set::Freshness::Stale
            {
                seen_stale = true;
            }
        }
        assert!(
            seen_stale,
            "the demo must actually exercise staleness, or it proves nothing about it"
        );
    }

    #[test]
    fn there_is_usually_something_asking_for_a_human() {
        let mut swarm = Swarm::new(8);
        let blocked = (0..48)
            .filter(|tick| {
                swarm
                    .advance(*tick)
                    .slots
                    .iter()
                    .any(|slot| slot.attention.state == AgentState::Blocked)
            })
            .count();
        assert!(
            blocked > 12,
            "a demo with nothing blocked never shows why the wall is worth looking at: {blocked}"
        );
    }

    #[test]
    fn a_populated_frame_is_never_reported_as_empty() {
        let mut swarm = Swarm::new(6);
        for tick in 2..60 {
            let spec = swarm.widget("agents", tick);
            if rows(&spec) > 0 {
                assert_ne!(spec.data.state, WidgetDataState::NoData);
            }
        }
    }
}
