//! Deterministic synthetic owner. It cannot call Gmail, Calendar or a message transport.
use crate::{
    Item, Snapshot, Status,
    mail::{Action, Attachment, Intent, KeyId, Message, Thread},
};
use std::collections::HashMap;

pub const ACCOUNTS: [&str; 3] = ["avery@fpl.dev", "avery@wagar.cc", "ajmw.subs@gmail.com"];
pub struct Owner {
    pub threads: Vec<Thread>,
    pub drafts: HashMap<KeyId, String>,
    history: Vec<Vec<Undo>>,
    snoozes: HashMap<KeyId, u64>,
    pub now: u64,
}
struct Undo {
    key: KeyId,
    thread: Option<Thread>,
    draft: Option<String>,
    restore_draft: bool,
    wake_at: Option<u64>,
}
impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}
impl Owner {
    pub fn new() -> Self {
        let seeds = [
            (
                "Mira Vale",
                "Enclosure fit check before Friday",
                "The revised mounting points are ready for review.",
                "Project",
            ),
            (
                "Jonas Reed",
                "Canvas / keyboard pass",
                "I've left notes on the reader pane and account switcher.",
                "Engineering",
            ),
            (
                "Rowan Ellis",
                "Updated prototype drawings",
                "The STEP export and dimensions are attached.",
                "Project",
            ),
            (
                "Northline Studio",
                "Thursday design review",
                "Could we move the review to 2:30?",
                "Calendar",
            ),
            (
                "Alfalfa",
                "Three projects need a decision",
                "Two design reviews and one owner mapping are waiting.",
                "Work",
            ),
            (
                "Sage Martin",
                "Quick question about the panel finish",
                "Would the matte finish work for this first run?",
                "Project",
            ),
            (
                "Edgerunner",
                "Shared workspace build is ready",
                "The view-model checks passed. GPU review is next.",
                "Engineering",
            ),
            (
                "Field Notes",
                "A small list of things worth reading",
                "Notes on calmer software, notebooks and making things.",
                "Reading",
            ),
            (
                "Casey Lane",
                "Lunch next week?",
                "Tuesday or Thursday would work for me.",
                "Personal",
            ),
            (
                "Paper Trail",
                "Your order is on its way",
                "Tracking details for your notebook order.",
                "Receipt",
            ),
            (
                "Research Wing",
                "Weekly research digest",
                "This week's sources and two open questions.",
                "Reading",
            ),
            (
                "ohmOS",
                "Messaging view notes",
                "The paired account list is ready for the next pass.",
                "Engineering",
            ),
        ];
        let mut threads = Vec::new();
        for index in 0..48 {
            let (sender, subject, snippet, label) = seeds[index % seeds.len()];
            let account = ACCOUNTS[if index < 6 { 0 } else { index % 3 }];
            let id = KeyId {
                account: account.into(),
                thread: format!("demo-thread-{index}"),
            };
            let address = format!(
                "{}@example.com",
                sender
                    .split_whitespace()
                    .next()
                    .unwrap_or("demo")
                    .to_lowercase()
            );
            let body = if index == 0 {
                "Hi Avery,\n\nThe revised mounting points are ready for review. I moved the two rear bosses 1.5 mm inward and left the front datum alone. The connector clearance is now 2 mm above the keep-out.\n\nCould you check the attached section before Friday's prototype run? The one open question is whether we keep the flush fastener or switch to the countersunk option.\n\nMy preference is to keep this first batch easy to inspect, then tighten the finish once we have the fit confirmed.\n\nThanks,\nMira"
            } else {
                snippet
            };
            let mut messages = vec![Message {
                sender: sender.into(),
                address,
                to: account.into(),
                time: "Today, 11:42".into(),
                body: body.into(),
            }];
            if index == 0 {
                messages.insert(0,Message{sender:"Avery".into(),address:account.into(),to:"mira@example.com".into(),time:"Yesterday, 16:20".into(),body:"Thanks for taking another look. Please keep the connector datum fixed so we can compare this against the last fit check.\n\nA section through the rear mount would be useful before we cut the next set.".into()});
            }
            threads.push(Thread {
                id,
                sender: sender.into(),
                subject: subject.into(),
                snippet: snippet.into(),
                time: if index < 8 {
                    format!("{}:{:02}", 11 - index / 2, 42 - (index % 2) * 17)
                } else if index < 22 {
                    "Yesterday".into()
                } else {
                    "Oct 17".into()
                },
                unread: index % 3 != 1,
                starred: index == 0 || index == 4 || index == 8,
                archived: false,
                snoozed: false,
                label: label.into(),
                messages,
                attachments: if index == 0 {
                    vec![
                        Attachment {
                            name: "enclosure-section-r3.pdf".into(),
                            detail: "PDF · 284 KB · demo".into(),
                        },
                        Attachment {
                            name: "mounting-clearance.step".into(),
                            detail: "STEP · 1.2 MB · demo".into(),
                        },
                    ]
                } else {
                    vec![]
                },
                context: if index == 0 {
                    vec![
                        "Alfalfa / Enclosure prototype / Fit review".into(),
                        "Marbles / Rear mounting clearance / In review".into(),
                    ]
                } else {
                    vec![]
                },
            });
        }
        Self {
            threads,
            drafts: HashMap::new(),
            history: Vec::new(),
            snoozes: HashMap::new(),
            now: 0,
        }
    }
    fn remember(&mut self, keys: &[KeyId], restore_draft: bool) {
        let group = keys
            .iter()
            .map(|key| Undo {
                key: key.clone(),
                thread: self.threads.iter().find(|t| &t.id == key).cloned(),
                draft: self.drafts.get(key).cloned(),
                restore_draft,
                wake_at: self.snoozes.get(key).copied(),
            })
            .collect();
        if self.history.len() >= 32 {
            self.history.remove(0);
        }
        self.history.push(group);
    }
    /// Clock supplied by the host; timers remain deterministic in tests and screenshots.
    pub fn advance_to(&mut self, now: u64) -> usize {
        self.now = self.now.max(now);
        let due: Vec<_> = self
            .snoozes
            .iter()
            .filter(|(_, at)| **at <= self.now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &due {
            if let Some(t) = self.threads.iter_mut().find(|t| &t.id == key) {
                t.snoozed = false;
                t.archived = false;
            }
            self.snoozes.remove(key);
        }
        due.len()
    }
    pub fn apply(&mut self, intent: Intent) -> Result<String, String> {
        if let Intent::Undo = intent {
            let Some(group) = self.history.pop() else {
                return Ok("Nothing to undo".into());
            };
            for old in group {
                if let Some(thread) = old.thread {
                    if let Some(row) = self.threads.iter_mut().find(|t| t.id == old.key) {
                        *row = thread;
                    }
                } else {
                    self.threads.retain(|t| t.id != old.key);
                }
                if old.restore_draft {
                    match old.draft {
                        Some(draft) => {
                            self.drafts.insert(old.key.clone(), draft);
                        }
                        None => {
                            self.drafts.remove(&old.key);
                        }
                    }
                }
                match old.wake_at {
                    Some(at) => {
                        self.snoozes.insert(old.key, at);
                    }
                    None => {
                        self.snoozes.remove(&old.key);
                    }
                }
            }
            self.advance_to(self.now);
            return Ok("Change undone".into());
        }
        if let Intent::ComposePreview {
            account,
            to,
            subject,
            body,
        } = intent
        {
            if !ACCOUNTS.contains(&account.as_str())
                || to.len() > 320
                || subject.len() > 512
                || body.trim().is_empty()
                || body.len() > 64 * 1024
            {
                return Err("Invalid demo draft identity or content".into());
            }
            let mut suffix = self.threads.len();
            let key = loop {
                let key = KeyId {
                    account: account.clone(),
                    thread: format!("demo-compose-{suffix}"),
                };
                if !self.threads.iter().any(|t| t.id == key) {
                    break key;
                }
                suffix += 1;
            };
            self.remember(std::slice::from_ref(&key), true);
            self.threads.insert(
                0,
                Thread {
                    id: key.clone(),
                    sender: "Draft".into(),
                    subject: if subject.is_empty() {
                        "(No subject)".into()
                    } else {
                        subject
                    },
                    snippet: body.chars().take(128).collect(),
                    time: "Now".into(),
                    unread: false,
                    starred: false,
                    archived: true,
                    snoozed: false,
                    label: "Draft preview".into(),
                    messages: vec![Message {
                        sender: "Avery".into(),
                        address: account.clone(),
                        to: to.clone(),
                        time: "Draft preview".into(),
                        body: body.clone(),
                    }],
                    attachments: vec![],
                    context: vec![format!("To: {to}")],
                },
            );
            self.drafts.insert(key, body);
            return Ok(format!("Draft preview saved for {account}"));
        }
        let checkpoint = matches!(intent, Intent::SavePreview { .. });
        if let Intent::SavePreview { key, body } | Intent::EditPreview { key, body } = intent {
            if !self.threads.iter().any(|t| t.id == key) || body.len() > 64 * 1024 {
                return Err("Invalid draft identity or size".into());
            }
            if checkpoint {
                self.remember(std::slice::from_ref(&key), true);
            }
            if body.trim().is_empty() {
                self.drafts.remove(&key);
            } else {
                self.drafts.insert(key.clone(), body);
            }
            return Ok(format!("Draft preview saved for {}", key.account));
        }
        let record_undo = !matches!(intent, Intent::Open(_));
        let (keys, action) = match intent {
            Intent::Bulk { keys, action } => (keys, action),
            Intent::Archive(k) => (vec![k], Action::Archive),
            Intent::Star(k) => (vec![k], Action::Star),
            Intent::Unread(k) => (vec![k], Action::Unread),
            Intent::Snooze(k) => (vec![k], Action::Snooze),
            Intent::Open(k) => (vec![k], Action::Read),
            _ => return Err("Unsupported demo action".into()),
        };
        let mut unique = std::collections::HashSet::new();
        if keys.is_empty()
            || keys.len() > 1000
            || keys
                .iter()
                .any(|k| !unique.insert(k) || !self.threads.iter().any(|t| &t.id == k))
        {
            return Err("Invalid bulk identities; nothing changed".into());
        }
        if record_undo {
            self.remember(&keys, false);
        }
        for key in &keys {
            if let Some(row) = self.threads.iter_mut().find(|t| &t.id == key) {
                match action {
                    Action::Archive => {
                        row.archived = true;
                        row.snoozed = false;
                        self.snoozes.remove(key);
                    }
                    Action::Star => row.starred = !row.starred,
                    Action::Unread => row.unread = true,
                    Action::Read => row.unread = false,
                    Action::Snooze => {
                        row.snoozed = true;
                        self.snoozes
                            .insert(key.clone(), self.now.saturating_add(86400));
                    }
                }
            }
        }
        Ok(format!(
            "{} · {} conversation(s) · demo",
            action.label(),
            keys.len()
        ))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedOwner {
    version: u32,
    threads: Vec<Thread>,
    drafts: Vec<(KeyId, String)>,
    snoozes: Vec<(KeyId, u64)>,
    // The host supplies current time. Ticking alone must not rewrite flash storage.
    #[serde(default, skip_serializing)]
    now: u64,
}
impl Owner {
    pub fn save(&self) -> Result<String, String> {
        serde_json::to_string(&SavedOwner {
            version: 1,
            threads: self.threads.clone(),
            drafts: self
                .drafts
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            snoozes: self.snoozes.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            now: self.now,
        })
        .map_err(|e| e.to_string())
    }
    pub fn restore(json: &str) -> Result<Self, String> {
        if json.len() > 8 * 1024 * 1024 {
            return Err("Demo state exceeds size limit".into());
        }
        let saved: SavedOwner =
            serde_json::from_str(json).map_err(|e| format!("Cannot restore demo state: {e}"))?;
        if saved.version != 1 || saved.threads.len() > 1000 {
            return Err("Unsupported demo state version or size".into());
        }
        crate::mail::validate(&saved.threads)?;
        if saved
            .threads
            .iter()
            .any(|t| !ACCOUNTS.contains(&t.id.account.as_str()))
        {
            return Err("Unknown saved demo account".into());
        }
        let keys: std::collections::HashSet<_> = saved.threads.iter().map(|t| &t.id).collect();
        let mut seen = std::collections::HashSet::new();
        if saved
            .drafts
            .iter()
            .any(|(k, v)| !keys.contains(k) || !seen.insert(k) || v.len() > 64 * 1024)
        {
            return Err("Invalid saved draft identity or size".into());
        }
        seen.clear();
        if saved
            .snoozes
            .iter()
            .any(|(k, _)| !keys.contains(k) || !seen.insert(k))
            || saved
                .threads
                .iter()
                .any(|t| t.snoozed != saved.snoozes.iter().any(|(k, _)| k == &t.id))
        {
            return Err("Invalid saved snooze identity".into());
        }
        Ok(Self {
            threads: saved.threads,
            drafts: saved.drafts.into_iter().collect(),
            snoozes: saved.snoozes.into_iter().collect(),
            now: saved.now,
            history: Vec::new(),
        })
    }
}

pub fn work() -> Snapshot {
    let rows = [
        (
            "Canvas mail workspace",
            "Shared views",
            Status::Active,
            1,
            "Avery",
        ),
        (
            "Review rear mounting clearance",
            "Enclosure prototype",
            Status::Review,
            1,
            "Mira",
        ),
        (
            "Aggregate Edgerunner and Alfalfa context",
            "Coding",
            Status::Active,
            2,
            "Avery",
        ),
        (
            "Map paired messaging accounts",
            "ohmOS",
            Status::Open,
            2,
            "Rowan",
        ),
        (
            "Calendar timezone and all-day checks",
            "Calendar",
            Status::Done,
            2,
            "Avery",
        ),
        (
            "Read-only owner access",
            "Workspace",
            Status::Blocked,
            1,
            "Avery",
        ),
        (
            "Compact command palette",
            "Canvas",
            Status::Open,
            2,
            "Jonas",
        ),
        (
            "Attachment owner references",
            "Storage",
            Status::Deferred,
            3,
            "Avery",
        ),
    ];
    Snapshot{revision:1,items:rows.iter().enumerate().map(|(i,(title,project,status,priority,owner))|Item{
        id:format!("demo-work-{i}"),title:(*title).into(),status:*status,priority:*priority,project:(*project).into(),owner:(*owner).into(),updated:"Today".into(),
        description:"Synthetic work item for interaction and layout review. The real owner remains responsible for claims, status and evidence.".into(),source:Some("Demo source reference".into()),evidence:vec!["Fixture evidence · no live state".into()],dependencies:vec![]}).collect(),
        fresh:true,source:"Marbles · demo".into(),observed:"Fixture".into(),notice:None}
}
pub fn calendar() -> Result<canvas_calendar::Snapshot, String> {
    let rows = [
        ("Design review", "10:00:00", "10:45:00"),
        ("Enclosure fit check", "13:00:00", "13:30:00"),
        ("Workspace UI pass", "15:00:00", "16:00:00"),
    ];
    let events:Vec<_>=rows.iter().enumerate().map(|(i,(title,start,end))|serde_json::json!({"id":format!("demo-{i}"),"summary":title,"status":"confirmed",
        "start":{"dateTime":format!("2026-10-19T{start}-07:00"),"timeZone":"America/Los_Angeles"},
        "end":{"dateTime":format!("2026-10-19T{end}-07:00"),"timeZone":"America/Los_Angeles"},"location":"Demo meeting"})).collect();
    canvas_calendar::Snapshot::from_bridge(
        &serde_json::json!({"schema_version":1,"operation":"events","account":{"connection_id":"demo-google","email":ACCOUNTS[0]},"calendar_id":"demo-calendar","data":{"items":events}}),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triage_and_undo_never_cross_account_identity() {
        let mut owner = Owner::new();
        let original = owner.threads[0].clone();
        let mut duplicate = original.clone();
        duplicate.id.account = ACCOUNTS[1].into();
        owner.threads.push(duplicate);
        owner.apply(Intent::Archive(original.id.clone())).unwrap();
        assert!(owner.threads[0].archived);
        assert!(!owner.threads.last().unwrap().archived);
        owner.apply(Intent::Undo).unwrap();
        assert!(!owner.threads[0].archived);
        owner
            .apply(Intent::SavePreview {
                key: original.id.clone(),
                body: "Thanks for the section.".into(),
            })
            .unwrap();
        assert_eq!(owner.drafts.len(), 1);
        assert!(owner.drafts.contains_key(&original.id));
    }
    #[test]
    fn fixtures_are_deterministic_and_scoped() {
        let a = Owner::new();
        let b = Owner::new();
        crate::mail::validate(&a.threads).unwrap();
        assert_eq!(
            serde_json::to_value(a.threads).unwrap(),
            serde_json::to_value(b.threads).unwrap()
        );
        assert_eq!(calendar().unwrap().events.len(), 3);
        assert!(work().validate().is_ok());
    }
    #[test]
    fn composing_preserves_explicit_from_and_to_without_sending() {
        let mut owner = Owner::new();
        owner
            .apply(Intent::ComposePreview {
                account: ACCOUNTS[1].into(),
                to: "mira@example.com".into(),
                subject: "Review".into(),
                body: "Looks good.".into(),
            })
            .unwrap();
        let draft = &owner.threads[0];
        assert_eq!(draft.id.account, ACCOUNTS[1]);
        assert_eq!(draft.messages[0].to, "mira@example.com");
        assert!(owner.drafts.contains_key(&draft.id));
        assert!(draft.archived);
        owner.apply(Intent::Undo).unwrap();
        assert_eq!(owner.threads.len(), 48);
        assert!(owner.drafts.is_empty());
        assert!(
            owner
                .apply(Intent::ComposePreview {
                    account: "unknown@example.com".into(),
                    to: "mira@example.com".into(),
                    subject: "Review".into(),
                    body: "Looks good.".into()
                })
                .is_err()
        );
    }
}

#[cfg(test)]
mod triage_tests {
    use super::*;
    #[test]
    fn clock_ticks_do_not_rewrite_an_unchanged_checkpoint() {
        let mut owner = Owner::new();
        owner.advance_to(1000);
        let before = owner.save().unwrap();
        owner.advance_to(1001);
        assert_eq!(owner.save().unwrap(), before);
        owner
            .apply(Intent::Snooze(owner.threads[0].id.clone()))
            .unwrap();
        let snoozed = owner.save().unwrap();
        assert_ne!(snoozed, before);
        owner.advance_to(1002);
        assert_eq!(owner.save().unwrap(), snoozed);
    }
    #[test]
    fn bulk_read_counts_and_reply_autosave_keep_triage_undo() {
        let mut owner = Owner::new();
        let key = owner.threads[0].id.clone();
        owner
            .apply(Intent::Bulk {
                keys: vec![key.clone()],
                action: Action::Read,
            })
            .unwrap();
        assert!(!owner.threads[0].unread);
        owner.apply(Intent::Undo).unwrap();
        assert!(owner.threads[0].unread);
        owner.apply(Intent::Archive(key.clone())).unwrap();
        owner
            .apply(Intent::EditPreview {
                key: key.clone(),
                body: "Autosaved reply".into(),
            })
            .unwrap();
        assert_eq!(owner.drafts[&key], "Autosaved reply");
        owner.apply(Intent::Undo).unwrap();
        assert!(!owner.threads[0].archived);
        assert_eq!(owner.drafts[&key], "Autosaved reply");
    }
    #[test]
    fn bulk_is_atomic_and_undoes_as_one_operation() {
        let mut owner = Owner::new();
        let keys = vec![owner.threads[0].id.clone(), owner.threads[8].id.clone()];
        let mut invalid = keys.clone();
        invalid.push(KeyId {
            account: ACCOUNTS[0].into(),
            thread: "missing".into(),
        });
        assert!(
            owner
                .apply(Intent::Bulk {
                    keys: invalid,
                    action: Action::Archive
                })
                .is_err()
        );
        assert!(owner.threads.iter().all(|t| !t.archived));
        owner
            .apply(Intent::Bulk {
                keys: keys.clone(),
                action: Action::Archive,
            })
            .unwrap();
        assert_eq!(owner.threads.iter().filter(|t| t.archived).count(), 2);
        // Reading another thread must not consume the bulk undo slot.
        owner
            .apply(Intent::Open(owner.threads[2].id.clone()))
            .unwrap();
        owner.apply(Intent::Undo).unwrap();
        assert!(owner.threads.iter().all(|t| !t.archived));
        assert!(
            owner
                .apply(Intent::Bulk {
                    keys: vec![keys[0].clone(), keys[0].clone()],
                    action: Action::Archive
                })
                .is_err()
        );
    }
    #[test]
    fn persisted_snoozes_return_once_after_restart_and_drafts_keep_identity() {
        let mut owner = Owner::new();
        owner.advance_to(1000);
        let key = owner.threads[0].id.clone();
        owner.apply(Intent::Snooze(key.clone())).unwrap();
        owner
            .apply(Intent::SavePreview {
                key: key.clone(),
                body: "Draft survives restart".into(),
            })
            .unwrap();
        let mut restored = Owner::restore(&owner.save().unwrap()).unwrap();
        assert_eq!(restored.advance_to(87399), 0);
        assert!(restored.threads[0].snoozed);
        assert_eq!(restored.advance_to(87400), 1);
        assert_eq!(restored.advance_to(87401), 0);
        assert!(!restored.threads[0].snoozed);
        assert!(!restored.threads[0].archived);
        assert_eq!(restored.drafts[&key], "Draft survives restart");
        let mut invalid: serde_json::Value = serde_json::from_str(&owner.save().unwrap()).unwrap();
        invalid["version"] = 999.into();
        assert!(Owner::restore(&invalid.to_string()).is_err());
        assert!(Owner::restore("{broken").is_err());
        invalid["version"] = 1.into();
        invalid["snoozes"] = serde_json::json!([]);
        assert!(Owner::restore(&invalid.to_string()).is_err());
    }
}
