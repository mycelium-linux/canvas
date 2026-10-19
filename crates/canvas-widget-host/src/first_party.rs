//! Bounded first-party owner observations, following the Panopticon source/presenter boundary.
//! No credentials, delivery, execution, or recipient selection belong in these projections.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const SCHEMA: &str = "canvas.first-party/1";
pub const KIND: &str = "canvas.first-party.snapshot.v1";
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 64;
pub const FRESH_MS: u64 = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    OhmBridge,
    Ohmos,
    Alfalfa,
    Edgerunner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub title: String,
    /// Owner vocabulary, not a guessed execution or delivery state.
    pub status: String,
    pub detail: String,
    /// Owner timestamp when supplied. Reading an old message does not update it.
    pub updated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub schema: String,
    pub owner: Owner,
    /// Operator-enrolled owner instance/account, never derived from a message title.
    pub source: String,
    /// Project ID for coding; authorized messaging store scope for ohmOS.
    pub scope: String,
    pub read_at_unix_ms: u64,
    pub state: String,
    /// The legacy ohmOS seam cannot attest real versus seeded conversations.
    pub provenance: String,
    pub total: usize,
    pub omitted: usize,
    pub error_count: usize,
    pub rows: Vec<Row>,
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

fn text(value: &Value, key: &str, limit: usize) -> Result<String, String> {
    let text = value[key]
        .as_str()
        .ok_or_else(|| format!("missing {key}"))?;
    if text.is_empty() || text.len() > limit || text.chars().any(char::is_control) {
        return Err(format!("invalid {key}"));
    }
    Ok(text.into())
}

/// Read the existing dumb-bridge Snapshot contract; never copy its send policy.
fn bridge_project(
    source: &str,
    scope: &str,
    input: &Value,
    read_at_unix_ms: u64,
    selected: &BTreeSet<String>,
) -> Result<Projection, String> {
    use std::collections::BTreeMap;
    if selected.is_empty() {
        return Err("explicit messaging account enrollment required".into());
    }
    let accounts = input["accounts"]
        .as_array()
        .ok_or("missing bridge accounts")?;
    let conversations = input["conversations"]
        .as_array()
        .ok_or("missing bridge conversations")?;
    let errors = input["errors"]
        .as_array()
        .ok_or("missing bridge source errors")?;
    if accounts.len() > 128
        || conversations.len() > 4096
        || errors.len() > 4096
        || errors.iter().any(|error| !error.is_string())
    {
        return Err("invalid or oversized bridge snapshot".into());
    }
    let mut by_id = BTreeMap::new();
    let mut sample = false;
    for account in accounts {
        let id = text(account, "id", 256)?;
        let transport = text(account, "transport", 64)?;
        let demo = account["demo"]
            .as_bool()
            .ok_or("missing account provenance")?;
        if selected.contains(&id) {
            // The legacy ohmos provider wraps the unverified msgd seam. Unknown
            // adapters do not earn live provenance merely by setting demo=false.
            sample |= demo || !(id.starts_with("iroh:") || id.starts_with("adb:"));
        }
        if by_id.insert(id, (transport, demo)).is_some() {
            return Err("duplicate bridge account".into());
        }
    }
    let missing = selected
        .iter()
        .filter(|id| !by_id.contains_key(*id))
        .count();
    let error_count = errors
        .len()
        .checked_add(missing)
        .ok_or("source error count overflow")?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0;
    for conversation in conversations {
        let account = text(conversation, "account_id", 256)?;
        let (transport, _) = by_id
            .get(&account)
            .ok_or("conversation refers to an unknown account")?;
        let id = text(conversation, "id", 256)?;
        if !seen.insert((account.clone(), id.clone())) {
            return Err("duplicate account conversation".into());
        }
        if !selected.contains(&account) {
            continue;
        }
        let unread = conversation["unread"]
            .as_u64()
            .ok_or("missing unread count")?;
        let updated = conversation["updated_at"]
            .as_u64()
            .ok_or("invalid conversation timestamp")?;
        let row = Row {
            id,
            account_id: Some(account),
            title: text(conversation, "title", 512)?,
            status: if unread == 0 {
                "No unread".into()
            } else {
                format!("{unread} unread")
            },
            detail: transport.clone(),
            updated_at_unix_ms: Some(updated.checked_mul(1000).ok_or("timestamp overflow")?),
        };
        total += 1;
        if rows.len() < MAX_ROWS {
            rows.push(row);
        }
    }
    let result = Projection {
        schema: SCHEMA.into(),
        owner: Owner::OhmBridge,
        source: source.into(),
        scope: scope.into(),
        read_at_unix_ms,
        state: if error_count > 0 {
            "partial"
        } else if total == 0 {
            "empty"
        } else {
            "ready"
        }
        .into(),
        provenance: if sample { "sample" } else { "live" }.into(),
        omitted: total - rows.len(),
        total,
        error_count,
        rows,
    };
    result.validate()?;
    Ok(result)
}

