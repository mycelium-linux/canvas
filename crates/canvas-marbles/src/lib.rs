//! Marbles public CLI -> Canvas shared TaskDeck presentation protocol.
//! Wire contract: canvas-workspace/endpoint.rs and gfx/panels.rs; no owner writes.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const SURFACES: usize = 4;
pub const PER_SURFACE: usize = 4;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub root: PathBuf,
    pub marbles: PathBuf,
    pub trackers: Vec<String>,
    pub cohort: String,
    pub workspace: String,
    #[serde(default)]
    pub project_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub viewer_principal: Option<String>,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !self.root.is_absolute() || !self.root.is_dir() || !self.marbles.is_absolute() {
            return Err("Owner root and CLI must be absolute installed paths".into());
        }
        let identity = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                && !s.starts_with('-')
        };
        if self.trackers.is_empty()
            || self.trackers.len() > 16
            || !identity(&self.cohort)
            || !identity(&self.workspace)
            || self.trackers.iter().any(|s| !identity(s))
            || self.trackers.iter().collect::<BTreeSet<_>>().len() != self.trackers.len()
        {
            return Err(
                "Configure unique explicit trackers, cohort and workspace identities".into(),
            );
        }
        if self.workspace.len() > 61
            || !self.workspace.starts_with(|c: char| c.is_ascii_lowercase())
            || !self
                .workspace
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err("Workspace must be a Canvas identifier of at most 61 bytes".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Issue {
    pub id: String,
    pub project: String,
    pub title: String,
    pub status: String,
    pub priority: u8,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub actor_kind: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub dependency_count: usize,
    #[serde(default)]
    pub evidence: Vec<Value>,
    #[serde(default)]
    pub metadata: Value,
}

pub fn parse_owner(bytes: &[u8], tracker: &str, cohort: &str) -> Result<Vec<Issue>, String> {
    let rows: Vec<Issue> = serde_json::from_slice(bytes).map_err(|_| "Invalid owner response")?;
    // Validate the entire response before scope filtering; crossed scope is an error.
    if rows.iter().any(|i| {
        i.project != tracker
            || i.id.is_empty()
            || i.id.len() > 128
            || i.id.chars().any(char::is_control)
    }) {
        return Err("Owner response crossed tracker scope or has an invalid identity".into());
    }
    let mut seen = BTreeSet::new();
    if rows.iter().any(|i| !seen.insert(i.id.clone())) {
        return Err("Duplicate owner identity".into());
    }
    Ok(rows
        .into_iter()
        .filter(|i| {
            i.id == cohort
                || i.parent.as_deref() == Some(cohort)
                || i.metadata.get("coordination_issue").and_then(Value::as_str) == Some(cohort)
        })
        .collect())
}

pub fn fetch(config: &Config) -> Result<Vec<Issue>, String> {
    let mut all = Vec::new();
    for tracker in &config.trackers {
        let bytes = owner_command(config, tracker).map_err(|e| format!("{tracker}: {e}"))?;
        all.extend(parse_owner(&bytes, tracker, &config.cohort)?);
    }
    // Stable identity ordering keeps presentation membership stable across status changes.
    all.sort_by(|a, b| (&a.project, &a.id).cmp(&(&b.project, &b.id)));
    Ok(all)
}

fn owner_command(config: &Config, tracker: &str) -> Result<Vec<u8>, String> {
    let mut child = Command::new(&config.marbles)
        .args(["list", "--project", tracker, "--all", "--json"])
        .current_dir(&config.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot start Marbles owner CLI")?;
    let stdout = child.stdout.take().ok_or("Missing owner output")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let budget = Duration::from_secs(10);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < budget => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Marbles owner unavailable or timed out".into());
            }
        }
    };
    let bytes = receiver
        .recv_timeout(budget.saturating_sub(start.elapsed()))
        .map_err(|_| "Owner output timed out")?
        .map_err(|_| "Owner output failed")?;
    if !status.success() || bytes.len() > 4 * 1024 * 1024 {
        return Err("Marbles read failed or exceeded response limit".into());
    }
    Ok(bytes)
}

