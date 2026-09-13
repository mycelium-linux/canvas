//! Shared visual state observed and changed by both humans and agents.
//!
//! Sources own data and transport. Canvas owns the bounded, revisioned view of
//! that data: viewport, attention, navigation, and presentation parameters.

use super::{Error, Result, SCHEMA_VERSION, check, contract, identifier, visible};
use canvas_widgets::WidgetSpec;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_VIEW_PARAMETERS: usize = 64;
const MAX_PARAMETER_VALUES: usize = 256;
const MAX_VIEW_OPERATIONS: usize = 128;
const MAX_VIEW_SOURCES: usize = 64;
/// Placed widgets per view. A surface a person can actually read runs out long
/// before this; the cap is here so a patch cannot grow a document without bound.
const MAX_VIEW_WIDGETS: usize = 64;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum SourcePosition {
    /// Exact immutable revision of a finite artifact or snapshot.
    Revision { revision: u64 },
    /// Follow a live source while recording the last sequence Canvas observed.
    Live {
        observed_sequence: u64,
        #[serde(default)]
        last_observed_ms: u64,
        #[serde(default)]
        state: FreshnessState,
    },
    /// Hold a live source at an exact replayable sequence.
    Sequence { sequence: u64 },
}

/// Health of a live source, surfaced visibly so stale data is never
/// silently presented as current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum FreshnessState {
    /// Source is current.
    Fresh,
    /// Source hasn't updated within its declared freshness window.
    Stale { age_ms: u64 },
    /// Source connection is lost. Last known-good is preserved.
    Disconnected,
    /// Source returned an error.
    Error { message: String },
}

impl Default for FreshnessState {
    fn default() -> Self {
        FreshnessState::Stale { age_ms: 0 }
    }
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

/// A widget instance placed on the view surface.
///
/// Presentation Canvas owns, which is why it is not a [`ViewSource`]: a source
/// names data owned elsewhere and carries a position into it, while a widget is
/// the drawing itself. Kept in the shared document rather than a store beside it
/// so placement inherits the revision, patch and conflict rules the rest of the
/// view already has, and so whatever persists a view persists its widgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PlacedWidget {
    pub spec: WidgetSpec,
    /// Where on the surface, in the same normalized space as [`ViewRect`].
    #[serde(default)]
    pub rect: ViewRect,
}
/// Whether a view's arrangement should fill the displays Canvas currently
/// occupies, or keep its geometry on the full desktop while displays reveal and
/// hide parts of it.
///
/// This is a property of the view rather than a global mode because the two
/// behaviours serve different uses, not different tastes: an operational
/// dashboard wants to fill whatever screens exist, while a workspace someone
/// arranged by hand is navigated from spatial memory and should stay put. A
/// global switch would force one answer for both and leave "why did my layout
/// move" answerable only by recalling which mode is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum LayoutPolicy {
    /// Fill the displays Canvas is on; a display change reflows the layout.
    #[default]
    Reflow,
    /// Keep the arrangement on the whole desktop; displays reveal and hide it.
    Fixed,
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
    /// Absent in documents written before widgets existed, hence `default`.
    #[serde(default)]
    pub widgets: BTreeMap<String, PlacedWidget>,
    /// Absent in views written before display reflow existed, hence `default`.
    /// Reflow is the default because a saved view is usually operational.
    #[serde(default)]
    pub layout_policy: LayoutPolicy,
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

/// A proposal from Alfalfa to Canvas for an executive view.
///
/// Alfalfa sends this to recommend a view for a project. Canvas validates it
/// against the recommendation contract, checks provenance, and either applies
/// it as a new SharedViewDocument or updates an existing one. The proposal is
/// revision-checked: a stale recommendation is rejected before rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ViewRecommendation {
    pub schema_version: u32,
    pub project_id: String,
    pub view_id: String,
    /// Monotonic revision that Canvas must have seen less than this value to
    /// accept the recommendation. Prevents a slow network from regressing a
    /// view that has already advanced.
    pub expected_base_revision: Option<u64>,
    pub rationale: String,
    pub audience: AudiencePolicy,
    pub sources: Vec<RecommendationSource>,
    pub widgets: BTreeMap<String, PlacedWidget>,
    pub refresh: RefreshPolicy,
    pub evidence_revision: u64,
    pub provenance: ProvenanceReceipt,
}

/// Whether a view is intended for public walls, specific agents, or a
/// private screen. Carries the auth scope that must be satisfied to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum AudiencePolicy {
    /// Visible to every authenticated viewer on the wall.
    Public,
    /// Visible only to callers with the named scope (e.g. "agent:cortex").
    Scoped { value: String },
    /// Visible only to the proposing agent, identified by actor id.
    Private { actor_id: String },
}

/// One data source in a recommendation, with the connector/query identity
/// that Canvas resolves at the trusted host boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RecommendationSource {
    pub id: String,
    pub owner: String,
    pub contract: String,
    /// Stable connector/query identifier (e.g. "prometheus-elmers/loop").
    /// Canvas resolves this to transport+credentials at the host boundary;
    /// the recommendation never carries URLs or secrets.
    pub connector: String,
    pub refresh_seconds: u32,
    pub freshness_seconds: u32,
}

