use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

pub const OWNER: &str = "fab";
pub const PROJECTS_STREAM: &str = "operations-projects";
pub const METRICS_STREAM: &str = "operations-metrics";
pub const RUNS_STREAM: &str = "operations-runs";
pub const DELIVERIES_STREAM: &str = "operations-deliveries";
pub const TRENDS_STREAM: &str = "operations-trends";

const TREND_POINTS: usize = 48;

/// Producer-owned rolling observations. Canvas receives a complete bounded
/// snapshot, Lua describes it once, and Rust animates between snapshots.
#[derive(Debug, Default)]
pub struct TrendHistory {
    active_runs: VecDeque<(u64, u64)>,
    failed_runs: VecDeque<(u64, u64)>,
    queued_deliveries: VecDeque<(u64, u64)>,
}

impl TrendHistory {
    pub fn observe(&mut self, overview: &Value, observed_at_ms: u64) -> Value {
        let count_status = |rows: Option<&Vec<Value>>, wanted: &[&str]| {
            rows.into_iter()
                .flatten()
                .filter(|row| {
                    row.get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| wanted.contains(&status))
                })
                .count() as u64
        };
        let runs = overview.get("recentRuns").and_then(Value::as_array);
        let deliveries = overview.get("recentDeliveries").and_then(Value::as_array);
        push_sample(
            &mut self.active_runs,
            observed_at_ms,
            count_status(runs, &["queued", "running", "publishing"]),
        );
        push_sample(
            &mut self.failed_runs,
            observed_at_ms,
            count_status(runs, &["failed", "timed_out"]),
        );
        push_sample(
            &mut self.queued_deliveries,
            observed_at_ms,
            count_status(deliveries, &["queued", "processing"]),
        );
        json!({
            "period": "Rolling 4 minutes · 5s samples",
            "series": [
                trend_series("Active runs", &self.active_runs),
                trend_series("Failed runs", &self.failed_runs),
                trend_series("Queued deliveries", &self.queued_deliveries),
            ]
        })
    }
}

fn push_sample(history: &mut VecDeque<(u64, u64)>, x: u64, y: u64) {
    history.push_back((x, y));
    while history.len() > TREND_POINTS {
        history.pop_front();
    }
}

fn trend_series(label: &str, history: &VecDeque<(u64, u64)>) -> Value {
    let origin = history.front().map_or(0, |(x, _)| *x);
    json!({
        "label": label,
        // Relative seconds are both meaningful to the plot and dramatically
        // smaller than repeating 13-digit epoch milliseconds in a datagram.
        "points": history.iter().map(|(x, y)| json!([x.saturating_sub(origin) / 1_000, *y])).collect::<Vec<_>>()
    })
}

/// Keep the owner response useful while fitting Canvas's datagram transport.
/// Fab remains the source of lifecycle truth; this only bounds and flattens it.
pub fn compact_payload(overview: &Value) -> Value {
    let mut projects = overview
        .get("businesses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|business| {
            business
                .get("projects")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |project| {
                    json!({
                        "company_id": business.get("id"),
                        "company": business.get("name"),
                        "id": project.get("id"),
                        "name": project.get("name"),
                        "status": project.get("status"),
                        "active_deployments": project.get("activeDeployments"),
                        "runs_by_status": project.get("runsByStatus"),
                        "deployments_by_status": project.get("deploymentsByStatus"),
                        "artifact_files": project.get("artifactFiles"),
                        "artifact_bytes": project.get("artifactBytes"),
                        "updated_at": project.get("updatedAt"),
                    })
                })
        })
        .take(40)
        .collect::<Vec<_>>();
    if projects.is_empty() {
        projects = inferred_projects(overview);
    }
    let bounded = |key: &str, limit: usize| {
        overview
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
    };
    json!({
        "totals": overview.get("totals"),
        "projects": projects,
        "recent_runs": bounded("recentRuns", 16),
        "recent_deliveries": bounded("recentDeliveries", 12),
        "worker": overview.get("worker"),
    })
}

fn inferred_projects(overview: &Value) -> Vec<Value> {
    let mut projects = BTreeMap::<String, Value>::new();
    for run in overview
        .get("recentRuns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = run.get("project").and_then(Value::as_str) else {
            continue;
        };
        let entry = projects.entry(id.to_string()).or_insert_with(|| {
            json!({
                "company_id": null,
                "company": "Retained run window",
                "id": id,
                "name": id.split(['-', '_']).map(|part| {
                    let mut chars = part.chars();
                    chars.next().map_or_else(String::new, |first| first.to_uppercase().collect::<String>() + chars.as_str())
                }).collect::<Vec<_>>().join(" "),
                "status": run.get("status"),
                "active_deployments": 0,
                "runs_by_status": {},
                "deployments_by_status": {},
                "artifact_files": 0,
                "artifact_bytes": 0,
                "updated_at": run.get("updatedAt"),
            })
        });
        let status = run
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let count = entry["runs_by_status"][status].as_u64().unwrap_or(0) + 1;
        entry["runs_by_status"][status] = json!(count);
    }
    projects.into_values().take(40).collect()
}

