//! Which agents earn a slot on the wall, and how a slot stops flickering.
//!
//! A swarm produces a firehose. Rendering it directly gives a wall that strobes:
//! surfaces appear, jump, and vanish faster than anyone can read them, and the
//! motion is the first thing the eye follows, so the busiest agent wins
//! attention rather than the most important one.
//!
//! This module is the policy that prevents that, and it is deliberately pure —
//! no rendering, no widgets, no clock of its own. It answers one question: given
//! everything we have heard from agents and a fixed number of slots, which
//! agents are shown, in what order, and when does that change?
//!
//! Three rules do the work, and each exists because of a specific way a wall
//! becomes unreadable:
//!
//! - **Stable slots.** An agent that stays in the set keeps its position. Slots
//!   are assigned on admission and never re-sorted while occupied, so a change
//!   in one agent's state cannot move a different agent's surface under the
//!   reader's eye.
//! - **Admission resistance.** A newcomer displaces an incumbent only when it is
//!   meaningfully more salient, not merely more recent. Without a margin, two
//!   agents of equal importance trade one slot forever.
//! - **Eviction hysteresis.** An agent that goes quiet is marked stale before it
//!   is removed, and removed only after a grace period. A surface that vanishes
//!   the instant a datagram is late is worse than one that says it is stale.
//!
//! The fourth rule is honesty rather than stability: the set always reports how
//! many agents it is *not* showing. Eight of thirty agents rendered without
//! saying so is a picture that reads as the whole fleet.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How long an agent may go unheard before it is shown as stale.
pub const STALE_AFTER_MS: u64 = 15_000;

/// How long an agent stays in the set after going stale. Generous on purpose:
/// reclaiming a slot the moment a producer stutters is the flicker this module
/// exists to prevent, and an idle agent costs nothing but a slot.
pub const EVICT_AFTER_MS: u64 = 120_000;

/// How much more salient a newcomer must be to take an occupied slot. Expressed
/// in salience points so it is comparable to the ranks below rather than being a
/// tunable with no meaning.
pub const ADMISSION_MARGIN: u32 = 10;

/// What an agent is doing, coarsest-grained and independent of what it is
/// looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Waiting on a human. Ranked highest because it is the only state where
    /// looking at the wall changes the outcome.
    Blocked,
    /// Doing something.
    Working,
    /// Alive and not doing anything in particular.
    Idle,
}

impl AgentState {
    /// Base salience. Blocked outranks working outranks idle, by more than
    /// [`ADMISSION_MARGIN`], so a blocked agent always takes a slot from a
    /// working one rather than waiting its turn.
    fn rank(self) -> u32 {
        match self {
            AgentState::Blocked => 200,
            AgentState::Working => 100,
            AgentState::Idle => 0,
        }
    }
}

/// One report from one agent. The routing key is `agent`: a later report about
/// the same agent updates its slot rather than claiming another, which is what
/// keeps ten agents to ten surfaces instead of a thousand events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    pub agent: String,
    pub project: String,
    /// Content hash of what the agent is looking at, when it is looking at
    /// something. `None` is a real answer — thinking is not reading.
    pub observing: Option<String>,
    /// Human-facing label for whatever `observing` names.
    pub title: String,
    pub state: AgentState,
    /// When the agent reported this, by the agent's clock.
    pub observed_at_ms: u64,
    /// Position in the stream log this report arrived at, so a slot can be
    /// traced back to what produced it.
    pub sequence: u64,
}

/// Freshness of a slot, decided by elapsed time rather than stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    /// Heard from too long ago to trust as current. Still shown, and shown as
    /// stale — the distinction between "this agent is idle" and "we stopped
    /// hearing from this agent" is exactly the one a viewer needs.
    Stale,
}

/// An agent currently holding a slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    /// Assigned on admission and stable for as long as the agent is held, so a
    /// renderer can key a surface by it and never move an unrelated surface.
    pub index: usize,
    pub attention: Attention,
    pub freshness: Freshness,
}

/// Who is on screen right now, and in what order.
///
/// Named for the occupants rather than the surface: "wall" already means the
/// physical display everywhere else in this tree, and a type that describes what
/// to put on one should not borrow its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    /// Held agents, ordered by slot index — not by salience, because re-sorting
    /// on every update is the flicker.
    pub slots: Vec<Slot>,
    /// Agents known but not shown. Rendering `slots.len()` as though it were the
    /// fleet is the same defect as a widget rendering fabricated data.
    pub not_shown: usize,
    /// Every agent heard from, shown or not.
    pub known: usize,
}

