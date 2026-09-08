//! Folding an activity stream into something a module can draw.
//!
//! The producers publish what just happened; a view needs what is *currently*
//! true. This holds the difference: a node's last firing, decayed into the `heat`
//! that [`GraphSnapshot`] carries.
//!
//! It lives host-side because a Lua module cannot keep state — each invocation
//! gets a fresh VM and only the resulting scene is retained, so heat accumulated
//! across frames has nowhere to live inside the module. The host owns what is
//! recent; the module owns what recent looks like.
//!
//! Producer-agnostic by construction. Elmers fires workflows and capabilities;
//! the MCP bridge fires calling systems, connectors and tools. Both arrive in the
//! envelope from `shared-infra/docs/activity-stream.md` and land in the same
//! graph, which is the whole reason that contract was written down.

use std::collections::BTreeMap;

use crate::lua_module::{GraphEdge, GraphNode, GraphSnapshot, StatusLevel};

/// How long a firing stays visible. Long enough that a burst reads as one event
/// rather than a flicker, short enough that a quiet system looks quiet.
pub const DEFAULT_HALF_LIFE_MS: u64 = 4_000;

/// Below this a node is drawn as idle. Without a floor, heat asymptotes and every
/// node that ever fired stays faintly lit forever.
const HEAT_FLOOR: f32 = 0.02;

#[derive(Debug, Clone)]
struct Fired {
    label: String,
    group: String,
    level: StatusLevel,
    at_ms: u64,
}

/// Accumulates firings and hands back a decayed snapshot on demand.
#[derive(Debug, Default)]
pub struct ActivityGraph {
    nodes: BTreeMap<String, Fired>,
    edges: BTreeMap<(String, String), u64>,
    /// Frames the producer says we missed, from gaps in `seq`. Surfaced rather
    /// than hidden: a view that silently drops firings is worse than one that
    /// admits it, because the whole point is trusting what you see.
    pub dropped: u64,
    last_seq: Option<u64>,
}

impl ActivityGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Light a node. Unknown ids are accepted rather than filtered: the newest
    /// thing in the system must not be the one thing the view cannot show.
    pub fn fire(
        &mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        group: impl Into<String>,
        level: StatusLevel,
        at_ms: u64,
    ) {
        self.nodes.insert(
            id.into(),
            Fired {
                label: label.into(),
                group: group.into(),
                level,
                at_ms,
            },
        );
    }

    /// Light the path a call travelled. Both ends are expected to have fired too:
    /// an edge alone reads as a line between two dark nodes.
    pub fn pulse(&mut self, from: impl Into<String>, to: impl Into<String>, at_ms: u64) {
        self.edges.insert((from.into(), to.into()), at_ms);
    }

    /// Record a producer sequence, counting anything it skipped.
    pub fn observe_seq(&mut self, seq: u64) {
        if let Some(last) = self.last_seq
            && seq > last + 1
        {
            self.dropped += seq - last - 1;
        }
        self.last_seq = Some(seq);
    }

    fn heat(at_ms: u64, now_ms: u64, half_life_ms: u64) -> f32 {
        if now_ms <= at_ms {
            return 1.0;
        }
        let elapsed = (now_ms - at_ms) as f32;
        let decayed = 0.5f32.powf(elapsed / half_life_ms.max(1) as f32);
        if decayed < HEAT_FLOOR { 0.0 } else { decayed }
    }

    /// The graph as of `now_ms`, with heat decayed.
    pub fn snapshot(&self, now_ms: u64, half_life_ms: u64) -> GraphSnapshot {
        let nodes = self
            .nodes
            .iter()
            .map(|(id, fired)| GraphNode {
                id: id.clone(),
                label: fired.label.clone(),
                group: Some(fired.group.clone()),
                heat: Self::heat(fired.at_ms, now_ms, half_life_ms),
                level: fired.level,
            })
            .collect();
        let edges = self
            .edges
            .iter()
            .filter_map(|((from, to), at)| {
                let heat = Self::heat(*at, now_ms, half_life_ms);
                (heat > 0.0).then(|| GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    heat,
                })
            })
            .collect();
        GraphSnapshot { nodes, edges }
    }

    /// Drop nodes quiet for long enough that they are no longer activity. Bounds
    /// what an always-on view accumulates over days.
    pub fn forget_before(&mut self, cutoff_ms: u64) {
        self.nodes.retain(|_, fired| fired.at_ms >= cutoff_ms);
        self.edges.retain(|_, at| *at >= cutoff_ms);
    }
}