/// Refresh and staleness policy for a recommended view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RefreshPolicy {
    /// How often the view polls its sources.
    pub interval_seconds: u32,
    /// After this many seconds without an update, the view marks itself stale.
    pub stale_after_seconds: u32,
}

/// Receipt proving the recommendation was produced by a trusted source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ProvenanceReceipt {
    pub proposed_by: String,
    pub proposed_at_ms: u64,
    /// SHA-256 of the recommendation body (excluding this field), verified by
    /// the proposer's key. Proves the content has not been tampered with.
    pub digest: String,
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
    PlaceWidget {
        id: String,
        widget: Box<PlacedWidget>,
    },
    MoveWidget {
        id: String,
        rect: ViewRect,
    },
    RemoveWidget {
        id: String,
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
        // A view still may not be empty, but a dashboard of a clock and a label is a
        // real view with nothing external to name, so widgets satisfy this too.
        check(
            !(self.sources.is_empty() && self.widgets.is_empty()),
            "Shared view must carry a source or a widget",
        )?;
        check(
            self.sources.len() <= MAX_VIEW_SOURCES,
            "Shared view source count is invalid",
        )?;
        for (id, source) in &self.sources {
            check(identifier(id), "Invalid shared view source id")?;
            source.source.validate()?;
            validate_source_position(source.source.kind, source.position.clone())?;
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
        check(
            self.widgets.len() <= MAX_VIEW_WIDGETS,
            "Shared view has too many widgets",
        )?;
        for (id, widget) in &self.widgets {
            check(identifier(id), "Invalid shared view widget id")?;
            validate_placed_widget(widget)?;
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
                    source.position = position.clone();
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
                ViewOperation::PlaceWidget { id, widget } => {
                    check(identifier(id), "Invalid shared view widget id")?;
                    validate_placed_widget(widget)?;
                    next.widgets.insert(id.clone(), (**widget).clone());
                    continue;
                }
                ViewOperation::MoveWidget { id, rect } => {
                    check(identifier(id), "Invalid shared view widget id")?;
                    rect.validate()?;
                    let widget = next
                        .widgets
                        .get_mut(id)
                        .ok_or_else(|| Error(format!("Unknown shared view widget: {id}")))?;
                    widget.rect = *rect;
                    continue;
                }
                ViewOperation::RemoveWidget { id } => {
                    check(identifier(id), "Invalid shared view widget id")?;
                    next.widgets.remove(id);
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

/// A placed widget is only as valid as its spec, so the widget crate keeps owning
/// what a spec means and this checks placement.
fn validate_placed_widget(widget: &PlacedWidget) -> Result<()> {
    widget
        .spec
        .validate()
        .map_err(|error| Error(format!("Invalid placed widget spec: {error}")))?;
    widget.rect.validate()
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
            layout_policy: Default::default(),
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
            widgets: BTreeMap::new(),
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
                        last_observed_ms: 0,
                        state: FreshnessState::Fresh,
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
                        last_observed_ms: 0,
                        state: FreshnessState::Fresh,
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
            last_observed_ms: 0,
            state: FreshnessState::Fresh,
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

#[cfg(test)]
mod placed_widget_tests {
    use super::*;
    use canvas_widgets::WidgetKind;

    fn text_widget(body: &str) -> PlacedWidget {
        PlacedWidget {
            spec: WidgetSpec {
                schema_version: canvas_widgets::WIDGET_SCHEMA_VERSION,
                id: "w".into(),
                kind: WidgetKind::Text { text: body.into() },
                theme: None,
                refresh_ms: None,
                chrome: canvas_widgets::WidgetChrome::DragOnly,
                data: canvas_widgets::WidgetData::default(),
            },
            rect: ViewRect {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            },
        }
    }

    fn widget_only_view() -> SharedViewDocument {
        SharedViewDocument {
            layout_policy: Default::default(),
            schema_version: SCHEMA_VERSION,
            id: "dash".into(),
            revision: 1,
            sources: BTreeMap::new(),
            viewport: ViewRect::default(),
            attention: None,
            navigation: ViewNavigation::None,
            parameters: BTreeMap::new(),
            widgets: BTreeMap::from([("clock".into(), text_widget("hello"))]),
        }
    }

    fn patch(view: &SharedViewDocument, operations: Vec<ViewOperation>) -> SharedViewPatch {
        SharedViewPatch {
            schema_version: SCHEMA_VERSION,
            view_id: view.id.clone(),
            base_revision: view.revision,
            revision: view.revision + 1,
            operations,
        }
    }

    #[test]
    fn a_view_of_only_widgets_is_a_real_view() {
        // A dashboard of a clock and a label names nothing external, and refusing it
        // would push widgets into a store beside the document.
        widget_only_view().validate().expect("widget-only view");
    }

    #[test]
    fn a_view_with_neither_a_source_nor_a_widget_is_still_refused() {
        let mut view = widget_only_view();
        view.widgets.clear();
        assert!(view.validate().is_err(), "an empty view must not validate");
    }

    #[test]
    fn placing_moving_and_removing_a_widget_advances_the_revision() {
        let view = widget_only_view();
        let placed = view
            .apply_patch(&patch(
                &view,
                vec![ViewOperation::PlaceWidget {
                    id: "note".into(),
                    widget: Box::new(text_widget("second")),
                }],
            ))
            .expect("place");
        assert_eq!(placed.revision, 2);
        assert_eq!(placed.widgets.len(), 2);

        let moved = placed
            .apply_patch(&patch(
                &placed,
                vec![ViewOperation::MoveWidget {
                    id: "note".into(),
                    rect: ViewRect {
                        x: 0.25,
                        y: 0.25,
                        width: 0.25,
                        height: 0.25,
                    },
                }],
            ))
            .expect("move");
        assert_eq!(moved.widgets["note"].rect.x, 0.25);
        // Moving must not disturb the spec it is moving.
        assert_eq!(moved.widgets["note"].spec, placed.widgets["note"].spec);

        let removed = moved
            .apply_patch(&patch(
                &moved,
                vec![ViewOperation::RemoveWidget { id: "note".into() }],
            ))
            .expect("remove");
        assert_eq!(removed.widgets.len(), 1);
        assert_eq!(removed.revision, 4);
    }

    #[test]
    fn moving_a_widget_that_is_not_there_fails_rather_than_creating_one() {
        let view = widget_only_view();
        let error = view
            .apply_patch(&patch(
                &view,
                vec![ViewOperation::MoveWidget {
                    id: "absent".into(),
                    rect: ViewRect::default(),
                }],
            ))
            .expect_err("unknown widget");
        assert!(
            error.0.contains("Unknown shared view widget"),
            "{}",
            error.0
        );
    }

    #[test]
    fn an_invalid_spec_cannot_be_placed() {
        let mut widget = text_widget("");
        widget.spec.kind = WidgetKind::Text {
            text: String::new(),
        };
        let view = widget_only_view();
        assert!(
            view.apply_patch(&patch(
                &view,
                vec![ViewOperation::PlaceWidget {
                    id: "bad".into(),
                    widget: Box::new(widget),
                }],
            ))
            .is_err(),
            "the widget crate owns what a spec means; an invalid one must not land"
        );
    }

    #[test]
    fn a_document_written_before_widgets_existed_still_loads() {
        // The field is `default`, so an older document is not a parse error.
        let json = r#"{"schema_version":1,"id":"old","revision":3,
            "sources":{"a":{"source":{"owner":"panopticon","id":"m","contract":"map.v1","kind":"stream"},
            "position":{"kind":"live","observed_sequence":9}}}}"#;
        let view: SharedViewDocument = serde_json::from_str(json).expect("legacy document");
        assert!(view.widgets.is_empty());
        view.validate().expect("legacy documents stay valid");
    }

    #[test]
    fn widgets_survive_a_round_trip_through_json() {
        // Whatever persists a view persists its widgets; this is that guarantee.
        let view = widget_only_view();
        let json = serde_json::to_string(&view).expect("serialize");
        assert_eq!(
            serde_json::from_str::<SharedViewDocument>(&json).expect("deserialize"),
            view
        );
    }

    #[test]
    fn view_recommendation_round_trips() {
        let rec = ViewRecommendation {
            schema_version: 1,
            project_id: "alfalfa".into(),
            view_id: "alfalfa/executive".into(),
            expected_base_revision: Some(5),
            rationale: "Live view of Alfalfa CI pipeline health".into(),
            audience: AudiencePolicy::Scoped {
                value: "agent:alfalfa".into(),
            },
            sources: vec![RecommendationSource {
                id: "pipeline".into(),
                owner: "alfalfa".into(),
                contract: "canvas.prometheus.v1".into(),
                connector: "prometheus-alfalfa/pipeline".into(),
                refresh_seconds: 30,
                freshness_seconds: 120,
            }],
            widgets: BTreeMap::new(),
            refresh: RefreshPolicy {
                interval_seconds: 30,
                stale_after_seconds: 120,
            },
            evidence_revision: 1,
            provenance: ProvenanceReceipt {
                proposed_by: "alfalfa-cli".into(),
                proposed_at_ms: 1710000000000,
                digest: "a".repeat(64),
            },
        };
        let json = serde_json::to_string(&rec).unwrap();
        let parsed: ViewRecommendation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, rec);
    }

    #[test]
    fn unknown_fields_in_recommendation_are_rejected() {
        let json = r#"{
            "schema_version": 1,
            "project_id": "test",
            "view_id": "test/view",
            "expected_base_revision": null,
            "rationale": "test",
            "audience": {"scope": "public"},
            "sources": [],
            "widgets": {},
            "refresh": {"interval_seconds": 10, "stale_after_seconds": 30},
            "evidence_revision": 1,
            "provenance": {"proposed_by": "test", "proposed_at_ms": 1, "digest": "x"},
            "sneaky_field": "should be rejected"
        }"#;
        assert!(serde_json::from_str::<ViewRecommendation>(json).is_err());
    }
}