/// Normalize only the documented owner read. Unknown fields never enter Canvas.
/// The final argument enrolls Edgerunner session IDs or bridge account IDs.
/// Labels and enrollment associations are never grants.
pub fn project(
    owner: Owner,
    source: &str,
    scope: &str,
    input: &Value,
    read_at_unix_ms: u64,
    session_ids: &BTreeSet<String>,
) -> Result<Projection, String> {
    if !identifier(source) || !identifier(scope) || read_at_unix_ms == 0 {
        return Err("invalid source, scope or read time".into());
    }
    if owner == Owner::OhmBridge {
        return bridge_project(source, scope, input, read_at_unix_ms, session_ids);
    }
    let mut rows = Vec::new();
    let inputs: Vec<&Value> = match owner {
        Owner::OhmBridge => return Err("bridge projection must use its account contract".into()),
        Owner::Ohmos => input
            .as_array()
            .ok_or("ohmOS inbox must be an array")?
            .iter()
            .collect(),
        Owner::Edgerunner => {
            if session_ids.is_empty() {
                return Err("no coding sessions enrolled to this project".into());
            }
            if input["schema_version"] != 1 {
                return Err("owned-session contract required".into());
            }
            input["sessions"]
                .as_array()
                .ok_or("missing owned sessions")?
                .iter()
                .collect()
        }
        Owner::Alfalfa => {
            if input["schema"] != "alfalfa.manifest-view/1" || input["project_id"] != scope {
                return Err("Alfalfa project identity/contract mismatch".into());
            }
            vec![input]
        }
    };
    if inputs.len() > 4096 {
        return Err("owner row budget exceeded".into());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0;
    for value in inputs {
        let row = match owner {
            Owner::OhmBridge => return Err("unexpected bridge projection path".into()),
            Owner::Alfalfa => Row {
                id: scope.into(),
                account_id: None,
                title: text(value, "name", 512)?,
                status: text(value, "status", 64)?,
                detail: text(value, "purpose", 4096)?,
                updated_at_unix_ms: None,
            },
            Owner::Edgerunner => {
                let id = text(value, "id", 256)?;
                if !session_ids.contains(&id) {
                    continue;
                }
                Row {
                    id,
                    account_id: None,
                    title: value["title"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Untitled session")
                        .into(),
                    status: text(value, "status", 64)?,
                    detail: "Owner-authorized coding session".into(),
                    updated_at_unix_ms: None,
                }
            }
            Owner::Ohmos => {
                let id = value["id"]
                    .as_i64()
                    .filter(|id| *id > 0)
                    .ok_or("invalid chat ID")?;
                let platform = text(value, "platform", 64)?;
                let unread = value["unread"].as_u64().ok_or("missing unread count")?;
                let stamp = value["last_ts"]
                    .as_u64()
                    .ok_or("invalid message timestamp")?;
                Row {
                    id: id.to_string(),
                    account_id: None,
                    title: text(value, "title", 512)?,
                    status: if unread == 0 {
                        "No unread".into()
                    } else {
                        format!("{unread} unread")
                    },
                    // Message bodies, recipient IDs and attachments stay with ohmOS.
                    detail: platform,
                    updated_at_unix_ms: Some(stamp.checked_mul(1000).ok_or("timestamp overflow")?),
                }
            }
        };
        if !seen.insert(row.id.clone()) {
            return Err("duplicate owner ID".into());
        }
        total += 1;
        if rows.len() < MAX_ROWS {
            rows.push(row);
        }
    }
    let result = Projection {
        schema: SCHEMA.into(),
        owner,
        source: source.into(),
        scope: scope.into(),
        read_at_unix_ms,
        state: if rows.is_empty() { "empty" } else { "ready" }.into(),
        provenance: if owner == Owner::Ohmos {
            "sample"
        } else {
            "live"
        }
        .into(),
        total,
        omitted: total - rows.len(),
        error_count: 0,
        rows,
    };
    result.validate()?;
    Ok(result)
}

impl Projection {
    pub fn disconnected(owner: Owner, source: &str, scope: &str, read_at_unix_ms: u64) -> Self {
        Self {
            schema: SCHEMA.into(),
            owner,
            source: source.into(),
            scope: scope.into(),
            read_at_unix_ms,
            state: "disconnected".into(),
            provenance: if owner == Owner::Ohmos {
                "sample"
            } else {
                "live"
            }
            .into(),
            total: 0,
            omitted: 0,
            error_count: 1,
            rows: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA
            || !identifier(&self.source)
            || !identifier(&self.scope)
            || self.read_at_unix_ms == 0
            || self.rows.len() > MAX_ROWS
            || self.total > 4096
            || self.total.checked_sub(self.rows.len()) != Some(self.omitted)
            || !matches!(
                self.state.as_str(),
                "ready" | "empty" | "disconnected" | "partial"
            )
            || !matches!(self.provenance.as_str(), "sample" | "live")
            || (self.owner == Owner::Ohmos && self.provenance != "sample")
            || (self.state == "ready" && self.rows.is_empty())
            || (matches!(self.state.as_str(), "empty" | "disconnected") && self.total != 0)
            || self.error_count > 4096
            || (self.state == "partial" && self.error_count == 0)
            || (matches!(self.state.as_str(), "ready" | "empty") && self.error_count != 0)
        {
            return Err("invalid first-party projection".into());
        }
        let mut ids = BTreeSet::new();
        for row in &self.rows {
            for (value, limit) in [
                (&row.id, 256),
                (&row.title, 512),
                (&row.status, 64),
                (&row.detail, 4096),
            ] {
                if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
                    return Err("invalid projected text".into());
                }
            }
            if let Some(account) = &row.account_id {
                if account.is_empty()
                    || account.len() > 256
                    || account.chars().any(char::is_control)
                {
                    return Err("invalid account ID".into());
                }
            }
            if (self.owner == Owner::OhmBridge) != row.account_id.is_some() {
                return Err("account identity does not match owner contract".into());
            }
            if !ids.insert((&row.account_id, &row.id)) {
                return Err("duplicate projected ID".into());
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_BYTES {
            return Err("projection exceeds 64KiB".into());
        }
        Ok(())
    }
}

/// Recipient-local aggregate: project context and coding sessions stay separate
/// owner observations. A failed source cannot erase or freshen its neighbor.
#[derive(Debug, Serialize)]
pub struct CodingWorkspace<'a> {
    pub schema: &'static str,
    pub project_id: &'a str,
    pub context: CodingSource<'a>,
    pub sessions: CodingSource<'a>,
}

#[derive(Debug, Serialize)]
pub struct CodingSource<'a> {
    pub state: &'a str,
    pub observation: Option<&'a Projection>,
}

pub fn coding<'a>(
    project_id: &'a str,
    context: Option<&'a Projection>,
    sessions: Option<&'a Projection>,
    now: u64,
) -> Result<CodingWorkspace<'a>, String> {
    if !identifier(project_id) {
        return Err("invalid project ID".into());
    }
    let source = |owner, value: Option<&'a Projection>| -> Result<CodingSource<'a>, String> {
        let Some(value) = value else {
            return Ok(CodingSource {
                state: "not_connected",
                observation: None,
            });
        };
        value.validate()?;
        if value.owner != owner || value.scope != project_id {
            return Err("coding owner/project mismatch".into());
        }
        if value.read_at_unix_ms > now.saturating_add(5000) {
            return Err("future owner read".into());
        }
        Ok(CodingSource {
            state: if now.saturating_sub(value.read_at_unix_ms) >= FRESH_MS {
                "stale"
            } else {
                &value.state
            },
            observation: Some(value),
        })
    };
    Ok(CodingWorkspace {
        schema: "canvas.coding-workspace/1",
        project_id,
        context: source(Owner::Alfalfa, context)?,
        sessions: source(Owner::Edgerunner, sessions)?,
    })
}