/// The MCP bridge's `call` frame, as published by `GET /events`.
///
/// A call is three things firing at once — the system that asked, the service it
/// asked, and the tool it named — so all three light and the path between them
/// pulses. Lighting only the tool would lose who called it, which is the question
/// the stream exists to answer.
pub fn apply_bridge_call(graph: &mut ActivityGraph, data: &serde_json::Value, at_ms: u64) -> bool {
    let Some(connector) = data.get("connector").and_then(|v| v.as_str()) else {
        return false;
    };
    let client = data.get("client").and_then(|v| v.as_str()).unwrap_or("unknown");
    let level = match data.get("outcome").and_then(|v| v.as_str()) {
        Some("ok") => StatusLevel::Good,
        Some("denied") => StatusLevel::Warning,
        Some("error") => StatusLevel::Critical,
        _ => StatusLevel::Neutral,
    };

    let client_id = format!("x-{client}");
    let connector_id = format!("s-{connector}");
    graph.fire(&client_id, client, "calling system", level, at_ms);
    graph.fire(&connector_id, connector, "service", level, at_ms);
    graph.pulse(&client_id, &connector_id, at_ms);

    if let Some(tool) = data.get("tool").and_then(|v| v.as_str()) {
        let tool_id = format!("t-{tool}");
        graph.fire(&tool_id, tool, "tool", level, at_ms);
        graph.pulse(&connector_id, &tool_id, at_ms);
    }
    true
}