/// A bounded, stable working set of agents.
#[derive(Debug, Clone)]
pub struct WorkingSet {
    capacity: usize,
    /// Everything heard from, including agents with no slot. Bounded by
    /// eviction, not by capacity — knowing that thirty agents exist is what
    /// makes `not_shown` truthful.
    known: BTreeMap<String, Attention>,
    /// agent -> slot index, for the agents currently holding one.
    held: BTreeMap<String, usize>,
    /// The project the human is standing in front of, if known. Outranks
    /// everything else, because a wall in a room is about that room.
    focus: Option<String>,
}

impl WorkingSet {
    /// `capacity` is how many surfaces the frame can carry legibly. It comes
    /// from the layout, not from this module — how many things fit is a question
    /// about pixels and minimum sizes, not about salience.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            known: BTreeMap::new(),
            held: BTreeMap::new(),
            focus: None,
        }
    }

    /// Name the project the human is present in. Agents in it outrank all others
    /// regardless of state.
    pub fn focus_on(&mut self, project: Option<String>) {
        self.focus = project;
    }

    /// Capacity can change when the frame does — a display is added, a view
    /// reflows. Shrinking gives up the least salient slots and compacts the rest
    /// upward, keeping relative order so the surfaces that survive stay in the
    /// sequence the reader learned.
    pub fn resize(&mut self, capacity: usize, now_ms: u64) {
        self.capacity = capacity;
        while self.held.len() > capacity {
            let Some(victim) = self.least_salient_held(now_ms) else {
                break;
            };
            self.held.remove(&victim);
        }
        self.compact();
    }

    /// Record what an agent says it is doing.
    ///
    /// Out-of-order reports are ignored rather than applied: a lower sequence
    /// than the one already held is an old datagram arriving late, and letting
    /// it win would make a surface flick backwards to a state that is no longer
    /// true.
    pub fn observe(&mut self, attention: Attention, now_ms: u64) {
        if let Some(existing) = self.known.get(&attention.agent)
            && attention.sequence < existing.sequence
        {
            return;
        }
        let agent = attention.agent.clone();
        self.known.insert(agent.clone(), attention);
        self.expire(now_ms);
        if self.held.contains_key(&agent) {
            return;
        }
        if self.held.len() < self.capacity {
            self.admit(agent);
            return;
        }
        self.maybe_displace(&agent, now_ms);
    }

    /// Drop agents nothing has been heard from for [`EVICT_AFTER_MS`].
    ///
    /// Called on every observation, and worth calling on a timer too: a wall
    /// whose agents all went away should empty, not freeze on the last thing it
    /// happened to see.
    pub fn expire(&mut self, now_ms: u64) {
        let gone: Vec<String> = self
            .known
            .iter()
            .filter(|(_, attention)| {
                now_ms.saturating_sub(attention.observed_at_ms) >= EVICT_AFTER_MS
            })
            .map(|(agent, _)| agent.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        for agent in gone {
            self.known.remove(&agent);
            self.held.remove(&agent);
        }
        self.compact();
        self.backfill();
    }

    /// What to render.
    pub fn roster(&self, now_ms: u64) -> Roster {
        let mut slots: Vec<Slot> = self
            .held
            .iter()
            .filter_map(|(agent, index)| {
                let attention = self.known.get(agent)?;
                Some(Slot {
                    index: *index,
                    freshness: if now_ms.saturating_sub(attention.observed_at_ms) >= STALE_AFTER_MS {
                        Freshness::Stale
                    } else {
                        Freshness::Fresh
                    },
                    attention: attention.clone(),
                })
            })
            .collect();
        slots.sort_by_key(|slot| slot.index);
        Roster {
            not_shown: self.known.len().saturating_sub(slots.len()),
            known: self.known.len(),
            slots,
        }
    }

    /// Salience of one agent: state, plus a large bonus for being in the project
    /// the human is present in. Deliberately not time-weighted — ranking by
    /// recency is what makes the busiest agent win, and the busiest agent is
    /// rarely the one worth watching.
    fn salience(&self, agent: &str) -> u32 {
        let Some(attention) = self.known.get(agent) else {
            return 0;
        };
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|project| *project == attention.project);
        attention.state.rank() + if focused { 1_000 } else { 0 }
    }

    fn admit(&mut self, agent: String) {
        let index = (0..self.capacity)
            .find(|candidate| !self.held.values().any(|held| held == candidate))
            .unwrap_or(self.held.len());
        self.held.insert(agent, index);
    }

    /// Take a slot from the least salient incumbent, but only by a margin.
    /// Without the margin, two equally salient agents swap the same slot on
    /// every update forever, which is the worst-looking failure of the lot.
    fn maybe_displace(&mut self, agent: &str, now_ms: u64) {
        let Some(victim) = self.least_salient_held(now_ms) else {
            return;
        };
        if self.salience(agent) < self.salience(&victim).saturating_add(ADMISSION_MARGIN) {
            return;
        }
        let Some(index) = self.held.remove(&victim) else {
            return;
        };
        self.held.insert(agent.to_string(), index);
    }

    /// Stale agents are given up before fresh ones at equal salience, so a
    /// producer that died does not hold a slot against a live agent.
    fn least_salient_held(&self, now_ms: u64) -> Option<String> {
        self.held
            .keys()
            .min_by_key(|agent| {
                let stale = self.known.get(*agent).is_some_and(|attention| {
                    now_ms.saturating_sub(attention.observed_at_ms) >= STALE_AFTER_MS
                });
                (self.salience(agent), !stale, (*agent).clone())
            })
            .cloned()
    }

    /// Close gaps left by eviction while preserving relative order.
    fn compact(&mut self) {
        let mut ordered: Vec<(String, usize)> =
            self.held.iter().map(|(a, i)| (a.clone(), *i)).collect();
        ordered.sort_by_key(|(_, index)| *index);
        for (position, (agent, _)) in ordered.into_iter().enumerate() {
            self.held.insert(agent, position);
        }
    }

    /// Fill slots freed by eviction with the most salient agents that have none.
    fn backfill(&mut self) {
        while self.held.len() < self.capacity {
            let Some(next) = self
                .known
                .keys()
                .filter(|agent| !self.held.contains_key(*agent))
                .max_by_key(|agent| (self.salience(agent), (*agent).clone()))
                .cloned()
            else {
                return;
            };
            self.admit(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(agent: &str, state: AgentState, at: u64, sequence: u64) -> Attention {
        Attention {
            agent: agent.into(),
            project: "dock".into(),
            observing: Some(format!("{agent}-artifact")),
            title: format!("{agent} is reading"),
            state,
            observed_at_ms: at,
            sequence,
        }
    }

    fn in_project(agent: &str, project: &str, at: u64, sequence: u64) -> Attention {
        Attention {
            project: project.into(),
            ..report(agent, AgentState::Working, at, sequence)
        }
    }

    #[test]
    fn a_second_report_updates_a_slot_rather_than_claiming_another() {
        let mut set = WorkingSet::new(4);
        set.observe(report("a", AgentState::Working, 0, 1), 0);
        set.observe(report("a", AgentState::Blocked, 10, 2), 10);
        let wall = set.roster(10);
        assert_eq!(wall.slots.len(), 1, "one agent is one surface");
        assert_eq!(wall.slots[0].attention.state, AgentState::Blocked);
        assert_eq!(wall.known, 1);
    }

    #[test]
    fn an_agents_slot_does_not_move_when_another_agent_changes() {
        let mut set = WorkingSet::new(3);
        set.observe(report("a", AgentState::Idle, 0, 1), 0);
        set.observe(report("b", AgentState::Idle, 0, 1), 0);
        set.observe(report("c", AgentState::Idle, 0, 1), 0);
        let before: Vec<usize> = set.roster(0).slots.iter().map(|slot| slot.index).collect();
        let a_index = set.roster(0).slots[0].index;

        // b becomes the most important thing on the wall.
        set.observe(report("b", AgentState::Blocked, 10, 2), 10);

        let after = set.roster(10);
        assert_eq!(
            after.slots.iter().map(|s| s.index).collect::<Vec<_>>(),
            before,
            "salience must not re-sort occupied slots"
        );
        assert_eq!(
            after.slots[0].index, a_index,
            "a's surface must not move because b changed"
        );
    }

    #[test]
    fn a_late_datagram_cannot_drag_a_slot_backwards() {
        let mut set = WorkingSet::new(2);
        set.observe(report("a", AgentState::Blocked, 100, 9), 100);
        // An older report arriving after the newer one.
        set.observe(report("a", AgentState::Working, 50, 4), 100);
        assert_eq!(
            set.roster(100).slots[0].attention.state,
            AgentState::Blocked,
            "an out-of-order report must be ignored, not applied"
        );
    }

    #[test]
    fn an_equally_salient_newcomer_does_not_take_an_occupied_slot() {
        let mut set = WorkingSet::new(1);
        set.observe(report("a", AgentState::Working, 0, 1), 0);
        set.observe(report("b", AgentState::Working, 1, 1), 1);
        let wall = set.roster(1);
        assert_eq!(wall.slots.len(), 1);
        assert_eq!(
            wall.slots[0].attention.agent, "a",
            "without a margin two equal agents trade one slot forever"
        );
        assert_eq!(wall.not_shown, 1);
    }

    #[test]
    fn a_blocked_agent_displaces_a_working_one() {
        let mut set = WorkingSet::new(1);
        set.observe(report("a", AgentState::Working, 0, 1), 0);
        set.observe(report("b", AgentState::Blocked, 1, 1), 1);
        assert_eq!(
            set.roster(1).slots[0].attention.agent,
            "b",
            "blocked is the one state where looking at the wall changes the outcome"
        );
    }

    #[test]
    fn the_project_the_human_is_in_outranks_everything() {
        let mut set = WorkingSet::new(1);
        set.observe(in_project("a", "dock", 0, 1), 0);
        set.focus_on(Some("studio".into()));
        set.observe(in_project("b", "studio", 1, 1), 1);
        assert_eq!(set.roster(1).slots[0].attention.agent, "b");
    }

    #[test]
    fn a_quiet_agent_is_shown_stale_before_it_is_dropped() {
        let mut set = WorkingSet::new(2);
        set.observe(report("a", AgentState::Working, 0, 1), 0);

        let fresh = set.roster(STALE_AFTER_MS - 1);
        assert_eq!(fresh.slots[0].freshness, Freshness::Fresh);

        let stale = set.roster(STALE_AFTER_MS);
        assert_eq!(
            stale.slots[0].freshness,
            Freshness::Stale,
            "a surface that vanishes when a datagram is late is worse than one that says it is stale"
        );
        assert_eq!(stale.slots.len(), 1, "stale is still shown");
    }

    #[test]
    fn an_agent_is_dropped_only_after_the_grace_period() {
        let mut set = WorkingSet::new(2);
        set.observe(report("a", AgentState::Working, 0, 1), 0);
        set.expire(EVICT_AFTER_MS - 1);
        assert_eq!(set.roster(EVICT_AFTER_MS - 1).slots.len(), 1);
        set.expire(EVICT_AFTER_MS);
        assert!(set.roster(EVICT_AFTER_MS).slots.is_empty());
    }

    #[test]
    fn the_wall_says_how_many_agents_it_is_not_showing() {
        let mut set = WorkingSet::new(3);
        for n in 0..30 {
            set.observe(report(&format!("agent-{n:02}"), AgentState::Working, 0, 1), 0);
        }
        let wall = set.roster(0);
        assert_eq!(wall.slots.len(), 3);
        assert_eq!(wall.known, 30);
        assert_eq!(
            wall.not_shown, 27,
            "rendering 3 of 30 without saying so reads as the whole fleet"
        );
    }

    #[test]
    fn a_freed_slot_is_refilled_by_the_most_salient_waiting_agent() {
        let mut set = WorkingSet::new(1);
        set.observe(report("a", AgentState::Working, 0, 1), 0);
        set.observe(report("waiting", AgentState::Idle, 0, 1), 0);
        // `a` goes away entirely; the slot must not sit empty.
        set.observe(report("waiting", AgentState::Idle, EVICT_AFTER_MS, 2), EVICT_AFTER_MS);
        let wall = set.roster(EVICT_AFTER_MS);
        assert_eq!(wall.slots.len(), 1);
        assert_eq!(wall.slots[0].attention.agent, "waiting");
        assert_eq!(wall.not_shown, 0);
    }

    #[test]
    fn slot_indices_stay_dense_and_unique_through_churn() {
        let mut set = WorkingSet::new(4);
        for round in 0..40u64 {
            let agent = format!("agent-{}", round % 9);
            let state = if round % 5 == 0 {
                AgentState::Blocked
            } else {
                AgentState::Working
            };
            let now = round * 1_000;
            set.observe(report(&agent, state, now, round + 1), now);
            let wall = set.roster(now);
            let mut indices: Vec<usize> = wall.slots.iter().map(|slot| slot.index).collect();
            let count = indices.len();
            indices.sort_unstable();
            indices.dedup();
            assert_eq!(indices.len(), count, "two surfaces claimed one slot");
            assert!(
                indices.iter().all(|index| *index < 4),
                "a slot outside the frame is a surface nobody can see"
            );
            assert_eq!(
                wall.known,
                wall.slots.len() + wall.not_shown,
                "the fleet count must always add up"
            );
        }
    }

    #[test]
    fn shrinking_the_frame_keeps_the_order_the_reader_learned() {
        let mut set = WorkingSet::new(4);
        for name in ["a", "b", "c", "d"] {
            set.observe(report(name, AgentState::Working, 0, 1), 0);
        }
        let before: Vec<String> = set
            .roster(0)
            .slots
            .iter()
            .map(|slot| slot.attention.agent.clone())
            .collect();
        set.resize(2, 0);
        let after: Vec<String> = set
            .roster(0)
            .slots
            .iter()
            .map(|slot| slot.attention.agent.clone())
            .collect();
        assert_eq!(after.len(), 2);
        let kept: Vec<&String> = before.iter().filter(|name| after.contains(name)).collect();
        assert_eq!(
            kept.into_iter().cloned().collect::<Vec<_>>(),
            after,
            "survivors must keep their relative order"
        );
        assert_eq!(set.roster(0).not_shown, 2);
    }
}