/// Per-source receive cursor. Authentication/grants remain Unibus's responsibility.
#[derive(Default)]
pub struct Cursor {
    previous: (u64, u64),
}
impl Cursor {
    pub fn accept(
        &mut self,
        projection: &Projection,
        sequence: u64,
        now: u64,
    ) -> Result<(), String> {
        projection.validate()?;
        let stamp = projection.read_at_unix_ms;
        if sequence == 0
            || stamp > now.saturating_add(5000)
            || now.saturating_sub(stamp) >= FRESH_MS
            || (stamp, sequence) <= self.previous
        {
            return Err("expired, future or replayed projection".into());
        }
        self.previous = (stamp, sequence);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn chat(id: i64) -> Value {
        json!({"id":id,"title":"A person","platform":"signal","unread":2,"last_ts":1,"last_text":"PRIVATE","remote_id":"PRIVATE"})
    }
    #[test]
    fn messaging_preserves_message_age_and_excludes_private_payloads() {
        let p = project(
            Owner::Ohmos,
            "phone",
            "personal",
            &json!([chat(1)]),
            10000,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(p.rows[0].updated_at_unix_ms, Some(1000));
        assert!(!serde_json::to_string(&p).unwrap().contains("PRIVATE"));
        assert_eq!(p.rows[0].status, "2 unread");
        assert_eq!(p.provenance, "sample");
    }
    #[test]
    fn duplicate_ids_and_incomplete_reads_never_look_empty() {
        assert!(project(
            Owner::Ohmos,
            "phone",
            "personal",
            &json!([chat(1), chat(1)]),
            10000,
            &BTreeSet::new()
        )
        .is_err());
        assert!(project(
            Owner::Ohmos,
            "phone",
            "personal",
            &json!({"error":"denied"}),
            10000,
            &BTreeSet::new()
        )
        .is_err());
        assert_eq!(
            project(
                Owner::Ohmos,
                "phone",
                "personal",
                &json!([]),
                10000,
                &BTreeSet::new()
            )
            .unwrap()
            .state,
            "empty"
        );
    }
    #[test]
    fn coding_requires_owner_contract_and_explicit_project_membership() {
        let sessions = json!({"schema_version":1,"sessions":[{"id":"s1","title":"Work","status":"running","labels":{"project":"victim"}},{"id":"s2","title":"Elsewhere","status":"running"}]});
        let p = project(
            Owner::Edgerunner,
            "coding",
            "alfalfa",
            &sessions,
            10000,
            &BTreeSet::from(["s1".into()]),
        )
        .unwrap();
        assert_eq!(p.total, 1);
        assert_eq!(p.rows[0].id, "s1");
        assert!(project(
            Owner::Alfalfa,
            "projects",
            "a",
            &json!({"schema":"alfalfa.manifest-view/1","project_id":"b"}),
            10000,
            &BTreeSet::new()
        )
        .is_err());
    }
    #[test]
    fn truncation_is_explicit_and_bad_late_rows_are_still_rejected() {
        let mut chats: Vec<_> = (1..=80).map(chat).collect();
        let p = project(
            Owner::Ohmos,
            "phone",
            "personal",
            &json!(chats),
            10000,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!((p.rows.len(), p.omitted), (64, 16));
        chats[79]["unread"] = Value::Null;
        assert!(project(
            Owner::Ohmos,
            "phone",
            "personal",
            &json!(chats),
            10000,
            &BTreeSet::new()
        )
        .is_err());
    }
    #[test]
    fn receiver_does_not_freshen_replayed_or_expired_data() {
        let p = Projection::disconnected(Owner::Ohmos, "phone", "personal", 10000);
        let mut cursor = Cursor::default();
        cursor.accept(&p, 1, 10000).unwrap();
        assert!(cursor.accept(&p, 1, 10001).is_err());
        assert!(cursor.accept(&p, 2, 30000).is_err());
        assert!(cursor.accept(&p, 2, 4000).is_err());
        cursor.accept(&p, 2, 10001).unwrap();
    }

    #[test]
    fn coding_aggregates_by_project_and_expires_each_owner_independently() {
        let context=project(Owner::Alfalfa,"projects","alfalfa",&json!({"schema":"alfalfa.manifest-view/1","project_id":"alfalfa","name":"Alfalfa","status":"active","purpose":"Project context"}),10000,&BTreeSet::new()).unwrap();
        let sessions = Projection::disconnected(Owner::Edgerunner, "coding", "alfalfa", 29000);
        let workspace = coding("alfalfa", Some(&context), Some(&sessions), 30000).unwrap();
        assert_eq!(workspace.context.state, "stale");
        assert_eq!(workspace.sessions.state, "disconnected");
        assert_eq!(
            workspace.context.observation.unwrap().rows[0].title,
            "Alfalfa"
        );
        assert!(coding("another", Some(&context), None, 30000).is_err());
        assert_eq!(
            coding("alfalfa", None, None, 30000).unwrap().sessions.state,
            "not_connected"
        );
    }

    #[test]
    fn bridge_preserves_account_identity_demo_provenance_and_partial_failures() {
        let mut input = json!({"accounts":[{"id":"iroh:sms","transport":"SMS","demo":false},{"id":"demo:sms","transport":"SMS","demo":true}],"conversations":[{"id":"same","account_id":"iroh:sms","title":"Real phone conversation","updated_at":1,"unread":2,"preview":"PRIVATE","recipient":"PRIVATE"},{"id":"same","account_id":"demo:sms","title":"Sample conversation","updated_at":1,"unread":0}],"errors":[]});
        let accounts = BTreeSet::from(["iroh:sms".into()]);
        let live = project(
            Owner::OhmBridge,
            "phone",
            "personal",
            &input,
            10000,
            &accounts,
        )
        .unwrap();
        assert_eq!(live.provenance, "live");
        assert_eq!(live.total, 1);
        assert_eq!(live.rows[0].account_id.as_deref(), Some("iroh:sms"));
        assert!(!serde_json::to_string(&live).unwrap().contains("PRIVATE"));
        let both = project(
            Owner::OhmBridge,
            "phone",
            "personal",
            &input,
            10000,
            &BTreeSet::from(["iroh:sms".into(), "demo:sms".into()]),
        )
        .unwrap();
        assert_eq!(both.provenance, "sample");
        assert_eq!(both.total, 2);
        input["errors"] = json!(["PRIVATE source failure"]);
        let partial = project(
            Owner::OhmBridge,
            "phone",
            "personal",
            &input,
            10000,
            &accounts,
        )
        .unwrap();
        assert_eq!(partial.state, "partial");
        assert_eq!(partial.error_count, 1);
        assert_eq!(partial.total, 1);
        assert!(!serde_json::to_string(&partial).unwrap().contains("PRIVATE"));
        let missing = project(
            Owner::OhmBridge,
            "phone",
            "personal",
            &input,
            10000,
            &BTreeSet::from(["iroh:signal".into()]),
        )
        .unwrap();
        assert_eq!(missing.state, "partial");
        assert_eq!(missing.total, 0);
    }
}