pub fn stream_payloads(overview: &Value) -> [(&'static str, Value); 4] {
    let payload = compact_payload(overview);
    [
        (PROJECTS_STREAM, json!({"projects": payload["projects"]})),
        (
            METRICS_STREAM,
            json!({"totals": payload["totals"], "worker": payload["worker"]}),
        ),
        (RUNS_STREAM, json!({"recent_runs": payload["recent_runs"]})),
        (
            DELIVERIES_STREAM,
            json!({"recent_deliveries": payload["recent_deliveries"]}),
        ),
    ]
}

/// Trim a stream's sole collection until its complete envelope fits the
/// platform datagram budget. Scalar metric packets pass through unchanged.
pub fn fit_event(mut event: Value, budget: usize) -> (Value, bool) {
    let mut thinned = false;
    for key in ["projects", "recent_runs", "recent_deliveries"] {
        loop {
            let over = serde_json::to_vec(&event).map_or(true, |bytes| bytes.len() > budget);
            if !over {
                return (event, thinned);
            }
            let Some(rows) = event
                .pointer_mut(&format!("/payload/{key}"))
                .and_then(Value::as_array_mut)
            else {
                break;
            };
            if rows.is_empty() {
                break;
            }
            rows.pop();
            thinned = true;
        }
    }
    (event, thinned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_company_and_project_lifecycle() {
        let value = json!({"totals":{"projects":1},"businesses":[{"id":"fpl","name":"FPL","projects":[{
            "id":"fab","name":"Fab","status":"building","activeDeployments":2,
            "runsByStatus":{"running":1},"deploymentsByStatus":{"active":2},
            "artifactFiles":4,"artifactBytes":1024,"updatedAt":"now"
        }]}],"recentRuns":[],"recentDeliveries":[],"worker":{"concurrency":4}});
        let compact = compact_payload(&value);
        assert_eq!(compact["projects"][0]["company_id"], "fpl");
        assert_eq!(compact["projects"][0]["runs_by_status"]["running"], 1);
        assert_eq!(compact["worker"]["concurrency"], 4);
    }

    #[test]
    fn stream_packets_are_independent_and_fit_the_transport() {
        let repeated = (0..40)
            .map(|index| json!({"id": format!("run-{index}"), "project":"fab", "status":"succeeded", "message":"x".repeat(200)}))
            .collect::<Vec<_>>();
        let overview = json!({
            "totals":{"projects":1}, "businesses":[], "worker":{"concurrency":4},
            "recentRuns": repeated, "recentDeliveries":[]
        });
        for (stream, payload) in stream_payloads(&overview) {
            let event = json!({"owner":OWNER,"stream":stream,"payload":payload});
            let (event, _) = fit_event(event, 1848);
            assert!(serde_json::to_vec(&event).unwrap().len() <= 1848);
        }
    }

    #[test]
    fn older_fab_overviews_infer_projects_from_retained_runs() {
        let overview = json!({"businesses":[{"projects":[]}],"recentRuns":[
            {"project":"fab","status":"running","updatedAt":"now"},
            {"project":"fab","status":"succeeded","updatedAt":"earlier"}
        ]});
        let compact = compact_payload(&overview);
        assert_eq!(compact["projects"][0]["id"], "fab");
        assert_eq!(compact["projects"][0]["runs_by_status"]["running"], 1);
        assert_eq!(compact["projects"][0]["runs_by_status"]["succeeded"], 1);
    }

    #[test]
    fn trend_history_is_bounded_compact_and_semantic() {
        let overview = json!({
            "recentRuns": [
                {"status":"running"}, {"status":"publishing"}, {"status":"failed"}
            ],
            "recentDeliveries": [{"status":"queued"}, {"status":"succeeded"}]
        });
        let mut history = TrendHistory::default();
        for index in 0..80 {
            history.observe(&overview, 1_000_000 + index * 5_000);
        }
        let payload = history.observe(&overview, 1_400_000);
        assert_eq!(
            payload["series"][0]["points"].as_array().unwrap().len(),
            TREND_POINTS
        );
        assert_eq!(payload["series"][0]["points"][47][1], 2);
        assert_eq!(payload["series"][1]["points"][47][1], 1);
        assert_eq!(payload["series"][2]["points"][47][1], 1);
        let event = json!({"owner":OWNER,"stream":TRENDS_STREAM,"payload":payload});
        assert!(serde_json::to_vec(&event).unwrap().len() <= 1848);
    }
}
