//! Carries prompts typed on the wall to the agent control planes that can
//! answer them, and carries their sessions back as a roster.
//!
//! Canvas publishes `canvas.prompt.v1` naming a *target* ("operator", "surf")
//! and never an address, so something has to resolve a name to a control plane.
//! That resolution, and the credentials it needs, live here rather than in the
//! display: a wall that held an mTLS identity for every tool would be a client
//! of all of them, and the secrets would have to be copied to every screen.
//!
//! The obvious cost of a single bridge is that it knows about every tool. That
//! is contained by keying adapters on a *protocol kind* in configuration
//! instead of on the target name: several targets can share one kind, so adding
//! an agent is a config entry, and only a genuinely new protocol is new code.

use serde::{Deserialize, Serialize};

pub mod edgerunner;
pub mod roster;
pub mod surf;

/// Protocols this bridge can speak.
///
/// Deliberately small. A new variant means a control plane whose verbs differ,
/// which is the only case that cannot be expressed as configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    /// Edgerunner: ACP underneath, reached over its REST surface.
    Edgerunner,
    /// Surf: a browsing agent driven by observe/execute.
    Surf,
}

/// One target a prompt surface may name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The name Canvas uses. Matched exactly against a prompt's `target`.
    pub name: String,
    pub kind: AdapterKind,
    pub base_url: String,
    /// Environment variable holding the bearer token.
    ///
    /// A variable name rather than the token itself, so a config file that
    /// leaks is not a credential that leaks.
    #[serde(default)]
    pub token_env: Option<String>,
    /// Client identity for control planes that require mTLS, as a PEM path.
    #[serde(default)]
    pub client_identity: Option<String>,
    /// Narrow a session listing to one project, when the control plane
    /// supports it.
    #[serde(default)]
    pub project: Option<String>,
}

/// Read, never written: `BusConfig` is deserialize-only upstream, and a config
/// this process cannot serialize is a config it cannot accidentally emit
/// somewhere with credentials resolved into it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    pub bus: canvas::bus::BusConfig,
    pub targets: Vec<Target>,
    /// How often session rosters are refreshed, in milliseconds.
    #[serde(default = "default_roster_interval_ms")]
    pub roster_interval_ms: u64,
}

fn default_roster_interval_ms() -> u64 {
    5_000
}

impl BridgeConfig {
    pub fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|error| anyhow::anyhow!("read bridge config {}: {error}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("parse bridge config {}: {error}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.targets.is_empty(), "bridge has no targets");
        let mut seen = std::collections::BTreeSet::new();
        for target in &self.targets {
            anyhow::ensure!(
                !target.name.trim().is_empty(),
                "a target has an empty name"
            );
            anyhow::ensure!(
                seen.insert(target.name.as_str()),
                "two targets are both named {:?}; a prompt could not say which it meant",
                target.name
            );
            anyhow::ensure!(
                target.base_url.starts_with("http://") || target.base_url.starts_with("https://"),
                "target {:?} needs an http(s) base_url, got {:?}",
                target.name,
                target.base_url
            );
        }
        Ok(())
    }

    /// The target a prompt names, or `None`.
    ///
    /// An unknown target is a configuration mistake, not something to guess at:
    /// silently routing to a default would send someone's words to an agent
    /// they did not address.
    pub fn target(&self, name: &str) -> Option<&Target> {
        self.targets.iter().find(|target| target.name == name)
    }
}

/// A prompt as it arrives from Canvas.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Prompt {
    pub target: String,
    pub text: String,
    /// Pinned session, or `None` to use whichever the target considers current.
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

impl Prompt {
    /// Parse a prompt body, rejecting anything that is not actionable.
    pub fn parse(body: &serde_json::Value) -> anyhow::Result<Self> {
        let prompt: Self = serde_json::from_value(body.clone())
            .map_err(|error| anyhow::anyhow!("unusable prompt: {error}"))?;
        anyhow::ensure!(!prompt.target.trim().is_empty(), "prompt has no target");
        anyhow::ensure!(!prompt.text.trim().is_empty(), "prompt has no text");
        Ok(prompt)
    }
}

/// A session as this bridge understands it, whatever control plane it came from.
///
/// The two control planes describe sessions differently; normalising here is
/// what lets one roster widget show both without the wall learning either
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub state: SessionState,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting on a person — an approval, a permission prompt.
    Blocked,
    Working,
    Idle,
    /// Finished or failed. Never a prompt destination.
    Ended,
}