fn bounded(value: &str, limit: usize) -> String {
    let clean: String = value
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();
    if clean.len() <= limit {
        return clean;
    }
    let mut end = limit.saturating_sub('…'.len_utf8());
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &clean[..end])
}

fn task(issue: &Issue) -> Value {
    // An open issue is not evidence that somebody requested input or approval.
    let state = match issue.status.as_str() {
        "in_progress" => "active",
        "open" | "closed" | "deferred" | "superseded" => "idle",
        _ => "unknown",
    };
    let source = issue
        .metadata
        .get("design")
        .and_then(Value::as_str)
        .unwrap_or("No source reference");
    let detail = format!(
        "{}\nStatus: {} · priority {}\nClaim kind: {}\nHolder: {}\nDependencies: {} · evidence records: {}\nSource: {}\nRead-only Marbles projection; details are excerpts.",
        issue.id,
        issue.status,
        issue.priority,
        issue.actor_kind.as_deref().unwrap_or("unclaimed"),
        issue.assignee.as_deref().unwrap_or("none"),
        issue.dependency_count,
        issue.evidence.len(),
        source
    );
    json!({"id":format!("{}:{}",issue.project,issue.id),"project":bounded(&issue.project,160),"title":bounded(&issue.title,240),"meta":bounded(&format!("{} · P{} · {}",issue.status,issue.priority,issue.id),240),"detail":bounded(&detail,1600),"state":state})
}

pub fn snapshots(result: Result<&[Issue], &str>) -> Vec<Value> {
    let (issues, fresh, detail) = match result {
        Ok(issues) if issues.len() <= SURFACES * PER_SURFACE => (
            issues,
            true,
            format!("{} scoped owner records · read-only", issues.len()),
        ),
        Ok(_) => (
            &[][..],
            false,
            "Cohort exceeds 16 records; narrow scope before display".into(),
        ),
        Err(_) => (
            &[][..],
            false,
            "Marbles unavailable; owner connection requires attention".into(),
        ),
    };
    (0..SURFACES).map(|index| {
        let items: Vec<_> = issues.iter().skip(index*PER_SURFACE).take(PER_SURFACE).map(task).collect();
        json!({"surface_id":format!("work-deck-{index}"),"frame":{"view":"task_deck","panel":{"items":items,"fresh":fresh}},"data":{"state":if fresh {"ready"} else {"stale"},"provenance":"live","source":"marbles:cohort","detail":detail}})
    }).collect()
}

pub fn document(workspace: &str) -> Value {
    let empty = snapshots(Err("Not yet observed"));
    let mut surfaces: Vec<_> = empty.iter().enumerate().map(|(index, snapshot)| json!({"id":snapshot["surface_id"],"title":format!("Work inbox · {}",index+1),"inputs":[],"content":{"kind":"widget","spec":{"schema_version":2,"id":snapshot["surface_id"],"kind":"signal_view","title":format!("Work inbox · {}",index+1),"frame":snapshot["frame"],"refresh_ms":15000,"chrome":"frameless","data":{"state":"loading","provenance":"live","source":"marbles:cohort","detail":"Waiting for owner readings"}}}})).collect();
    surfaces.push(json!({"id":"work-rows","title":"Workspace work","inputs":[],"content":{"kind":"widget","spec":{"schema_version":2,"id":"work-rows","kind":"status_list","title":"Workspace work","nodes":[],"refresh_ms":15000,"chrome":"frameless","data":{"state":"loading","provenance":"live","source":"marbles:cohort","detail":"Waiting for owner readings"}}}}));
    let placements: Vec<_> = (0..SURFACES).map(|index| json!({"surface_id":format!("work-deck-{index}"),"bounds":{"x":(index%2) as f64*0.5,"y":(index/2) as f64*0.5,"width":0.5,"height":0.5},"layer":0})).collect();
    json!({"schema_version":1,"id":format!("tv-{workspace}"),"project_id":workspace,"revision":2,"theme":{"source":"project"},"background":{"kind":"theme"},"chrome":{"canvas_padding":12,"surface_gap":12,"title":"hidden"},"surfaces":surfaces,"layout":{"kind":"canvas","placements":[{"surface_id":"work-rows","bounds":{"x":0.0,"y":0.0,"width":1.0,"height":1.0},"layer":0}]},"pages":[{"name":"Task details","layout":{"kind":"canvas","placements":placements}}],"active_page":0,"bindings":[],"focused_surface_id":null})
}

