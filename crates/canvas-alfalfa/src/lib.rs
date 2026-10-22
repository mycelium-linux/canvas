//! `canvas-alfalfa` — Alfalfa's fleet picture on the wall.
//!
//! One producer, two stream shapes, both under the existing
//! `canvas.widget.snapshot.v1` contract; the Lua binders
//! (`alfalfa-overview-v1`, `alfalfa-pulse-v1`) own presentation, matching
//! how `feed-headlines-v1` splits fetching from rendering. Data comes from
//! alfalfad's `/api/v1/repos` — each repo object already carries its marbles
//! (Alfalfa's word, used verbatim on the wall), PRs, stages, coverage, and
//! idle metrics, so the aggregate is a fold over the same list, not a second
//! source that could disagree with it.
//!
//! Stream naming: the overview is `alfalfa/overview`; each project gets
//! `alfalfa/<id>-pulse` with the id lowercased, because Canvas owner/stream
//! identifiers are lowercase grammar while Alfalfa ids are not (`JARVIS` →
//! `jarvis-pulse`).

use serde_json::{Value, json};

pub const OWNER: &str = "alfalfa";

/// The pulse stream for a project id, safe for binding identifiers.
pub fn pulse_stream(project_id: &str) -> String {
    let lowered: String = project_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .flat_map(char::to_lowercase)
        .collect();
    format!("{lowered}-pulse")
}

/// The dashboard stream for an Alfalfa workspace id.
pub fn dashboard_stream(project_id: &str) -> String {
    let lowered: String = project_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .flat_map(char::to_lowercase)
        .collect();
    format!("{lowered}-dashboard")
}

fn idle_percent(ratio: &Value) -> String {
    ratio
        .as_f64()
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "-".into())
}

fn trim(s: &str, width: usize) -> String {
    let s: String = s.chars().take(width).collect();
    if s.chars().count() == width {
        format!("{s}…")
    } else {
        s
    }
}

/// Fold the repo roster into the overview payload. Rows keep API order
/// (newest activity first); totals are sums, idle is a count.
pub fn overview_payload(repos: &[Value], max_rows: usize) -> Value {
    let mut active = 0_u64;
    let mut ready = 0_u64;
    let mut backlog = 0_u64;
    let mut idle = 0_u64;
    let mut rows: Vec<Value> = Vec::new();
    for repo in repos {
        let num = |key: &str| repo.get(key).and_then(Value::as_u64).unwrap_or(0);
        active += num("active");
        ready += num("ready");
        backlog += num("backlog");
        if repo.get("idle").and_then(Value::as_bool) == Some(true) {
            idle += 1;
        }
        if rows.len() < max_rows {
            rows.push(json!([
                repo.get("id").and_then(Value::as_str).unwrap_or("?"),
                repo.get("state").and_then(Value::as_str).unwrap_or("?"),
                num("active").to_string(),
                num("ready").to_string(),
                num("backlog").to_string(),
                idle_percent(repo.get("idle_ratio").unwrap_or(&Value::Null)),
            ]));
        }
    }
    // A separate, tighter matrix feeds the fleet heatmap: values stay small
    // integers so both views of the same fold fit the same datagram.
    let mut matrix_rows: Vec<Value> = Vec::new();
    let mut matrix_values: Vec<Vec<u64>> = Vec::new();
    for repo in repos.iter().take(12) {
        let num = |key: &str| repo.get(key).and_then(Value::as_u64).unwrap_or(0);
        matrix_rows.push(json!(repo.get("id").and_then(Value::as_str).unwrap_or("?")));
        matrix_values.push(vec![num("active"), num("ready"), num("backlog")]);
    }
    json!({
        "projects": repos.len(),
        "active": active,
        "ready": ready,
        "backlog": backlog,
        "idle": idle,
        "rows": rows,
        "truncated": repos.len() > max_rows,
        "matrix": {
            "columns": ["act", "rdy", "bky"],
            "rows": matrix_rows,
            "values": matrix_values,
        },
    })
}

/// Normalized bead rows for the pulse payload. Kept as plain array-of-arrays
/// (not a pre-rendered EventLog) so the widget shape stays the binder's call.
/// The fifth column is the widget severity: blocked screams, in-flight glows,
/// open waits, closed rests - decided once here, where the data is actually
/// built, rather than translated twice by each possible renderer.
pub fn bead_rows(beads: Option<&Vec<Value>>, cap: usize) -> Vec<Value> {
    beads
        .unwrap_or(&Vec::new())
        .iter()
        .take(cap)
        .map(|bead| {
            json!([
                "marble",
                bead.get("id").and_then(Value::as_str).unwrap_or("?"),
                bead.get("status").and_then(Value::as_str).unwrap_or("?"),
                trim(bead.get("title").and_then(Value::as_str).unwrap_or(""), 34),
                bead_level(bead),
            ])
        })
        .collect()
}