impl SessionState {
    /// Whether a prompt can still be sent here.
    pub fn accepts_prompts(self) -> bool {
        !matches!(self, Self::Ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(targets: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "bus": {
                "installation_id": "home",
                "producer_id": "agent-bridge",
                "hub_endpoint": "127.0.0.1:18790",
                "token_file": "/dev/null",
                "zones": ["living-room"],
                "capabilities": [],
                "screen_anchors": [],
                "schema_version": 1
            },
            "targets": targets
        })
    }

    fn parse(value: serde_json::Value) -> anyhow::Result<BridgeConfig> {
        let config: BridgeConfig = serde_json::from_value(value)?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn several_targets_may_share_one_adapter_kind() {
        let config = parse(config(serde_json::json!([
            {"name": "operator", "kind": "edgerunner", "base_url": "https://edgerunner.fpl.dev"},
            {"name": "fab", "kind": "edgerunner", "base_url": "https://edgerunner.fpl.dev"},
            {"name": "surf", "kind": "surf", "base_url": "http://127.0.0.1:8081"}
        ])))
        .expect("valid");

        // The point of keying on kind: a third edgerunner-backed agent is a
        // config entry, not a code change.
        assert_eq!(config.target("fab").unwrap().kind, AdapterKind::Edgerunner);
        assert_eq!(config.target("surf").unwrap().kind, AdapterKind::Surf);
    }

    /// An unaddressed prompt must not be delivered to a default: it would send
    /// someone's words to an agent they did not name.
    #[test]
    fn an_unknown_target_does_not_resolve() {
        let config = parse(config(serde_json::json!([
            {"name": "operator", "kind": "edgerunner", "base_url": "https://edgerunner.fpl.dev"}
        ])))
        .expect("valid");
        assert!(config.target("transmog").is_none());
    }

    #[test]
    fn duplicate_target_names_are_refused() {
        let error = parse(config(serde_json::json!([
            {"name": "operator", "kind": "edgerunner", "base_url": "https://a.example"},
            {"name": "operator", "kind": "surf", "base_url": "http://b.example"}
        ])))
        .expect_err("ambiguous");
        assert!(error.to_string().contains("both named"), "{error}");
    }

    #[test]
    fn a_base_url_without_a_scheme_is_refused() {
        let error = parse(config(serde_json::json!([
            {"name": "operator", "kind": "edgerunner", "base_url": "edgerunner.fpl.dev"}
        ])))
        .expect_err("no scheme");
        assert!(error.to_string().contains("base_url"), "{error}");
    }

    #[test]
    fn a_token_is_referenced_by_variable_never_written_in_the_file() {
        let config = parse(config(serde_json::json!([
            {"name": "operator", "kind": "edgerunner", "base_url": "https://a.example",
             "token_env": "EDGERUNNER_TOKEN"}
        ])))
        .expect("valid");
        assert_eq!(
            config.target("operator").unwrap().token_env.as_deref(),
            Some("EDGERUNNER_TOKEN")
        );
        // There is deliberately no field that could hold a literal token.
        let rendered = serde_json::to_string(config.target("operator").unwrap()).unwrap();
        assert!(!rendered.contains("token\":"), "{rendered}");
    }

    #[test]
    fn a_prompt_needs_a_target_and_something_to_say() {
        let good = Prompt::parse(&serde_json::json!({"target": "operator", "text": "status?"}))
            .expect("usable");
        assert_eq!(good.target, "operator");
        assert_eq!(good.session, None, "unpinned by default");

        assert!(Prompt::parse(&serde_json::json!({"target": "operator", "text": "  "})).is_err());
        assert!(Prompt::parse(&serde_json::json!({"text": "orphan"})).is_err());
        assert!(Prompt::parse(&serde_json::json!({"target": "", "text": "x"})).is_err());
    }

    #[test]
    fn a_prompt_may_pin_a_session() {
        let prompt = Prompt::parse(&serde_json::json!({
            "target": "operator", "text": "carry on", "session": "ses_9f2"
        }))
        .expect("usable");
        assert_eq!(prompt.session.as_deref(), Some("ses_9f2"));
    }

    #[test]
    fn an_ended_session_takes_no_prompts() {
        assert!(!SessionState::Ended.accepts_prompts());
        for state in [
            SessionState::Blocked,
            SessionState::Working,
            SessionState::Idle,
        ] {
            assert!(state.accepts_prompts(), "{state:?} should still accept");
        }
    }
}
