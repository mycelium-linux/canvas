//! Shared visual state observed and changed by both humans and agents.
//!
//! Sources own data and transport. Canvas owns the bounded, revisioned view of
//! that data: viewport, attention, navigation, and presentation parameters.

use super::{Error, Result, SCHEMA_VERSION, check, contract, identifier, visible};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_VIEW_PARAMETERS: usize = 64;
const MAX_PARAMETER_VALUES: usize = 256;
const MAX_VIEW_OPERATIONS: usize = 128;
const MAX_VIEW_SOURCES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum VisualSourceKind {
    Artifact,
    Stream,
}

/// Stable identity and schema for data owned outside Canvas.
///
/// Endpoints, credentials, decoded frames, and transport details never enter
/// this reference. `owner` resolves those details at the trusted host boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct VisualSourceRef {
    pub owner: String,
    pub id: String,
    pub contract: String,
    pub kind: VisualSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum SourcePosition {
    /// Exact immutable revision of a finite artifact or snapshot.
    Revision { revision: u64 },
    /// Follow a live source while recording the last sequence Canvas observed.
    Live { observed_sequence: u64 },
    /// Hold a live source at an exact replayable sequence.
    Sequence { sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ViewSource {
    pub source: VisualSourceRef,
    pub position: SourcePosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ViewRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for ViewRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum AttentionActorKind {
    Human,
    Agent,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ViewAttention {
    pub actor_kind: AttentionActorKind,
    pub actor_id: String,
    pub region: ViewRect,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ViewNavigation {
    #[default]
    None,
    Page {
        index: u32,
    },
    Timeline {
        position_ms: u64,
        playing: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ViewParameterValue {
    Boolean(bool),
    Number(f64),
    Text(String),
    Range { minimum: f64, maximum: f64 },
    Set(Vec<String>),
}

/// The exact visual context shared between Canvas, a person, and an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SharedViewDocument {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub sources: BTreeMap<String, ViewSource>,
    #[serde(default)]
    pub viewport: ViewRect,
    #[serde(default)]
    pub attention: Option<ViewAttention>,
    #[serde(default)]
    pub navigation: ViewNavigation,
    #[serde(default)]
    pub parameters: BTreeMap<String, ViewParameterValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SharedViewPatch {
    pub schema_version: u32,
    pub view_id: String,
    pub base_revision: u64,
    pub revision: u64,
    pub operations: Vec<ViewOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ViewOperation {
    SetSourcePosition {
        source_id: String,
        position: SourcePosition,
    },
    SetViewport {
        viewport: ViewRect,
    },
    SetAttention {
        attention: Option<ViewAttention>,
    },
    SetNavigation {
        navigation: ViewNavigation,
    },
    SetParameter {
        id: String,
        value: ViewParameterValue,
    },
    RemoveParameter {
        id: String,
    },
}

impl VisualSourceRef {
    pub fn validate(&self) -> Result<()> {
        check(identifier(&self.owner), "Invalid visual source owner")?;
        check(source_identifier(&self.id), "Invalid visual source id")?;
        check(contract(&self.contract), "Invalid visual source contract")
    }
}

fn source_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

impl ViewRect {
    pub fn validate(self) -> Result<()> {
        let values = [self.x, self.y, self.width, self.height];
        check(
            values.iter().all(|value| value.is_finite()),
            "View rectangle values must be finite",
        )?;
        check(
            self.x >= 0.0
                && self.y >= 0.0
                && self.width > 0.0
                && self.height > 0.0
                && self.x + self.width <= 1.0
                && self.y + self.height <= 1.0,
            "View rectangle must be a positive normalized region",
        )
    }
}

impl SharedViewDocument {
    pub fn validate(&self) -> Result<()> {
        check(
            self.schema_version == SCHEMA_VERSION,
            "Unsupported shared view schema_version",
        )?;
        check(identifier(&self.id), "Invalid shared view id")?;
        check(self.revision > 0, "Shared view revision must be positive")?;
        check(
            !self.sources.is_empty() && self.sources.len() <= MAX_VIEW_SOURCES,
            "Shared view source count is invalid",
        )?;
        for (id, source) in &self.sources {
            check(identifier(id), "Invalid shared view source id")?;
            source.source.validate()?;
            validate_source_position(source.source.kind, source.position)?;
        }
        self.viewport.validate()?;
        if let Some(attention) = &self.attention {
            validate_attention(attention)?;
        }
        check(
            self.parameters.len() <= MAX_VIEW_PARAMETERS,
            "Shared view has too many parameters",
        )?;
        for (id, value) in &self.parameters {
            check(identifier(id), "Invalid shared view parameter id")?;
            validate_parameter(value)?;
        }
        Ok(())
    }

    pub fn apply_patch(&self, patch: &SharedViewPatch) -> Result<Self> {
        self.validate()?;
        check(
            patch.schema_version == SCHEMA_VERSION,
            "Unsupported shared view patch schema_version",
        )?;
        check(
            patch.view_id == self.id,
            "Shared view patch targets another view",
        )?;
        check(
            patch.base_revision == self.revision,
            format!(
                "Shared view revision conflict: expected {}, received {}",
                self.revision, patch.base_revision
            ),
        )?;
        check(
            patch.revision == patch.base_revision + 1,
            "Shared view patch revision must advance by one",
        )?;
        check(
            !patch.operations.is_empty() && patch.operations.len() <= MAX_VIEW_OPERATIONS,
            "Shared view patch operation count is invalid",
        )?;

        let mut next = self.clone();
        let mut touched = BTreeSet::new();
        for operation in &patch.operations {
            let field = match operation {
                ViewOperation::SetSourcePosition {
                    source_id,
                    position,
                } => {
                    check(identifier(source_id), "Invalid shared view source id")?;
                    let source = next
                        .sources
                        .get_mut(source_id)
                        .ok_or_else(|| Error(format!("Unknown shared view source: {source_id}")))?;
                    source.position = *position;
                    continue;
                }
                ViewOperation::SetViewport { viewport } => {
                    next.viewport = *viewport;
                    "viewport"
                }
                ViewOperation::SetAttention { attention } => {
                    next.attention.clone_from(attention);
                    "attention"
                }
                ViewOperation::SetNavigation { navigation } => {
                    next.navigation = *navigation;
                    "navigation"
                }
                ViewOperation::SetParameter { id, value } => {
                    check(identifier(id), "Invalid shared view parameter id")?;
                    validate_parameter(value)?;
                    next.parameters.insert(id.clone(), value.clone());
                    continue;
                }
                ViewOperation::RemoveParameter { id } => {
                    check(identifier(id), "Invalid shared view parameter id")?;
                    next.parameters.remove(id);
                    continue;
                }
            };
            check(
                touched.insert(field),
                format!("Shared view patch changes {field} more than once"),
            )?;
        }
        next.revision = patch.revision;
        next.validate()?;
        Ok(next)
    }
}

fn validate_source_position(kind: VisualSourceKind, position: SourcePosition) -> Result<()> {
    match (kind, position) {
        (VisualSourceKind::Artifact, SourcePosition::Revision { revision }) => {
            check(revision > 0, "Artifact source revision must be positive")
        }
        (VisualSourceKind::Stream, SourcePosition::Live { .. })
        | (VisualSourceKind::Stream, SourcePosition::Sequence { .. }) => Ok(()),
        (VisualSourceKind::Artifact, _) => Err(Error(
            "Artifact source requires an exact revision position".into(),
        )),
        (VisualSourceKind::Stream, _) => Err(Error(
            "Stream source requires a live or sequence position".into(),
        )),
    }
}

fn validate_attention(attention: &ViewAttention) -> Result<()> {
    check(
        identifier(&attention.actor_id),
        "Invalid attention actor id",
    )?;
    attention.region.validate()?;
    if let Some(label) = &attention.label {
        check(visible(label), "Invalid attention label")?;
    }
    Ok(())
}

fn validate_parameter(value: &ViewParameterValue) -> Result<()> {
    match value {
        ViewParameterValue::Boolean(_) => Ok(()),
        ViewParameterValue::Number(value) => {
            check(value.is_finite(), "View parameter number must be finite")
        }
        ViewParameterValue::Text(value) => check(visible(value), "Invalid view parameter text"),
        ViewParameterValue::Range { minimum, maximum } => check(
            minimum.is_finite() && maximum.is_finite() && minimum <= maximum,
            "View parameter range must be finite and ordered",
        ),
        ViewParameterValue::Set(values) => {
            check(
                values.len() <= MAX_PARAMETER_VALUES,
                "View parameter set is too large",
            )?;
            for value in values {
                check(visible(value), "Invalid view parameter set value")?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_view() -> SharedViewDocument {
        SharedViewDocument {
            schema_version: SCHEMA_VERSION,
            id: "inspection-view".into(),
            revision: 4,
            sources: BTreeMap::from([(
                "image".into(),
                ViewSource {
                    source: VisualSourceRef {
                        owner: "dock".into(),
                        id: "board-photo".into(),
                        contract: "image.rgba.v1".into(),
                        kind: VisualSourceKind::Artifact,
                    },
                    position: SourcePosition::Revision { revision: 8 },
                },
            )]),
            viewport: ViewRect::default(),
            attention: None,
            navigation: ViewNavigation::None,
            parameters: BTreeMap::new(),
        }
    }

    #[test]
    fn agent_focuses_image_region_without_replacing_artifact() {
        let original = image_view();
        let focused = original
            .apply_patch(&SharedViewPatch {
                schema_version: SCHEMA_VERSION,
                view_id: original.id.clone(),
                base_revision: 4,
                revision: 5,
                operations: vec![
                    ViewOperation::SetViewport {
                        viewport: ViewRect {
                            x: 0.2,
                            y: 0.1,
                            width: 0.4,
                            height: 0.5,
                        },
                    },
                    ViewOperation::SetAttention {
                        attention: Some(ViewAttention {
                            actor_kind: AttentionActorKind::Agent,
                            actor_id: "codex".into(),
                            region: ViewRect {
                                x: 0.3,
                                y: 0.2,
                                width: 0.1,
                                height: 0.1,
                            },
                            label: Some("trace near regulator".into()),
                        }),
                    },
                ],
            })
            .unwrap();

        assert_eq!(focused.sources, original.sources);
        assert_eq!(focused.revision, 5);
        assert_eq!(focused.attention.unwrap().actor_id, "codex");
    }

    #[test]
    fn live_graph_uses_the_same_view_lifecycle() {
        let mut view = image_view();
        view.sources = BTreeMap::from([
            (
                "brain".into(),
                ViewSource {
                    source: VisualSourceRef {
                        owner: "elmers".into(),
                        id: "brain-events".into(),
                        contract: "graph.events.v1".into(),
                        kind: VisualSourceKind::Stream,
                    },
                    position: SourcePosition::Live {
                        observed_sequence: 901,
                    },
                },
            ),
            (
                "presence".into(),
                ViewSource {
                    source: VisualSourceRef {
                        owner: "jarvis".into(),
                        id: "presence-events".into(),
                        contract: "presence.events.v1".into(),
                        kind: VisualSourceKind::Stream,
                    },
                    position: SourcePosition::Live {
                        observed_sequence: 72,
                    },
                },
            ),
        ]);
        view.parameters
            .insert("minimum-strength".into(), ViewParameterValue::Number(0.45));

        view.validate().unwrap();
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            serde_json::from_str::<SharedViewDocument>(&json).unwrap(),
            view
        );
    }

    #[test]
    fn rejects_invalid_region_and_source_position() {
        let mut view = image_view();
        view.viewport = ViewRect {
            x: 0.8,
            y: 0.0,
            width: 0.3,
            height: 1.0,
        };
        assert!(view.validate().unwrap_err().0.contains("normalized region"));

        view.viewport = ViewRect::default();
        view.sources.get_mut("image").unwrap().position = SourcePosition::Live {
            observed_sequence: 1,
        };
        assert!(view.validate().unwrap_err().0.contains("Artifact source"));
    }

    #[test]
    fn artifact_source_identity_accepts_a_full_content_digest() {
        let mut view = image_view();
        view.sources.get_mut("image").unwrap().source.id = format!("asset-{}", "ab".repeat(32));
        view.validate().unwrap();

        view.sources.get_mut("image").unwrap().source.id = "asset/../../secret".into();
        assert!(view.validate().unwrap_err().0.contains("source id"));
    }

    #[test]
    fn stale_patch_is_rejected() {
        let error = image_view()
            .apply_patch(&SharedViewPatch {
                schema_version: SCHEMA_VERSION,
                view_id: "inspection-view".into(),
                base_revision: 3,
                revision: 4,
                operations: vec![ViewOperation::SetViewport {
                    viewport: ViewRect::default(),
                }],
            })
            .unwrap_err();
        assert!(error.0.contains("revision conflict"));
    }
}