/// Preserve the receiver's catalog, documents and selection, changing only our view.
/// The receiver still owns atomic admission against expected_revision.
pub fn install_request(config: &Config, checkpoint: &Value, owner: &str) -> Result<Value, String> {
    if checkpoint["schema_version"] != 1
        || checkpoint["owner"].as_str() != Some(owner)
        || owner.is_empty()
        || checkpoint["revision"].as_u64().is_none_or(|v| v == 0)
    {
        return Err("Checkpoint owner/version/revision mismatch".into());
    }
    let mut catalog = checkpoint["projection"]["catalog"].clone();
    let projects = catalog["projects"]
        .as_array_mut()
        .ok_or("Missing receiver project catalog")?;
    let matches = projects
        .iter()
        .filter(|p| p["id"] == config.workspace)
        .count();
    if matches > 1 {
        return Err("Duplicate workspace project".into());
    }
    if matches == 0 {
        projects.push(json!({"id":config.workspace,"name":"Work inbox","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}));
    }
    let mut documents = checkpoint["projection"]["documents"]
        .as_array()
        .ok_or("Missing receiver documents")?
        .clone();
    let positions: Vec<_> = documents
        .iter()
        .enumerate()
        .filter(|(_, d)| d["project_id"] == config.workspace)
        .map(|(n, _)| n)
        .collect();
    if positions.len() > 1 {
        return Err("Duplicate workspace document".into());
    }
    let mut next = document(&config.workspace);
    if let Some(&position) = positions.first() {
        if documents[position]["id"] != next["id"] {
            return Err("Workspace document identity changed; reconcile before install".into());
        }
        let revision = documents[position]["revision"]
            .as_u64()
            .and_then(|r| r.checked_add(1))
            .ok_or("Invalid document revision")?;
        next["revision"] = json!(revision);
        documents[position] = next;
    } else {
        next["revision"] = json!(1);
        documents.push(next);
    }
    let selected = checkpoint["projection"]["selected"]
        .as_str()
        .ok_or("Missing receiver selection")?;
    let request = json!({"schema_version":1,"request_id":"work-inbox-install","expected_revision":checkpoint["revision"],"command":{"command":"install","catalog":catalog,"documents":documents,"selected":selected}});
    if serde_json::to_vec(&request)
        .map_err(|_| "Cannot encode install request")?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("Install request exceeds receiver limit".into());
    }
    Ok(request)
}

/// Dense owner status rows use the existing cross-platform StatusList contract.
/// Public Macro screenshots informed reading order only; no source or assets copied.
pub fn presentation(result: Result<&[Issue], &str>, config: &Config, now: i64) -> Vec<Value> {
    let mut frames = snapshots(result);
    for frame in &mut frames {
        if let Some(items) = frame["frame"]["panel"]["items"].as_array_mut() {
            for item in items {
                if let Some(label) = item["project"]
                    .as_str()
                    .and_then(|p| config.project_labels.get(p))
                {
                    item["project"] = json!(bounded(label, 160));
                }
            }
        }
    }
    let ready = frames[0]["data"]["state"] == "ready";
    let mut rows: Vec<_> = if ready {
        result.unwrap_or(&[]).iter().collect()
    } else {
        vec![]
    };
    rows.sort_by_key(|i| {
        (
            matches!(i.status.as_str(), "closed" | "superseded"),
            i.priority,
            &i.project,
            &i.id,
        )
    });
    let done = rows
        .iter()
        .filter(|i| matches!(i.status.as_str(), "closed" | "superseded"))
        .count();
    let active = rows.len() - done;
    let nodes: Vec<_> = rows.iter().map(|i| {
        let claimed=i.assignee.is_some() && i.expires_at.is_some_and(|expiry| expiry>u64::try_from(now).unwrap_or(0));
        let (status,level)=match i.status.as_str() {
            "closed"|"superseded" => ("Done","good"),
            "in_progress" => ("In progress","warning"),
            "review" => ("In review","warning"),
            "blocked" => ("Blocked","critical"),
            "deferred" => ("Deferred","neutral"),
            "open" if claimed => ("Claimed","warning"),
            "open" => ("Open","neutral"),
            _ => ("Unknown","neutral"),
        };
        let project=config.project_labels.get(&i.project).map_or(i.project.as_str(),String::as_str);
        let holder=if i.assignee.as_deref().is_some() && i.assignee==config.viewer_principal { "You" } else if i.assignee.is_some() { i.actor_kind.as_deref().unwrap_or("Assigned") } else { "Unassigned" };
        let priority=match i.priority {0=>"Urgent",1=>"High",2=>"Normal",3=>"Low",_=>"Backlog"};
        json!({"label":bounded(&i.title,160),"state":level,"detail":bounded(&format!("{status} · {priority} · {project} · {holder}"),160)})
    }).collect();
    let observed = chrono::DateTime::from_timestamp(now, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let detail = if ready {
        format!(
            "{} records from {} projects · read-only",
            rows.len(),
            config.trackers.len()
        )
    } else {
        frames[0]["data"]["detail"]
            .as_str()
            .unwrap_or("Owner unavailable")
            .into()
    };
    frames.insert(0,json!({"surface_id":"work-rows","frame":{"title":if ready {format!("Workspace work · {active} open · {done} done")} else {"Workspace work · unavailable".into()},"nodes":nodes},"data":{"state":if ready {"ready"} else {"stale"},"provenance":"live","source":"marbles:cohort","observed_at":observed,"detail":detail}}));
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<Issue> {
        parse_owner(br#"[{"id":"issue","project":"dock","title":"Native inbox","status":"open","priority":2,"metadata":{"coordination_issue":"cohort"}}]"#, "dock","cohort").expect("fixture")
    }
    #[test]
    fn refuses_cross_scope_before_filtering() {
        assert!(parse_owner(br#"[{"id":"other","project":"private","title":"Secret","status":"open","priority":2}]"#, "dock", "cohort").is_err());
    }
    #[test]
    fn filters_unrelated_and_duplicate_records() {
        assert!(parse_owner(br#"[{"id":"other","project":"dock","title":"Other","status":"open","priority":2}]"#, "dock", "cohort").expect("valid").is_empty());
        let row = serde_json::to_value(
            json!({"id":"a","project":"dock","title":"A","status":"open","priority":2}),
        )
        .expect("value");
        assert!(
            parse_owner(
                &serde_json::to_vec(&vec![row.clone(), row]).expect("json"),
                "dock",
                "cohort"
            )
            .is_err()
        );
    }
    #[test]
    fn open_work_does_not_claim_input_or_approval() {
        let frames = snapshots(Ok(&rows()));
        assert_eq!(frames[0]["frame"]["panel"]["items"][0]["state"], "idle");
        assert_eq!(frames[0]["frame"]["panel"]["items"][0]["id"], "dock:issue");
        assert!(frames[0]["frame"]["panel"].get("selected").is_none());
    }
    #[test]
    fn failures_clear_all_decks_without_leaking_owner_error() {
        let frames = snapshots(Err("private owner credential error"));
        for f in &frames {
            assert_eq!(f["frame"]["panel"]["fresh"], false);
            assert_eq!(f["frame"]["panel"]["items"], json!([]));
        }
        assert!(
            !serde_json::to_string(&frames)
                .expect("json")
                .contains("credential")
        );
    }
    #[test]
    fn overflow_is_visible_rather_than_silently_hiding_work() {
        let rows = vec![rows()[0].clone(); 17];
        assert_eq!(snapshots(Ok(&rows))[0]["data"]["state"], "stale");
    }
    #[test]
    fn utf8_limits_preserve_valid_text_and_strip_controls() {
        let text = bounded(&"界".repeat(200), 240);
        assert!(text.len() <= 240);
        assert!(text.ends_with('…'));
        assert_eq!(bounded("a\u{1b}\nb", 240), "a\nb");
    }
    #[test]
    fn persisted_document_never_claims_a_live_owner_read() {
        for s in document("work-inbox")["surfaces"]
            .as_array()
            .expect("surfaces")
        {
            assert_eq!(s["content"]["spec"]["data"]["state"], "loading");
            if s["content"]["spec"]["kind"] == "signal_view" {
                assert_eq!(s["content"]["spec"]["frame"]["panel"]["fresh"], false);
            } else {
                assert_eq!(s["content"]["spec"]["nodes"], json!([]));
            }
        }
    }
    fn config() -> Config {
        Config {
            root: "/tmp".into(),
            marbles: "/usr/bin/false".into(),
            trackers: vec!["dock".into()],
            cohort: "cohort".into(),
            workspace: "work-inbox".into(),
            project_labels: BTreeMap::from([("dock".into(), "Canvas".into())]),
            viewer_principal: Some("viewer".into()),
        }
    }
    #[test]
    fn dense_rows_lead_with_task_names_and_explicit_owner_status() {
        let mut rows = rows();
        rows[0].assignee = Some("viewer".into());
        rows[0].expires_at = Some(2100);
        let frames = presentation(Ok(&rows), &config(), 2000);
        let node = &frames[0]["frame"]["nodes"][0];
        assert_eq!(node["label"], "Native inbox");
        assert_eq!(node["state"], "warning");
        assert_eq!(node["detail"], "Claimed · Normal · Canvas · You");
        assert_eq!(frames[0]["data"]["observed_at"], "1970-01-01T00:33:20.000Z");
        let expired = presentation(Ok(&rows), &config(), 2200);
        assert!(
            expired[0]["frame"]["nodes"][0]["detail"]
                .as_str()
                .expect("detail")
                .starts_with("Open")
        );
    }
    #[test]
    fn completed_rows_sort_after_open_work_and_keep_distinct_status_color() {
        let mut rows = vec![rows()[0].clone(); 2];
        rows[0].status = "closed".into();
        rows[0].id = "done".into();
        let frames = presentation(Ok(&rows), &config(), 2000);
        assert_eq!(frames[0]["frame"]["nodes"][0]["state"], "neutral");
        assert_eq!(frames[0]["frame"]["nodes"][1]["state"], "good");
        assert_eq!(
            frames[0]["frame"]["title"],
            "Workspace work · 1 open · 1 done"
        );
    }
    #[test]
    fn dense_failure_has_no_rows_and_visible_recovery_state() {
        let frames = presentation(Err("owner secret"), &config(), 2000);
        assert_eq!(frames[0]["frame"]["nodes"], json!([]));
        assert_eq!(frames[0]["data"]["state"], "stale");
        assert!(
            !serde_json::to_string(&frames)
                .expect("json")
                .contains("owner secret")
        );
    }
    #[test]
    fn installer_preserves_other_workspaces_and_revision_checks_owner() {
        let config = config();
        let other =
            json!({"id":"other-document","project_id":"other","revision":9,"custom":"keep"});
        let project = json!({"id":"other","custom":"keep"});
        let checkpoint = json!({"schema_version":1,"owner":"tv","revision":42,"projection":{"catalog":{"schema_version":1,"projects":[project.clone()]},"documents":[other.clone()],"selected":"other"}});
        let request = install_request(&config, &checkpoint, "tv").expect("install");
        assert_eq!(request["expected_revision"], 42);
        assert_eq!(request["command"]["selected"], "other");
        assert_eq!(request["command"]["documents"][0], other);
        assert_eq!(request["command"]["catalog"]["projects"][0], project);
        assert!(install_request(&config, &checkpoint, "wrong-owner").is_err());
        let mut checkpoint = checkpoint;
        checkpoint["projection"]["catalog"] = request["command"]["catalog"].clone();
        checkpoint["projection"]["documents"] = request["command"]["documents"].clone();
        let update = install_request(&config, &checkpoint, "tv").expect("update");
        assert_eq!(
            update["command"]["documents"]
                .as_array()
                .expect("documents")
                .len(),
            2
        );
        assert_eq!(update["command"]["documents"][1]["revision"], 2);
        checkpoint["projection"]["documents"][1]["id"] = json!("someone-elses-document");
        assert!(install_request(&config, &checkpoint, "tv").is_err());
    }
}
