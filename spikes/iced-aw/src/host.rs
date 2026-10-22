//! Host-mediated actions.
//!
//! The rule Canvas needs to hold: **a widget never performs an action, and a
//! payload arriving over a stream can never cause one.** Both plain iced and
//! iced_aw hand you the same escape hatch — `.on_press(SomeMessage)` — so
//! neither widget set enforces this. It has to come from the message type.
//!
//! The shape here makes it a compile-time property rather than a convention:
//!
//! * [`Intent`] is what a widget may *ask* for. Pure data. Anyone can build
//!   one — a button, a Lua adapter, an SSE frame. Building one grants nothing.
//! * [`Action`] is what the host has *decided to do*. It carries a private
//!   [`Seal`], so no module outside `host` can construct one, even by writing
//!   `Action { .. }`. There is exactly one way to obtain an `Action`:
//!   [`Host::authorize`].
//! * [`Origin`] records who asked. The gate denies actuating intents that
//!   arrived from a stream, which is the property the widget set cannot give us.

use std::fmt;

/// What a widget may ask the host to do. Deliberately inert: cloneable, cheap,
/// and constructible from anywhere, including untrusted input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Approve a routed approval (bead dock-8n5).
    Approve { approval_id: String },
    /// Deny a routed approval.
    Deny { approval_id: String },
    /// Open an RSS item in the browser (dock-4s7).
    OpenUrl { url: String },
    /// Jump to a project workspace (dock-aqz).
    JumpToProject { project_id: String },
    /// Persist a note.
    SaveNote { body: String },
    /// Presentational only: change the shell's own density.
    SetDensity(Density),
}

impl Intent {
    /// Whether honoring this intent reaches outside Canvas's own presentation.
    /// Presentational intents are safe for any origin; actuating ones are not.
    pub fn is_actuating(&self) -> bool {
        match self {
            Self::Approve { .. }
            | Self::Deny { .. }
            | Self::OpenUrl { .. }
            | Self::JumpToProject { .. }
            | Self::SaveNote { .. } => true,
            Self::SetDensity(_) => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Approve { approval_id } => format!("approve {approval_id}"),
            Self::Deny { approval_id } => format!("deny {approval_id}"),
            Self::OpenUrl { url } => format!("open {url}"),
            Self::JumpToProject { project_id } => format!("jump {project_id}"),
            Self::SaveNote { body } => format!("note ({} chars)", body.chars().count()),
            Self::SetDensity(d) => format!("density {d}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    pub const ALL: [Density; 2] = [Density::Comfortable, Density::Compact];
}

impl fmt::Display for Density {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Comfortable => "Comfortable",
            Self::Compact => "Compact",
        })
    }
}

/// Where an intent came from. The host cannot infer this after the fact, so the
/// message carries it and only the view layer that is wired to a real pointer
/// event is allowed to stamp [`Origin::LocalPointer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A human pressed something in the shell.
    LocalPointer,
    /// The intent was carried inside a data payload — an SSE frame, a Lua
    /// adapter's event batch, an MCP call. Untrusted by construction.
    Stream { adapter: &'static str },
}

/// Proof that [`Host::authorize`] ran. Private field, private type: no code
/// outside this module can name `Seal`, so no code outside this module can
/// build an `Action`.
#[derive(Debug)]
struct Seal;

/// A decision the host has made. The only thing the effect layer will execute.
#[derive(Debug)]
pub struct Action {
    intent: Intent,
    #[allow(dead_code)]
    seal: Seal,
}

impl Action {
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// An actuating intent arrived inside a data payload.
    StreamOriginated,
    /// The control gate is closed — a human has taken over this session.
    GateRevoked,
    /// The approval id is not one the host is currently routing.
    UnknownApproval,
    /// Nothing to save.
    EmptyNote,
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StreamOriginated => "stream-originated actuating intent",
            Self::GateRevoked => "control gate revoked",
            Self::UnknownApproval => "unknown approval id",
            Self::EmptyNote => "empty note",
        })
    }
}

/// The single choke point. Every intent from every widget goes through here.
pub struct Host {
    /// Approvals the host is currently routing. An `Approve` for anything else
    /// is refused rather than quietly ignored (fail loud).
    pub open_approvals: Vec<String>,
    /// Mirrors the backend control gate: revoked on human takeover.
    pub gate_open: bool,
    /// Every verdict, in order. This is what makes the rule auditable instead
    /// of merely asserted.
    pub audit: Vec<AuditEntry>,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub origin: Origin,
    pub intent: String,
    pub verdict: Result<(), Denied>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            open_approvals: vec!["appr-7301".into()],
            gate_open: true,
            audit: Vec::new(),
        }
    }
}

impl Host {
    /// The one way to turn an [`Intent`] into an [`Action`].
    pub fn authorize(&mut self, intent: Intent, origin: Origin) -> Result<Action, Denied> {
        let verdict = self.adjudicate(&intent, &origin);
        self.audit.push(AuditEntry {
            origin,
            intent: intent.label(),
            verdict: verdict.map(|()| ()),
        });
        verdict?;
        Ok(Action {
            intent,
            seal: Seal,
        })
    }

