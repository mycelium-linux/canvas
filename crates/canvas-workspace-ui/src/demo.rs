//! Deterministic synthetic owner. It cannot call Gmail, Calendar or a message transport.
use crate::{
    Item, Snapshot, Status,
    mail::{Attachment, Intent, KeyId, Message, Thread},
};
use std::collections::HashMap;

pub const ACCOUNTS: [&str; 3] = ["avery@fpl.dev", "avery@wagar.cc", "ajmw.subs@gmail.com"];
pub struct Owner {
    pub threads: Vec<Thread>,
    pub drafts: HashMap<KeyId, String>,
    history: Vec<Undo>,
}
struct Undo {
    key: KeyId,
    thread: Option<Thread>,
    draft: Option<String>,
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
        }
    }
    pub fn apply(&mut self, intent: Intent) -> Result<String, String> {
        if let Intent::ComposePreview {
            account,
            to,
            subject,
            body,
        } = &intent
        {
            if !ACCOUNTS.contains(&account.as_str())
                || to.len() > 320
                || subject.len() > 512
                || body.trim().is_empty()
                || body.len() > 64 * 1024
            {
                return Err("Invalid demo draft identity or content".into());
            }
            let key = KeyId {
                account: account.clone(),
                thread: format!("demo-compose-{}", self.threads.len()),
            };
            if self.history.len() >= 32 {
                self.history.remove(0);
            }
            self.history.push(Undo {
                key: key.clone(),
                thread: None,
                draft: None,
            });
            self.threads.insert(
                0,
                Thread {
                    id: key.clone(),
                    sender: "Draft".into(),
                    subject: if subject.is_empty() {
                        "(No subject)".into()
                    } else {
                        subject.clone()
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
            self.drafts.insert(key, body.clone());
            return Ok(format!("New draft preview saved for {account}"));
        }
        if let Intent::Undo = intent {
            let Some(old) = self.history.pop() else {
                return Ok("Nothing to undo".into());
            };
            if let Some(thread) = old.thread {
                let row = self
                    .threads
                    .iter_mut()
                    .find(|t| t.id == old.key)
                    .ok_or("Conversation no longer exists")?;
                *row = thread;
            } else {
                self.threads.retain(|t| t.id != old.key);
            }
            if let Some(draft) = old.draft {
                self.drafts.insert(old.key, draft);
            } else {
                self.drafts.remove(&old.key);
            }
            return Ok("Change undone".into());
        }
        let key = match &intent {
            Intent::Archive(k)
            | Intent::Star(k)
            | Intent::Unread(k)
            | Intent::Snooze(k)
            | Intent::Open(k) => k,
            Intent::SavePreview { key, .. } => key,
            Intent::Undo => return Err("Invalid undo state".into()),
            Intent::ComposePreview { .. } => return Err("Invalid draft state".into()),
        };
        let row = self
            .threads
            .iter_mut()
            .find(|t| &t.id == key)
            .ok_or("Unknown account / conversation identity")?;
        if let Intent::SavePreview { body, .. } = &intent {
            if body.trim().is_empty() || body.len() > 64 * 1024 {
                return Err("Draft preview is empty or too large".into());
            }
            if self.history.len() >= 32 {
                self.history.remove(0);
            }
            self.history.push(Undo {
                key: key.clone(),
                thread: Some(row.clone()),
                draft: self.drafts.get(key).cloned(),
            });
            self.drafts.insert(key.clone(), body.clone());
            return Ok(format!("Draft preview saved for {}", key.account));
        }
        if self.history.len() >= 32 {
            self.history.remove(0);
        }
        self.history.push(Undo {
            key: key.clone(),
            thread: Some(row.clone()),
            draft: self.drafts.get(key).cloned(),
        });
        let notice = match intent {
            Intent::Archive(_) => {
                row.archived = true;
                "Archived in demo"
            }
            Intent::Star(_) => {
                row.starred = !row.starred;
                "Star updated in demo"
            }
            Intent::Unread(_) => {
                row.unread = true;
                "Marked unread in demo"
            }
            Intent::Snooze(_) => {
                row.snoozed = true;
                "Snoozed until tomorrow in demo"
            }
            Intent::Open(_) => {
                row.unread = false;
                "Conversation opened"
            }
            _ => return Err("Unsupported demo action".into()),
        };
        Ok(notice.into())
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