/// Elmers' `step` frame. The capability that ran and the workflow it ran inside
/// both light, and the call travels the edge between them — which is what turns a
/// fan-out into one body with a ring around it rather than a scatter of dots.
pub fn apply_elmers_step(graph: &mut ActivityGraph, data: &serde_json::Value, at_ms: u64) -> bool {
    let Some(step) = data.get("step").and_then(|v| v.as_str()) else {
        return false;
    };
    let level = match data.get("status").and_then(|v| v.as_str()) {
        Some("completed") => StatusLevel::Good,
        Some("failed") => StatusLevel::Critical,
        Some("started") => StatusLevel::Neutral,
        _ => StatusLevel::Neutral,
    };
    let step_id = format!("c-{step}");
    graph.fire(&step_id, step, "capability", level, at_ms);
    if let Some(workflow) = data.get("workflow").and_then(|v| v.as_str()) {
        let workflow_id = format!("w-{workflow}");
        graph.fire(&workflow_id, workflow, "workflow", level, at_ms);
        graph.pulse(&workflow_id, &step_id, at_ms);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verbatim from a running bridge: the frame shape this must consume.
    const REAL_FRAME: &str = r#"{"v":1,"type":"call","seq":1,"at":"2026-09-08 03:15:32","data":{
        "connector":"elmers","method":"tools/call","tool":"crm_read","client":"desktop-bridge",
        "auth_method":"rejected","outcome":"denied","ms":1}}"#;

    fn data_of(frame: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(frame).expect("frame")["data"].clone()
    }

    #[test]
    fn a_real_bridge_frame_lights_the_caller_the_service_and_the_tool() {
        let mut graph = ActivityGraph::new();
        assert!(apply_bridge_call(&mut graph, &data_of(REAL_FRAME), 1_000));
        let snap = graph.snapshot(1_000, DEFAULT_HALF_LIFE_MS);

        let ids: Vec<&str> = snap.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"x-desktop-bridge"), "{ids:?}");
        assert!(ids.contains(&"s-elmers"), "{ids:?}");
        assert!(ids.contains(&"t-crm_read"), "{ids:?}");
        // Lighting only the tool would lose who called it.
        assert_eq!(snap.edges.len(), 2, "the path should pulse end to end");
        assert!(snap.nodes.iter().all(|n| n.heat > 0.99), "just-fired is hot");
    }

    #[test]
    fn outcome_becomes_the_level_a_tint_can_be_drawn_from() {
        for (outcome, expected) in [
            ("ok", StatusLevel::Good),
            ("denied", StatusLevel::Warning),
            ("error", StatusLevel::Critical),
        ] {
            let mut graph = ActivityGraph::new();
            apply_bridge_call(
                &mut graph,
                &json!({"connector": "github", "client": "chat", "outcome": outcome}),
                0,
            );
            let snap = graph.snapshot(0, DEFAULT_HALF_LIFE_MS);
            assert_eq!(snap.nodes[0].level, expected, "outcome {outcome}");
        }
    }

    #[test]
    fn two_producers_land_in_one_graph_without_colliding() {
        // The point of the shared envelope: elmers and the bridge coexist.
        let mut graph = ActivityGraph::new();
        apply_bridge_call(
            &mut graph,
            &json!({"connector": "elmers", "tool": "bead", "client": "edgerunner", "outcome": "ok"}),
            0,
        );
        apply_elmers_step(
            &mut graph,
            &json!({"step": "quote", "workflow": "assemble", "status": "completed"}),
            0,
        );
        let snap = graph.snapshot(0, DEFAULT_HALF_LIFE_MS);
        let groups: std::collections::BTreeSet<&str> = snap
            .nodes
            .iter()
            .filter_map(|n| n.group.as_deref())
            .collect();
        assert_eq!(
            groups,
            ["calling system", "capability", "service", "tool", "workflow"]
                .into_iter()
                .collect()
        );
        // `elmers` appears as a bridge service and as nothing else; the elmers
        // workflow is `w-assemble`. Namespacing is what keeps them apart.
        assert!(snap.nodes.iter().any(|n| n.id == "s-elmers"));
        assert!(snap.nodes.iter().any(|n| n.id == "w-assemble"));
    }

    #[test]
    fn heat_decays_and_eventually_reads_as_idle() {
        let mut graph = ActivityGraph::new();
        apply_bridge_call(&mut graph, &data_of(REAL_FRAME), 0);
        let hot = graph.snapshot(0, 1_000).nodes[0].heat;
        let half = graph.snapshot(1_000, 1_000).nodes[0].heat;
        let cold = graph.snapshot(60_000, 1_000).nodes[0].heat;
        assert!(hot > 0.99, "{hot}");
        assert!((half - 0.5).abs() < 0.01, "one half-life should halve it: {half}");
        // Without a floor every node that ever fired stays faintly lit forever.
        assert_eq!(cold, 0.0, "{cold}");
    }

    #[test]
    fn a_gap_in_the_producer_sequence_is_counted_rather_than_hidden() {
        let mut graph = ActivityGraph::new();
        for seq in [1, 2, 7, 8] {
            graph.observe_seq(seq);
        }
        assert_eq!(graph.dropped, 4, "frames 3..6 were missed");
    }

    #[test]
    fn a_frame_without_the_field_this_needs_is_refused_not_half_applied() {
        let mut graph = ActivityGraph::new();
        assert!(!apply_bridge_call(&mut graph, &json!({"client": "chat"}), 0));
        assert!(!apply_elmers_step(&mut graph, &json!({"workflow": "assemble"}), 0));
        assert!(graph.snapshot(0, DEFAULT_HALF_LIFE_MS).nodes.is_empty());
    }

    #[test]
    fn a_quiet_node_is_eventually_forgotten() {
        let mut graph = ActivityGraph::new();
        apply_bridge_call(&mut graph, &data_of(REAL_FRAME), 1_000);
        graph.forget_before(5_000);
        assert!(graph.snapshot(5_000, DEFAULT_HALF_LIFE_MS).nodes.is_empty());
    }
}
