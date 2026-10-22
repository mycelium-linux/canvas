//! Pure Workbench composition for web and Canvas consumers.
//!
//! Artifact identity belongs to AlfAlpha; iteration belongs to the tool owner.
//! This projection neither authenticates, resolves CIDs, executes tools nor
//! invents readiness. Owner adapters must enforce identity and billing before
//! accepting its existing [`IterationRequest`] handoff.

use crate::part_inspector::{IterationRequest, PartProject};
use crate::project_browser::{Error, ProjectManifest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputState<T> {
    Unavailable {
        reason: String,
    },
    Observed {
        source: String,
        observed_at_ms: u64,
        value: T,
    },
}

/// Independent panes consume owner facts, not the availability of another pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workbench {
    pub artifacts: InputState<PartProject>,
    pub assistant: InputState<AssistantStatus>,
    pub evidence: InputState<RunEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantStatus {
    pub session_id: String,
    pub model: String,
    /// Actual owner state, rather than motion suggesting a running generation.
    pub state: AssistantState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantState {
    Idle,
    Running,
    Waiting,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEvidence {
    pub run_id: String,
    pub project_id: String,
    pub manifest_revision: String,
    /// Missing duration/cost is unknown, not zero.
    pub elapsed_ms: Option<u64>,
    pub cost_microunits: Option<u64>,
    pub report_artifact_id: Option<String>,
}

impl Workbench {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            artifacts: InputState::Unavailable {
                reason: reason.into(),
            },
            assistant: InputState::Unavailable {
                reason: "Assistant is not connected".into(),
            },
            evidence: InputState::Unavailable {
                reason: "No run evidence received".into(),
            },
        }
    }

    pub fn with_manifest(
        manifest: &ProjectManifest,
        source: &str,
        observed_at_ms: u64,
    ) -> Result<Self, Error> {
        if source.trim().is_empty() || observed_at_ms == 0 {
            return Err(Error(
                "manifest observation requires source and timestamp".into(),
            ));
        }
        let mut view = Self::unavailable("Manifest unavailable");
        view.artifacts = InputState::Observed {
            source: source.into(),
            observed_at_ms,
            value: manifest.inspect_parts()?,
        };
        Ok(view)
    }

    /// Reject stale tabs and cross-project selection before producing the
    /// existing immutable owner handoff. This is not an authorization grant.
    pub fn iteration(
        &self,
        manifest: &ProjectManifest,
        part_id: &str,
        prompt: &str,
        selected: &[String],
    ) -> Result<IterationRequest, Error> {
        let InputState::Observed { value, .. } = &self.artifacts else {
            return Err(Error("No observed manifest for iteration".into()));
        };
        if value.project_id != manifest.project_id || value.manifest_revision != manifest.revision {
            return Err(Error(
                "Project manifest changed; refresh before iterating".into(),
            ));
        }
        let current = manifest.inspect_parts()?;
        let part = current
            .parts
            .iter()
            .find(|part| part.id == part_id)
            .ok_or_else(|| Error("Selected part does not belong to this manifest".into()))?;
        manifest.iteration_request(part, prompt, selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_browser::ManifestEntry;

    fn manifest() -> ProjectManifest {
        ProjectManifest {
            schema_version: 1,
            project_id: "pedal".into(),
            revision: "rev-1".into(),
            entries: vec![ManifestEntry {
                id: "model".into(),
                path: "parts/enclosure/body.glb".into(),
                display_name: "Enclosure".into(),
                cid: "bafy-model".into(),
                media_type: "model/gltf-binary".into(),
                bytes: 123,
                modified_at_ms: None,
            }],
        }
    }

    #[test]
    fn artifacts_do_not_claim_assistant_or_run_connectivity() {
        let view = Workbench::with_manifest(&manifest(), "alfalfa:pedal", 123).unwrap();
        assert!(matches!(view.artifacts, InputState::Observed { .. }));
        assert!(matches!(view.assistant, InputState::Unavailable { .. }));
        assert!(matches!(view.evidence, InputState::Unavailable { .. }));
        assert!(Workbench::with_manifest(&manifest(), "", 123).is_err());
        assert!(Workbench::with_manifest(&manifest(), "owner", 0).is_err());
    }

    #[test]
    fn iteration_reuses_owner_contract_and_rejects_stale_or_foreign_input() {
        let mut manifest = manifest();
        let view = Workbench::with_manifest(&manifest, "alfalfa:pedal", 123).unwrap();
        let request = view
            .iteration(
                &manifest,
                "parts/enclosure",
                "Add mounting holes",
                &["model".into()],
            )
            .unwrap();
        assert_eq!(request.artifacts[0].cid, "bafy-model");
        assert!(
            view.iteration(&manifest, "parts/enclosure", "Change", &["foreign".into()])
                .is_err()
        );
        manifest.revision = "rev-2".into();
        assert!(
            view.iteration(&manifest, "parts/enclosure", "Change", &["model".into()])
                .is_err()
        );
        manifest.revision = "rev-1".into();
        manifest.project_id = "other".into();
        assert!(
            view.iteration(&manifest, "parts/enclosure", "Change", &["model".into()])
                .is_err()
        );
    }
}