/// The widget severity for a marble; exported so the Lua binder uses the row
/// verbatim and the projection is testable where it is defined.
pub fn bead_level(bead: &Value) -> &'static str {
    if bead.get("blocked").and_then(Value::as_bool) == Some(true)
        || bead.get("status").and_then(Value::as_str) == Some("blocked")
    {
        return "critical";
    }
    match bead.get("status").and_then(Value::as_str) {
        Some("closed") => "neutral",
        Some("in_progress") | Some("active") => "good",
        _ => "warning",
    }
}

/// One project's pulse: stage counts plus the recent marbles and PRs as the
/// API ordered them. Marbles carry no timestamps in the roster, and inventing
/// recency would be fabrication; the wall shows what Alfalfa itself shows.
pub fn pulse_payload(repo: &Value, marbles: usize, prs: usize) -> Value {
    let project = repo.get("id").and_then(Value::as_str).unwrap_or("?");
    let marble_rows = bead_rows(repo.get("beads").and_then(Value::as_array), marbles);
    let pr_rows: Vec<Value> = repo
        .get("prs")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .take(prs)
                .map(|pr| {
                    json!([
                        "pull",
                        pr.get("number")
                            .and_then(Value::as_u64)
                            .map(|n| format!("#{n}"))
                            .or_else(|| pr.get("id").and_then(Value::as_str).map(str::to_owned))
                            .unwrap_or_else(|| "?".into()),
                        pr.get("state").and_then(Value::as_str).unwrap_or("?"),
                        trim(pr.get("title").and_then(Value::as_str).unwrap_or(""), 34),
                        "good",
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    let idle_pct = repo
        .get("idle_ratio")
        .and_then(Value::as_f64)
        .map(|r| (r * 100.0).round() as u64);
    json!({
        "project": project,
        "state": repo.get("state").and_then(Value::as_str).unwrap_or("?"),
        "activity": idle_pct.map(|i| 100 - i.min(100)),
        "marble_count": repo.get("beads").and_then(Value::as_array).map_or(0, |b| b.len()),
        "done_count": repo.get("stages").and_then(Value::as_array).map(|stages| {
            stages
                .iter()
                .find(|s| s.get("stage").and_then(Value::as_str) == Some("done"))
                .and_then(|s| s.get("count").and_then(Value::as_u64))
                .unwrap_or(0)
        }),
        "capacity": repo.get("capacity").and_then(Value::as_u64).unwrap_or(0),
        "active": repo.get("active").and_then(Value::as_u64).unwrap_or(0),
        "ready": repo.get("ready").and_then(Value::as_u64).unwrap_or(0),
        "backlog": repo.get("backlog").and_then(Value::as_u64).unwrap_or(0),
        "idle": idle_percent(repo.get("idle_ratio").unwrap_or(&Value::Null)),
        "marbles": marble_rows,
        "pulls": pr_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_stream_is_project_scoped_and_identifier_safe() {
        assert_eq!(dashboard_stream("Ohm"), "ohm-dashboard");
        assert_eq!(
            dashboard_stream("Player Aerospace"),
            "playeraerospace-dashboard"
        );
    }

    fn repo(id: &str, state: &str, backlog: u64, idle: bool) -> Value {
        json!({
            "id": id,
            "state": state,
            "capacity": 2,
            "active": 1,
            "ready": 0,
            "backlog": backlog,
            "idle": idle,
            "idle_ratio": if idle { 1.0 } else { 0.25 },
            "beads": [
                {"id": format!("{id}-01"), "status": "closed", "stage": "done", "title": "Ship the thing with a very long title that should be trimmed at the codepoint boundary"},
                {"id": format!("{id}-02"), "status": "open", "stage": "active", "title": "Polish it"},
            ],
            "prs": [{"number": 42, "state": "open", "title": "Wire the widget"}],
        })
    }

    #[test]
    fn pulse_streams_lowercase_identifiers_without_losing_identity() {
        assert_eq!(pulse_stream("JARVIS"), "jarvis-pulse");
        assert_eq!(pulse_stream("agent-notebook"), "agent-notebook-pulse");
        assert_eq!(pulse_stream("Libre Chat!"), "librechat-pulse");
        let name = pulse_stream("JARVIS");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name} must fit Canvas identifier grammar"
        );
    }

    #[test]
    fn overview_folds_totals_and_keeps_bounded_rows() {
        let repos = vec![
            repo("dock", "active", 110, false),
            repo("JARVIS", "paused", 31, true),
        ];
        let payload = overview_payload(&repos, 25);
        assert_eq!(payload["projects"], 2);
        assert_eq!(payload["active"], 2);
        assert_eq!(payload["backlog"], 141);
        assert_eq!(payload["idle"], 1);
        assert_eq!(payload["rows"].as_array().unwrap().len(), 2);
        assert_eq!(payload["rows"][1][0], "JARVIS");
        assert_eq!(payload["rows"][1][5], "100%");
        assert_eq!(payload["truncated"], false);
    }

    #[test]
    fn overview_reports_truncation_honestly() {
        let repos: Vec<Value> = (0..30)
            .map(|i| repo(&format!("p{i}"), "active", 1, false))
            .collect();
        let payload = overview_payload(&repos, 8);
        assert_eq!(payload["rows"].as_array().unwrap().len(), 8);
        assert_eq!(payload["truncated"], true);
        assert_eq!(
            payload["projects"], 30,
            "totals count everything, not just shown rows"
        );
    }

    #[test]
    fn pulse_carries_marbles_and_prs_with_trimmed_titles() {
        let payload = pulse_payload(&repo("JARVIS", "active", 5, false), 6, 4);
        assert_eq!(payload["project"], "JARVIS");
        let marbles = payload["marbles"].as_array().unwrap();
        assert_eq!(marbles.len(), 2);
        assert_eq!(marbles[0][0], "marble");
        assert_eq!(marbles[0][1], "JARVIS-01");
        assert_eq!(marbles[0][4], "neutral", "closed marble rests");
        assert_eq!(marbles[1][4], "warning", "open marble waits");
        assert!(
            payload["pulls"][0][1] == "#42",
            "PR numbers render with their hash: {:?}",
            payload["pulls"][0]
        );
        let title = marbles[0][3].as_str().unwrap();
        assert!(
            title.chars().count() <= 35,
            "title must fit the cell: {title}"
        );
        assert!(title.ends_with('…'), "truncated titles say so");
    }

    #[test]
    fn overview_carries_a_tight_matrix_for_the_heatmap() {
        let repos: Vec<Value> = (0..15)
            .map(|i| repo(&format!("p{i}"), "active", i, false))
            .collect();
        let matrix = &overview_payload(&repos, 25)["matrix"];
        assert_eq!(
            matrix["rows"].as_array().unwrap().len(),
            12,
            "matrix caps tighter than rows"
        );
        assert_eq!(matrix["columns"], json!(["act", "rdy", "bky"]));
        assert_eq!(matrix["values"][0][2], 0);
    }

    #[test]
    fn bead_projection_carries_level_and_bounds() {
        let beads = json!([
            {"id": "dock-1", "status": "open", "title": "T", "blocked": true},
            {"id": "dock-2", "status": "closed", "title": "U"},
            {"id": "dock-3", "status": "in_progress", "title": "V"},
        ]);
        let rows = bead_rows(beads.as_array(), 6);
        assert_eq!(rows[0][4], "critical", "blocked wins over status");
        assert_eq!(rows[1][4], "neutral");
        assert_eq!(rows[2][4], "good");
        assert_eq!(bead_rows(None, 6), Vec::<Value>::new());
    }

    #[test]
    fn pulse_reports_numeric_activity_for_the_gauge() {
        let payload = pulse_payload(&repo("dock", "active", 5, false), 6, 4);
        assert_eq!(payload["activity"], 75, "idle_ratio 0.25 means 75% active");
        assert_eq!(payload["marble_count"], 2);
        let bare = pulse_payload(&json!({"id": "bare"}), 6, 4);
        assert_eq!(
            bare["activity"],
            Value::Null,
            "no ratio, no invented number"
        );
    }

    #[test]
    fn pulse_tolerates_missing_collections() {
        let payload = pulse_payload(&json!({"id": "bare"}), 6, 4);
        assert_eq!(payload["project"], "bare");
        assert_eq!(payload["marbles"], json!([]));
        assert_eq!(payload["backlog"], 0);
        assert_eq!(payload["idle"], "-");
    }
}