    fn adjudicate(&self, intent: &Intent, origin: &Origin) -> Result<(), Denied> {
        // The rule that matters: a payload cannot actuate. Checked before
        // anything else so no later branch can accidentally let one through.
        if matches!(origin, Origin::Stream { .. }) && intent.is_actuating() {
            return Err(Denied::StreamOriginated);
        }
        if !self.gate_open && intent.is_actuating() {
            return Err(Denied::GateRevoked);
        }
        match intent {
            Intent::Approve { approval_id } | Intent::Deny { approval_id } => {
                if self.open_approvals.iter().any(|id| id == approval_id) {
                    Ok(())
                } else {
                    Err(Denied::UnknownApproval)
                }
            }
            Intent::SaveNote { body } if body.trim().is_empty() => Err(Denied::EmptyNote),
            _ => Ok(()),
        }
    }

    /// The effect layer. Takes an `Action`, which it cannot have forged.
    pub fn perform(&mut self, action: Action) -> String {
        match action.intent() {
            Intent::Approve { approval_id } => {
                self.open_approvals.retain(|id| id != approval_id);
                format!("routed APPROVE for {approval_id}")
            }
            Intent::Deny { approval_id } => {
                self.open_approvals.retain(|id| id != approval_id);
                format!("routed DENY for {approval_id}")
            }
            Intent::OpenUrl { url } => format!("would open {url}"),
            Intent::JumpToProject { project_id } => format!("would activate workspace {project_id}"),
            Intent::SaveNote { body } => format!("saved note: {body}"),
            Intent::SetDensity(d) => format!("density is now {d}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_press_can_approve() {
        let mut host = Host::default();
        let action = host
            .authorize(
                Intent::Approve {
                    approval_id: "appr-7301".into(),
                },
                Origin::LocalPointer,
            )
            .expect("a human press on a routed approval is authorized");
        assert!(host.perform(action).contains("APPROVE"));
        assert!(host.open_approvals.is_empty());
    }

    /// The property the whole spike exists to demonstrate. An identical
    /// `Intent` — byte-for-byte the same value the button produces — is refused
    /// when it arrives inside a payload.
    #[test]
    fn a_stream_payload_cannot_approve() {
        let mut host = Host::default();
        let intent = Intent::Approve {
            approval_id: "appr-7301".into(),
        };
        let from_button = intent.clone();

        let denied = host.authorize(intent, Origin::Stream { adapter: "rss.lua" });
        assert_eq!(denied.err(), Some(Denied::StreamOriginated));
        assert_eq!(host.open_approvals, vec!["appr-7301".to_string()]);

        // Same value, human origin: allowed. So it is the origin doing the
        // work, not some difference in the payload the adapter could imitate.
        assert!(host.authorize(from_button, Origin::LocalPointer).is_ok());
    }

    #[test]
    fn a_stream_payload_cannot_open_a_url_or_jump() {
        let mut host = Host::default();
        for intent in [
            Intent::OpenUrl {
                url: "file:///etc/passwd".into(),
            },
            Intent::JumpToProject {
                project_id: "Studio".into(),
            },
            Intent::SaveNote { body: "x".into() },
        ] {
            assert_eq!(
                host.authorize(intent, Origin::Stream { adapter: "sse" })
                    .err(),
                Some(Denied::StreamOriginated)
            );
        }
    }

    /// Presentational intents are the one thing a payload may do, and the
    /// distinction is a method on `Intent` rather than scattered `match` arms.
    #[test]
    fn a_stream_payload_may_still_change_presentation() {
        let mut host = Host::default();
        assert!(
            host.authorize(
                Intent::SetDensity(Density::Compact),
                Origin::Stream { adapter: "rss.lua" },
            )
            .is_ok()
        );
    }

    #[test]
    fn a_revoked_gate_stops_a_human_too() {
        let mut host = Host::default();
        host.gate_open = false;
        assert_eq!(
            host.authorize(
                Intent::Approve {
                    approval_id: "appr-7301".into()
                },
                Origin::LocalPointer,
            )
            .err(),
            Some(Denied::GateRevoked)
        );
    }

    #[test]
    fn an_unrouted_approval_is_refused_loudly_not_ignored() {
        let mut host = Host::default();
        assert_eq!(
            host.authorize(
                Intent::Approve {
                    approval_id: "appr-0000".into()
                },
                Origin::LocalPointer,
            )
            .err(),
            Some(Denied::UnknownApproval)
        );
    }

    #[test]
    fn every_verdict_is_audited() {
        let mut host = Host::default();
        let _ = host.authorize(
            Intent::Approve {
                approval_id: "appr-7301".into(),
            },
            Origin::Stream { adapter: "rss.lua" },
        );
        let _ = host.authorize(
            Intent::Approve {
                approval_id: "appr-7301".into(),
            },
            Origin::LocalPointer,
        );
        assert_eq!(host.audit.len(), 2);
        assert!(host.audit[0].verdict.is_err());
        assert!(host.audit[1].verdict.is_ok());
    }
}
