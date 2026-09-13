//! Renderer-independent Canvas workspace documents.
//!
//! This crate owns durable presentation intent. Renderers own decoded assets,
//! frame state, GPU allocations, windows, and other reconstructible state.

use canvas_core::{Catalog, ResourceKind, ThemeSelection, is_declared_contract};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub mod backend;
pub mod background_picker;
pub mod briefing;
pub mod chart;
pub mod control;
pub mod display;
pub mod hud;
pub mod interaction;
#[cfg(unix)]
pub mod loopback;
#[cfg(feature = "lua-scenes")]
pub mod lua_module;
#[cfg(feature = "lua-scenes")]
pub mod lua_scene;
pub mod map;
#[cfg(all(feature = "mcp", unix))]
pub mod mcp;
pub mod media;
pub mod operational_views;
pub mod project_picker;
pub mod project_switcher;
pub mod reconcile;
pub mod runtime_config;
pub mod screensaver;
#[cfg(feature = "lua-scenes")]
pub mod stream_binding;
pub mod theme_picker;
pub mod view;
pub mod workspace_store;

pub const SCHEMA_VERSION: u32 = 1;
const MAX_ITEMS: usize = 1_024;
const MAX_GRADIENT_STOPS: usize = 8;
pub const MAX_CANVAS_PADDING: u16 = 96;
pub const MAX_SURFACE_GAP: u16 = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn check(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error(message.into()))
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn contract(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'/')
        })
}

fn visible(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 16_384
        && !value.chars().any(|character| character.is_control())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct WorkspaceDocument {
    pub schema_version: u32,
    pub id: String,
    pub project_id: String,
    pub revision: u64,
    pub theme: ThemeRef,
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub chrome: SurfaceChrome,
    pub surfaces: Vec<Surface>,
    pub layout: Layout,
    #[serde(default)]
    pub bindings: Vec<DataBinding>,
    #[serde(default)]
    pub focused_surface_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SurfaceChrome {
    #[serde(default)]
    pub title: SurfaceTitleMode,
    #[serde(default)]
    pub canvas_padding: u16,
    #[serde(default)]
    pub surface_gap: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum SurfaceTitleMode {
    Hidden,
    #[default]
    Focused,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ThemeRef {
    Project,
    /// A workspace-local selection from the bounded native theme catalog.
    Selection {
        theme: ThemeSelection,
    },
}

/// Backgrounds carry the same version boundary as [`SurfaceContent`], and for
/// the same reason: a `shader_id` this build has never heard of once made every
/// client unable to parse *any* control reply, so `canvas_inspect`, every MCP
/// write and `canvasctl` all failed together against a store a newer Canvas had
/// written. A cosmetic value took down the whole control plane.
///
/// The boundary sits at two depths here as well. `shader` is a kind this build
/// owns, but the shader *inside* it may belong to a newer one; that degrades to
/// [`Background::Unsupported`], retained verbatim and written back untouched.
/// A malformed *known* background still fails loudly -- that is a mistake, not a
/// version.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Background {
    /// Derive the background gradient from the active Coherence project theme.
    #[default]
    Theme,
    Solid {
        color: [u8; 4],
    },
    Gradient {
        angle_degrees: f32,
        stops: Vec<GradientStop>,
    },
    Image {
        asset: AssetRef,
        fit: BackgroundFit,
    },
    Shader {
        shader_id: BackgroundShader,
        speed: f32,
    },
    /// A background from a newer Canvas, kept exactly as written.
    #[serde(untagged)]
    Unsupported(serde_json::Value),
}

/// The `kind` tags this build can draw. Anything else is a newer Canvas.
const KNOWN_BACKGROUND_KINDS: [&str; 5] = ["theme", "solid", "gradient", "image", "shader"];

impl<'de> Deserialize<'de> for Background {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let raw = serde_json::Value::deserialize(deserializer)?;
        let tag = raw
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("Canvas background needs a kind"))?;
        if !KNOWN_BACKGROUND_KINDS.contains(&tag) {
            return Ok(Self::Unsupported(raw));
        }
        // The second depth: `shader` is ours, the shader named inside it may not
        // be. Without this a newer shader is indistinguishable from a malformed
        // known background and takes every control reply down with it, which is
        // how `star-nest` blinded the control plane.
        if tag == "shader"
            && raw
                .get("shader_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| !is_known_shader(id))
        {
            return Ok(Self::Unsupported(raw));
        }
        KnownBackground::deserialize(&raw)
            .map(KnownBackground::into_background)
            .map_err(D::Error::custom)
    }
}

/// Asking serde rather than a second list: a hand-written name table would drift
/// from the enum's own `rename_all` the first time a shader is added.
fn is_known_shader(id: &str) -> bool {
    serde_json::from_value::<BackgroundShader>(serde_json::Value::String(id.to_owned())).is_ok()
}

/// The derived shape, so the variant list is not written twice.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum KnownBackground {
    Theme,
    Solid {
        color: [u8; 4],
    },
    Gradient {
        angle_degrees: f32,
        stops: Vec<GradientStop>,
    },
    Image {
        asset: AssetRef,
        fit: BackgroundFit,
    },
    Shader {
        shader_id: BackgroundShader,
        speed: f32,
    },
}

impl KnownBackground {
    fn into_background(self) -> Background {
        match self {
            Self::Theme => Background::Theme,
            Self::Solid { color } => Background::Solid { color },
            Self::Gradient {
                angle_degrees,
                stops,
            } => Background::Gradient {
                angle_degrees,
                stops,
            },
            Self::Image { asset, fit } => Background::Image { asset, fit },
            Self::Shader { shader_id, speed } => Background::Shader { shader_id, speed },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum BackgroundShader {
    Tidepool,
    AuroraDrift,
    DuskEmber,
    SlowRipple,
    Spirales,
    FloatingRectangles,
    SmokeRings,
    Flame,
    Meme,
    StarNest,
}

impl BackgroundShader {
    pub const ALL: [Self; 10] = [
        Self::Tidepool,
        Self::AuroraDrift,
        Self::DuskEmber,
        Self::SlowRipple,
        Self::Spirales,
        Self::FloatingRectangles,
        Self::SmokeRings,
        Self::Flame,
        Self::Meme,
        Self::StarNest,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct GradientStop {
    pub offset: f32,
    pub color: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum BackgroundFit {
    Cover,
    Contain,
    Stretch,
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Surface {
    pub id: String,
    pub title: String,
    pub content: SurfaceContent,
    #[serde(default)]
    pub inputs: Vec<InputPort>,
}

/// Surface kinds this build understands, plus everything it does not.
///
/// A closed enum here used to mean that one unrecognized `kind` anywhere in a
/// workspace made the entire store unloadable — Canvas panicked on startup
/// against a store written by a newer build, and the workspace adapter could not
/// parse *any* control reply because it had never heard of `widget`. Neither
/// failure was confined to the surface that caused it.
///
/// A kind is a version boundary, not a typo. Unknown ones are retained verbatim
/// in [`SurfaceContent::Unsupported`] and drawn as a placeholder, so an older
/// binary degrades one tile instead of refusing to run — and writes the original
/// back untouched rather than destroying a widget it could not draw.
///
/// Malformed *known* kinds still fail loudly; that is a mistake, not a version.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum SurfaceContent {
    ProjectResource {
        resource_id: String,
    },
    Asset {
        asset: AssetRef,
    },
    Chart {
        chart: chart::ChartDocument,
    },
    Widget {
        spec: canvas_widgets::WidgetSpec,
    },
    Scene {
        scene: Scene,
    },
    Terminal {
        resource_id: String,
    },
    Stream {
        url: String,
    },
    /// Content from a newer Canvas, kept exactly as written.
    #[serde(untagged)]
    Unsupported(serde_json::Value),
}

/// The `kind` tags this build can render. Anything else is a newer Canvas.
const KNOWN_SURFACE_KINDS: [&str; 7] = [
    "project_resource",
    "asset",
    "chart",
    "widget",
    "scene",
    "terminal",
    "stream",
];

impl SurfaceContent {
    /// The wire tag, including for content this build cannot render.
    pub fn kind_tag(&self) -> &str {
        match self {
            Self::ProjectResource { .. } => "project_resource",
            Self::Asset { .. } => "asset",
            Self::Chart { .. } => "chart",
            Self::Widget { .. } => "widget",
            Self::Scene { .. } => "scene",
            Self::Terminal { .. } => "terminal",
            Self::Stream { .. } => "stream",
            Self::Unsupported(raw) => raw
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        }
    }

    /// What a placeholder should say, or `None` when this build can draw it.
    pub fn unsupported_reason(&self) -> Option<String> {
        match self {
            Self::Unsupported(_) => Some(format!(
                "unsupported surface \u{201c}{}\u{201d} — this Canvas is older than this workspace",
                self.kind_tag()
            )),
            // A widget kind this build does not know fails `Widget`'s own parse
            // and arrives here as `Unsupported`, so there is nothing extra to
            // detect: the same placeholder covers both depths.
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for SurfaceContent {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let raw = serde_json::Value::deserialize(deserializer)?;
        let tag = raw
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("Canvas surface content needs a kind"))?;
        if !KNOWN_SURFACE_KINDS.contains(&tag) {
            return Ok(Self::Unsupported(raw));
        }
        // The version boundary exists at two depths: `widget` is a kind this
        // build owns, but the widget *inside* it may not be. Without this a
        // newer widget kind is indistinguishable from a malformed known one and
        // takes the whole document down — which is how `rate_bars` stopped
        // Canvas from starting.
        if tag == "widget"
            && raw
                .get("spec")
                .and_then(|spec| spec.get("kind"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| !canvas_widgets::is_known_widget_kind(kind))
        {
            return Ok(Self::Unsupported(raw));
        }
        // A tag this build owns must parse. Degrading here would turn a typo in
        // a field name into a silently blank tile.
        Known::deserialize(&raw)
            .map(Known::into_content)
            .map_err(D::Error::custom)
    }
}

/// The derived shape, so the variant list is not written twice.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Known {
    ProjectResource { resource_id: String },
    Asset { asset: AssetRef },
    Chart { chart: chart::ChartDocument },
    Widget { spec: canvas_widgets::WidgetSpec },
    Scene { scene: Scene },
    Terminal { resource_id: String },
    Stream { url: String },
}

impl Known {
    fn into_content(self) -> SurfaceContent {
        match self {
            Self::ProjectResource { resource_id } => {
                SurfaceContent::ProjectResource { resource_id }
            }
            Self::Asset { asset } => SurfaceContent::Asset { asset },
            Self::Chart { chart } => SurfaceContent::Chart { chart },
            Self::Widget { spec } => SurfaceContent::Widget { spec },
            Self::Scene { scene } => SurfaceContent::Scene { scene },
            Self::Terminal { resource_id } => SurfaceContent::Terminal { resource_id },
            Self::Stream { url } => SurfaceContent::Stream { url },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AssetRef {
    pub sha256: String,
    pub media_type: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct InputPort {
    pub id: String,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DataBinding {
    pub id: String,
    pub source: DataSource,
    pub target: InputTarget,
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub delivery: DeliveryPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum DeliveryPolicy {
    Latest,
    Ordered {
        #[serde(default = "default_stream_batch")]
        max_batch: u16,
        #[serde(default = "default_stream_age_ms")]
        max_age_ms: u32,
    },
}

impl Default for DeliveryPolicy {
    fn default() -> Self {
        Self::Ordered {
            max_batch: default_stream_batch(),
            max_age_ms: default_stream_age_ms(),
        }
    }
}

const fn default_stream_batch() -> u16 {
    64
}
const fn default_stream_age_ms() -> u32 {
    2_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DataSource {
    pub owner: String,
    pub stream: String,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct InputTarget {
    pub surface_id: String,
    pub input_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Layout {
    Empty,
    Surface {
        surface_id: String,
    },
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
    Stack {
        surface_ids: Vec<String>,
        active_surface_id: String,
    },
    Canvas {
        placements: Vec<Placement>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Placement {
    pub surface_id: String,
    pub bounds: NormalizedRect,
    #[serde(default)]
    pub layer: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SceneNode {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub transform: Transform3,
    pub content: NodeContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Transform3 {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform3 {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum NodeContent {
    Group,
    Text { text: String },
    Image { asset: AssetRef },
    Mesh { asset: AssetRef },
    Camera { projection: Projection },
    Light { light: Light },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Projection {
    Perspective {
        vertical_fov_degrees: f32,
        near: f32,
        far: f32,
    },
    Orthographic {
        height: f32,
        near: f32,
        far: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum Light {
    Ambient { intensity: f32 },
    Directional { intensity: f32, color: [f32; 3] },
    Point { intensity: f32, color: [f32; 3] },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct WorkspacePatch {
    pub schema_version: u32,
    pub workspace_id: String,
    pub base_revision: u64,
    pub revision: u64,
    pub operations: Vec<PatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum PatchOperation {
    UpsertSurface { surface: Surface },
    RemoveSurface { surface_id: String },
    SetLayout { layout: Layout },
    SetTheme { theme: ThemeRef },
    SetBackground { background: Background },
    SetChrome { chrome: SurfaceChrome },
    SetFocus { surface_id: Option<String> },
    UpsertBinding { binding: DataBinding },
    RemoveBinding { binding_id: String },
}

impl AssetRef {
    fn validate(&self) -> Result<()> {
        check(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "Asset sha256 must be 64 lowercase hexadecimal characters",
        )?;
        check(
            visible(&self.media_type) && self.media_type.contains('/'),
            "Asset media_type must be a printable MIME type",
        )?;
        check(self.byte_length > 0, "Asset byte_length must be positive")
    }
}

impl Background {
    pub fn is_animated(&self) -> bool {
        matches!(self, Self::Shader { speed, .. } if *speed > 0.0)
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Theme | Self::Solid { .. } => Ok(()),
            Self::Gradient {
                angle_degrees,
                stops,
            } => {
                check(angle_degrees.is_finite(), "Gradient angle must be finite")?;
                check(
                    (2..=MAX_GRADIENT_STOPS).contains(&stops.len()),
                    "Gradient must contain 2 to 8 stops",
                )?;
                let mut previous = None;
                for stop in stops {
                    check(
                        stop.offset.is_finite()
                            && (0.0..=1.0).contains(&stop.offset)
                            && previous.is_none_or(|offset| stop.offset > offset),
                        "Gradient stops must be strictly ordered between zero and one",
                    )?;
                    previous = Some(stop.offset);
                }
                Ok(())
            }
            Self::Image { asset, .. } => {
                asset.validate()?;
                check(
                    asset.media_type.starts_with("image/"),
                    "Canvas background assets must be images",
                )
            }
            Self::Shader { speed, .. } => check(
                speed.is_finite() && (0.0..=4.0).contains(speed),
                "Canvas background shader speed must be between zero and four",
            ),
            // Never drawn, so there is nothing to bound. It is inert data this
            // build retains for whichever Canvas can render it.
            Self::Unsupported(raw) => check(
                raw.is_object(),
                "Unsupported Canvas background must be an object",
            ),
        }
    }
}

impl Transform3 {
    fn validate(&self) -> Result<()> {
        check(
            self.translation
                .iter()
                .chain(self.rotation.iter())
                .chain(self.scale.iter())
                .all(|value| value.is_finite()),
            "Scene transforms must be finite",
        )?;
        check(
            self.scale.iter().all(|value| *value != 0.0),
            "Scene scale components must be nonzero",
        )?;
        let rotation_length_squared: f32 = self.rotation.iter().map(|value| value * value).sum();
        check(
            rotation_length_squared > f32::EPSILON,
            "Scene rotation quaternion must be nonzero",
        )
    }
}

impl Scene {
    fn validate(&self) -> Result<()> {
        check(self.nodes.len() <= MAX_ITEMS, "Scene has too many nodes")?;
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            check(
                identifier(&node.id) && ids.insert(node.id.as_str()),
                format!("Invalid or duplicate scene node id: {}", node.id),
            )?;
            node.transform.validate()?;
            match &node.content {
                NodeContent::Group => {}
                NodeContent::Text { text } => {
                    check(visible(text), "Scene text must be printable")?;
                }
                NodeContent::Image { asset } | NodeContent::Mesh { asset } => {
                    asset.validate()?;
                }
                NodeContent::Camera { projection } => projection.validate()?,
                NodeContent::Light { light } => light.validate()?,
            }
        }
        for node in &self.nodes {
            if let Some(parent_id) = &node.parent_id {
                check(
                    parent_id != &node.id && ids.contains(parent_id.as_str()),
                    format!("Unknown or self-referential scene parent: {parent_id}"),
                )?;
                let mut cursor = Some(parent_id.as_str());
                let mut visited = BTreeSet::new();
                while let Some(id) = cursor {
                    check(visited.insert(id), "Scene graph contains a parent cycle")?;
                    cursor = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.id == id)
                        .and_then(|candidate| candidate.parent_id.as_deref());
                }
            }
        }
        Ok(())
    }
}

impl Projection {
    fn validate(self) -> Result<()> {
        match self {
            Self::Perspective {
                vertical_fov_degrees,
                near,
                far,
            } => check(
                vertical_fov_degrees.is_finite()
                    && (1.0..179.0).contains(&vertical_fov_degrees)
                    && near.is_finite()
                    && far.is_finite()
                    && near > 0.0
                    && far > near,
                "Invalid perspective projection",
            ),
            Self::Orthographic { height, near, far } => check(
                height.is_finite()
                    && near.is_finite()
                    && far.is_finite()
                    && height > 0.0
                    && far > near,
                "Invalid orthographic projection",
            ),
        }
    }
}

impl Light {
    fn validate(self) -> Result<()> {
        let (intensity, color) = match self {
            Self::Ambient { intensity } => (intensity, None),
            Self::Directional { intensity, color } | Self::Point { intensity, color } => {
                (intensity, Some(color))
            }
        };
        check(
            intensity.is_finite() && intensity >= 0.0,
            "Light intensity must be finite and nonnegative",
        )?;
        if let Some(color) = color {
            check(
                color
                    .iter()
                    .all(|component| component.is_finite() && (0.0..=1.0).contains(component)),
                "Light color components must be between zero and one",
            )?;
        }
        Ok(())
    }
}

impl WorkspaceDocument {
    pub fn parse(input: &str, catalog: &Catalog) -> Result<Self> {
        let document: Self = serde_json::from_str(input)
            .map_err(|error| Error(format!("Invalid Canvas workspace document: {error}")))?;
        document.validate(catalog)?;
        Ok(document)
    }

    pub fn resolved_theme(&self, catalog: &Catalog) -> Result<ThemeSelection> {
        self.validate(catalog)?;
        match self.theme {
            ThemeRef::Project => Ok(catalog
                .project(&self.project_id)
                .map_err(|error| Error(error.to_string()))?
                .theme),
            ThemeRef::Selection { theme } => Ok(theme),
        }
    }

    pub fn validate(&self, catalog: &Catalog) -> Result<()> {
        check(
            self.schema_version == SCHEMA_VERSION,
            "Unsupported Canvas workspace schema_version",
        )?;
        check(identifier(&self.id), "Invalid Canvas workspace id")?;
        let project = catalog
            .project(&self.project_id)
            .map_err(|error| Error(error.to_string()))?;
        check(self.revision > 0, "Workspace revision must be positive")?;
        self.background.validate()?;
        check(
            self.chrome.canvas_padding <= MAX_CANVAS_PADDING,
            format!("Canvas padding exceeds {MAX_CANVAS_PADDING}"),
        )?;
        check(
            self.chrome.surface_gap <= MAX_SURFACE_GAP,
            format!("Surface gap exceeds {MAX_SURFACE_GAP}"),
        )?;
        check(
            self.surfaces.len() <= MAX_ITEMS && self.bindings.len() <= MAX_ITEMS,
            "Workspace has too many surfaces or bindings",
        )?;

        let mut surface_ids = BTreeSet::new();
        for surface in &self.surfaces {
            check(
                identifier(&surface.id) && surface_ids.insert(surface.id.as_str()),
                format!("Invalid or duplicate surface id: {}", surface.id),
            )?;
            check(visible(&surface.title), "Surface title must be printable")?;
            let mut input_ids = BTreeSet::new();
            for input in &surface.inputs {
                check(
                    identifier(&input.id) && input_ids.insert(input.id.as_str()),
                    format!("Invalid or duplicate input id: {}", input.id),
                )?;
                check(contract(&input.contract), "Invalid input contract")?;
            }
            match &surface.content {
                SurfaceContent::ProjectResource { resource_id } => {
                    check(
                        project
                            .resources
                            .iter()
                            .any(|resource| &resource.id == resource_id),
                        format!("Unknown project resource: {resource_id}"),
                    )?;
                }
                SurfaceContent::Terminal { resource_id } => {
                    let resource = project
                        .resources
                        .iter()
                        .find(|resource| &resource.id == resource_id);
                    check(
                        resource.is_some_and(|resource| {
                            matches!(
                                resource.kind,
                                ResourceKind::TerminalSession | ResourceKind::AgentSession
                            )
                        }),
                        format!(
                            "Terminal surface requires a managed session resource: {resource_id}"
                        ),
                    )?;
                }
                SurfaceContent::Asset { asset } => asset.validate()?,
                SurfaceContent::Chart { chart } => {
                    chart.validate().map_err(|error| Error(error.to_string()))?
                }
                SurfaceContent::Widget { spec } => {
                    spec.validate().map_err(|error| Error(error.to_string()))?
                }
                SurfaceContent::Scene { scene } => scene.validate()?,
                SurfaceContent::Stream { url } => {
                    check(!url.is_empty(), "Stream URL must not be empty")?;
                }
                // Retained verbatim from a newer Canvas. There is nothing this
                // build can check about it, and refusing the whole document is
                // exactly the failure this variant exists to prevent — but it
                // must still be a JSON object so it round-trips as one.
                SurfaceContent::Unsupported(raw) => {
                    check(
                        raw.is_object(),
                        format!(
                            "Unsupported surface content must be an object: {}",
                            surface.id
                        ),
                    )?;
                }
            }
        }

        let mut placed = BTreeSet::new();
        self.layout.validate(&surface_ids, &mut placed)?;
        if let Some(focused) = &self.focused_surface_id {
            check(
                surface_ids.contains(focused.as_str()),
                format!("Unknown focused surface: {focused}"),
            )?;
        }

        let mut binding_ids = BTreeSet::new();
        for binding in &self.bindings {
            check(
                identifier(&binding.id) && binding_ids.insert(binding.id.as_str()),
                format!("Invalid or duplicate binding id: {}", binding.id),
            )?;
            check(
                identifier(&binding.source.owner)
                    && identifier(&binding.source.stream)
                    && contract(&binding.source.contract),
                "Invalid data source",
            )?;
            check(
                is_declared_contract(&binding.source.contract),
                format!(
                    "Data binding source contract {} is not declared in canvas_core::DECLARED_CONTRACTS",
                    binding.source.contract
                ),
            )?;
            if let Some(adapter) = &binding.adapter {
                check(identifier(adapter), "Invalid data adapter id")?;
            }
            if let DeliveryPolicy::Ordered {
                max_batch,
                max_age_ms,
            } = binding.delivery
            {
                check(
                    (1..=256).contains(&max_batch) && (16..=60_000).contains(&max_age_ms),
                    "Ordered delivery requires max_batch 1-256 and max_age_ms 16-60000",
                )?;
            }
            let surface = self
                .surfaces
                .iter()
                .find(|surface| surface.id == binding.target.surface_id)
                .ok_or_else(|| {
                    Error(format!(
                        "Unknown binding surface: {}",
                        binding.target.surface_id
                    ))
                })?;
            let input = surface
                .inputs
                .iter()
                .find(|input| input.id == binding.target.input_id)
                .ok_or_else(|| {
                    Error(format!(
                        "Unknown binding input: {}",
                        binding.target.input_id
                    ))
                })?;
            check(
                binding.adapter.is_some() || input.contract == binding.source.contract,
                "Binding source and input contracts differ without an adapter",
            )?;
        }
        Ok(())
    }

    pub fn apply_patch(&self, patch: &WorkspacePatch, catalog: &Catalog) -> Result<Self> {
        check(
            patch.schema_version == SCHEMA_VERSION,
            "Unsupported Canvas patch schema_version",
        )?;
        check(
            patch.workspace_id == self.id,
            "Canvas patch targets another workspace",
        )?;
        check(
            patch.base_revision == self.revision && patch.revision == self.revision + 1,
            "Canvas patch revision does not follow the current document",
        )?;
        check(
            !patch.operations.is_empty() && patch.operations.len() <= 256,
            "Canvas patch must contain 1 to 256 operations",
        )?;

        let mut next = self.clone();
        for operation in &patch.operations {
            match operation {
                PatchOperation::UpsertSurface { surface } => {
                    upsert_by(&mut next.surfaces, surface.clone(), |candidate| {
                        &candidate.id
                    })
                }
                PatchOperation::RemoveSurface { surface_id } => {
                    next.surfaces.retain(|surface| surface.id != *surface_id);
                }
                PatchOperation::SetLayout { layout } => next.layout = layout.clone(),
                PatchOperation::SetTheme { theme } => next.theme = *theme,
                PatchOperation::SetBackground { background } => {
                    next.background.clone_from(background)
                }
                PatchOperation::SetChrome { chrome } => next.chrome = *chrome,
                PatchOperation::SetFocus { surface_id } => {
                    next.focused_surface_id.clone_from(surface_id)
                }
                PatchOperation::UpsertBinding { binding } => {
                    upsert_by(&mut next.bindings, binding.clone(), |candidate| {
                        &candidate.id
                    })
                }
                PatchOperation::RemoveBinding { binding_id } => {
                    next.bindings.retain(|binding| binding.id != *binding_id);
                }
            }
        }
        next.revision = patch.revision;
        next.validate(catalog)?;
        Ok(next)
    }
}

fn upsert_by<T, F>(values: &mut Vec<T>, value: T, id: F)
where
    F: Fn(&T) -> &String,
{
    if let Some(index) = values
        .iter()
        .position(|candidate| id(candidate) == id(&value))
    {
        values[index] = value;
    } else {
        values.push(value);
    }
}

impl Layout {
    fn validate<'a>(
        &'a self,
        surface_ids: &BTreeSet<&'a str>,
        placed: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            Self::Surface { surface_id } => place(surface_id, surface_ids, placed),
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                check(
                    ratio.is_finite() && (0.05..=0.95).contains(ratio),
                    "Split ratio must be between 0.05 and 0.95",
                )?;
                first.validate(surface_ids, placed)?;
                second.validate(surface_ids, placed)
            }
            Self::Stack {
                surface_ids: stack,
                active_surface_id,
            } => {
                check(!stack.is_empty(), "A stack must contain a surface")?;
                check(
                    stack.iter().any(|id| id == active_surface_id),
                    "Stack active surface is not in the stack",
                )?;
                for surface_id in stack {
                    place(surface_id, surface_ids, placed)?;
                }
                Ok(())
            }
            Self::Canvas { placements } => {
                check(
                    !placements.is_empty() && placements.len() <= MAX_ITEMS,
                    "A Canvas layout must contain 1 to 1024 placements",
                )?;
                for placement in placements {
                    placement.bounds.validate()?;
                    place(&placement.surface_id, surface_ids, placed)?;
                }
                Ok(())
            }
        }
    }
}

fn place<'a>(
    surface_id: &'a str,
    surface_ids: &BTreeSet<&'a str>,
    placed: &mut BTreeSet<&'a str>,
) -> Result<()> {
    check(
        surface_ids.contains(surface_id),
        format!("Layout references unknown surface: {surface_id}"),
    )?;
    check(
        placed.insert(surface_id),
        format!("Layout places a surface more than once: {surface_id}"),
    )
}

impl NormalizedRect {
    fn validate(self) -> Result<()> {
        check(
            [self.x, self.y, self.width, self.height]
                .iter()
                .all(|value| value.is_finite())
                && self.y >= 0.0
                && self.width > 0.0
                && self.height > 0.0
                && self.width <= 1.0
                && self.height <= 1.0
                && self.x < 1.0
                && self.x + self.width > 0.0
                && self.y < 1.0,
            "Canvas bounds must be a positive, reachable rectangle in normalized workspace space",
        )
    }
}

#[cfg(test)]
mod forward_compatibility_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_surface_kind_from_a_newer_canvas_degrades_instead_of_failing() {
        // The `widget` kind did exactly this to the workspace adapter: one
        // unknown tag made every control reply unparseable, not just this one.
        let raw = json!({"kind": "hologram", "spec": {"depth": 3}});
        let content: SurfaceContent = serde_json::from_value(raw.clone()).expect("degrades");
        assert!(matches!(content, SurfaceContent::Unsupported(_)));
        assert_eq!(content.kind_tag(), "hologram");
        assert!(
            content
                .unsupported_reason()
                .is_some_and(|r| r.contains("hologram"))
        );
    }

    #[test]
    fn an_unknown_widget_kind_degrades_at_the_surface_rather_than_panicking() {
        // A kind like `rate_bars` was — before this build gained it — enough
        // to stop Canvas booting at all. One tile is the blast radius now.
        let raw = json!({"kind": "widget", "spec": {
            "schema_version": 1, "id": "future", "kind": "flux_capacitor", "jigawatts": 1.21
        }});
        let content: SurfaceContent = serde_json::from_value(raw).expect("degrades");
        assert!(matches!(content, SurfaceContent::Unsupported(_)));
    }

    #[test]
    fn unsupported_content_is_written_back_exactly_as_it_arrived() {
        // Without this an older Canvas would load a newer workspace, draw a
        // placeholder, save, and destroy the widget it merely could not render.
        let raw = json!({"kind": "hologram", "spec": {"depth": 3, "nested": [1, 2]}});
        let content: SurfaceContent = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&content).unwrap(), raw);
    }

    #[test]
    fn a_malformed_known_kind_still_fails_loudly() {
        // A version boundary and a typo are different things. `stream` is a kind
        // this build owns, so a missing field is a mistake, not a newer Canvas.
        let wrong: std::result::Result<SurfaceContent, _> =
            serde_json::from_value(json!({"kind": "stream"}));
        assert!(
            wrong.is_err(),
            "a known kind must not degrade into a blank tile"
        );

        let typo: std::result::Result<SurfaceContent, _> =
            serde_json::from_value(json!({"kind": "stream", "ur1": "x"}));
        assert!(
            typo.is_err(),
            "an unknown field in a known kind is still an error"
        );
    }

    #[test]
    fn a_known_widget_kind_with_a_bad_field_is_an_error_not_a_placeholder() {
        // Proven the hard way: `rate_bars` exists in this build, so a wrong
        // field inside it is a mistake and must not be swallowed as "newer".
        let raw = json!({"kind": "widget", "spec": {
            "schema_version": 2, "id": "burn", "kind": "rate_bars", "budget": 0.5
        }});
        let parsed: std::result::Result<SurfaceContent, _> = serde_json::from_value(raw);
        assert!(
            parsed.is_err(),
            "a known widget kind must fail loudly on a bad field"
        );
    }

    #[test]
    fn content_without_a_kind_is_refused() {
        let none: std::result::Result<SurfaceContent, _> =
            serde_json::from_value(json!({"spec": {}}));
        assert!(none.is_err());
    }

    #[test]
    fn every_known_kind_round_trips_through_the_custom_deserializer() {
        // The hand-written deserializer must not quietly drop a variant the
        // derived one used to handle.
        for value in [
            json!({"kind": "project_resource", "resource_id": "code"}),
            json!({"kind": "terminal", "resource_id": "shell"}),
            json!({"kind": "stream", "url": "https://example.test/s"}),
        ] {
            let content: SurfaceContent =
                serde_json::from_value(value.clone()).expect("known kind parses");
            assert!(content.unsupported_reason().is_none(), "{value}");
            assert_eq!(serde_json::to_value(&content).unwrap(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::parse(
            r#"{
            "schema_version": 1,
            "projects": [{
                "id": "dock",
                "name": "Dock",
                "theme": {"source": "coherence", "mode": "dark", "accent": "info"},
                "resources": [
                {"id":"docs","title":"Docs","host":"mac","kind":"mac_app","target":"com.apple.Safari"},
                {"id":"shell","title":"Shell","host":"agora_one","kind":"terminal_session","target":"dock"}
                ]
            }]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn feeds_reel_workspace_is_a_valid_dock_document() {
        // Self-validating guarantee (tenet 9): the demo view that wires the
        // public-API headline feeds to the feed-headlines-v1 Lua binder parses
        // and passes the same validation a real workspace document does, so a
        // user dropping it on a dock project sees surfaces instead of a blank.
        let json = include_str!("../../../config/demo/feeds-reel.json");
        let doc: WorkspaceDocument = serde_json::from_str(json)
            .expect("feeds-reel.json must deserialize as a WorkspaceDocument");
        doc.validate(&catalog())
            .expect("feeds-reel.json must pass validation");
        assert_eq!(doc.project_id, "dock");
        assert_eq!(doc.bindings.len(), 3, "one binding per headline feed");
        assert!(
            doc.bindings
                .iter()
                .all(|b| b.adapter.as_deref() == Some("feed-headlines-v1"))
        );
    }

    fn document() -> WorkspaceDocument {
        WorkspaceDocument {
            schema_version: SCHEMA_VERSION,
            id: "dock-main".into(),
            project_id: "dock".into(),
            revision: 1,
            theme: ThemeRef::Project,
            background: crate::Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: vec![
                Surface {
                    id: "browser".into(),
                    title: "Dock docs".into(),
                    content: SurfaceContent::ProjectResource {
                        resource_id: "docs".into(),
                    },
                    inputs: vec![InputPort {
                        id: "presence".into(),
                        contract: "jarvis.presence.v1".into(),
                    }],
                },
                Surface {
                    id: "terminal".into(),
                    title: "Dock shell".into(),
                    content: SurfaceContent::Terminal {
                        resource_id: "shell".into(),
                    },
                    inputs: vec![],
                },
            ],
            layout: Layout::Split {
                axis: Axis::Horizontal,
                ratio: 0.6,
                first: Box::new(Layout::Surface {
                    surface_id: "browser".into(),
                }),
                second: Box::new(Layout::Surface {
                    surface_id: "terminal".into(),
                }),
            },
            bindings: vec![DataBinding {
                id: "presence-follow".into(),
                source: DataSource {
                    owner: "jarvis".into(),
                    stream: "presence".into(),
                    contract: "jarvis.presence.v1".into(),
                },
                target: InputTarget {
                    surface_id: "browser".into(),
                    input_id: "presence".into(),
                },
                adapter: None,
                delivery: DeliveryPolicy::default(),
            }],
            focused_surface_id: Some("browser".into()),
        }
    }

    #[test]
    fn document_round_trips_and_resolves_project_theme() {
        let catalog = catalog();
        let document = document();
        document.validate(&catalog).unwrap();
        let json = serde_json::to_string_pretty(&document).unwrap();
        let decoded = WorkspaceDocument::parse(&json, &catalog).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(
            decoded.resolved_theme(&catalog).unwrap(),
            catalog.project("dock").unwrap().theme
        );
    }

    #[test]
    fn patch_is_revision_checked_and_applied_atomically() {
        let catalog = catalog();
        let document = document();
        let patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: "dock-main".into(),
            base_revision: 1,
            revision: 2,
            operations: vec![PatchOperation::SetFocus {
                surface_id: Some("terminal".into()),
            }],
        };
        let next = document.apply_patch(&patch, &catalog).unwrap();
        assert_eq!(next.revision, 2);
        assert_eq!(next.focused_surface_id.as_deref(), Some("terminal"));

        let error = next.apply_patch(&patch, &catalog).unwrap_err();
        assert!(error.to_string().contains("revision"));
    }

    #[test]
    fn backgrounds_are_backward_compatible_validated_and_patchable() {
        let catalog = catalog();
        let document = document();
        let mut legacy = serde_json::to_value(&document).unwrap();
        legacy.as_object_mut().unwrap().remove("background");
        legacy.as_object_mut().unwrap().remove("chrome");
        let legacy = WorkspaceDocument::parse(&legacy.to_string(), &catalog).unwrap();
        assert_eq!(legacy.background, Background::Theme);
        assert_eq!(legacy.chrome.title, SurfaceTitleMode::Focused);
        assert_eq!(legacy.chrome.canvas_padding, 0);
        assert_eq!(legacy.chrome.surface_gap, 0);

        let gradient = Background::Gradient {
            angle_degrees: 125.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: [7, 12, 24, 255],
                },
                GradientStop {
                    offset: 0.55,
                    color: [20, 52, 72, 255],
                },
                GradientStop {
                    offset: 1.0,
                    color: [62, 31, 90, 255],
                },
            ],
        };
        let patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: document.id.clone(),
            base_revision: document.revision,
            revision: document.revision + 1,
            operations: vec![PatchOperation::SetBackground {
                background: gradient.clone(),
            }],
        };
        let applied = document.apply_patch(&patch, &catalog).unwrap();
        assert_eq!(applied.background, gradient);
        let encoded = serde_json::to_string(&applied).unwrap();
        assert_eq!(
            WorkspaceDocument::parse(&encoded, &catalog).unwrap(),
            applied
        );

        let chrome_patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: applied.id.clone(),
            base_revision: applied.revision,
            revision: applied.revision + 1,
            operations: vec![PatchOperation::SetChrome {
                chrome: SurfaceChrome {
                    title: SurfaceTitleMode::Always,
                    ..SurfaceChrome::default()
                },
            }],
        };
        assert_eq!(
            applied
                .apply_patch(&chrome_patch, &catalog)
                .unwrap()
                .chrome
                .title,
            SurfaceTitleMode::Always
        );

        let mut invalid_spacing = applied.clone();
        invalid_spacing.chrome.canvas_padding = MAX_CANVAS_PADDING + 1;
        assert!(
            invalid_spacing
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("padding")
        );

        let mut invalid = applied.clone();
        invalid.background = Background::Gradient {
            angle_degrees: 45.0,
            stops: vec![
                GradientStop {
                    offset: 0.7,
                    color: [0, 0, 0, 255],
                },
                GradientStop {
                    offset: 0.3,
                    color: [255, 255, 255, 255],
                },
            ],
        };
        assert!(
            invalid
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("ordered")
        );
        // A shader_id this build does not know is a version boundary, not an
        // instruction: it is retained inert and never drawn, exactly as an
        // unknown widget kind is. The property that matters is unchanged --
        // nothing outside BackgroundShader::ALL can ever be rendered -- but it
        // now costs one undrawn background instead of every client's ability to
        // parse a control reply, which is what `star-nest` actually cost.
        let mut encoded = serde_json::to_value(&invalid).unwrap();
        encoded["background"] = serde_json::json!({
            "kind": "shader",
            "shader_id": "downloaded-code",
            "speed": 1.0
        });
        let parsed = serde_json::from_value::<WorkspaceDocument>(encoded).unwrap();
        assert!(matches!(parsed.background, Background::Unsupported(_)));
        assert!(!parsed.background.is_animated());
        // Retained verbatim, so an older binary writing this document back does
        // not destroy a newer Canvas's setting.
        assert_eq!(
            serde_json::to_value(&parsed.background).unwrap()["shader_id"],
            "downloaded-code"
        );
    }

    #[test]
    fn only_positive_speed_shader_backgrounds_are_animated() {
        assert!(!Background::Theme.is_animated());
        assert!(
            !Background::Shader {
                shader_id: BackgroundShader::Tidepool,
                speed: 0.0,
            }
            .is_animated()
        );
        assert!(
            Background::Shader {
                shader_id: BackgroundShader::Tidepool,
                speed: 0.75,
            }
            .is_animated()
        );
    }

    #[test]
    fn dangling_layout_and_binding_contracts_fail_loudly() {
        let catalog = catalog();
        let mut dangling = document();
        dangling.layout = Layout::Surface {
            surface_id: "missing".into(),
        };
        assert!(
            dangling
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("unknown surface")
        );

        let mut doc = document();
        doc.bindings[0].source.contract = "canvas.widget.snapshot.v1".into();
        assert!(
            doc.validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("contracts differ")
        );

        // An undeclared contract is rejected at ingress, naming the contract.
        let mut undeclared = document();
        undeclared.bindings[0].source.contract = "jarvis.attention.v2".into();
        assert!(
            undeclared
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("not declared"),
            "undeclared contract should fail the registry check"
        );
    }

    #[test]
    fn terminal_surface_cannot_embed_or_repurpose_a_process() {
        let catalog = catalog();
        let mut document = document();
        document.surfaces[1].content = SurfaceContent::Terminal {
            resource_id: "docs".into(),
        };
        assert!(
            document
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("managed session")
        );
    }

    #[test]
    fn malformed_spatial_data_and_assets_are_rejected() {
        let catalog = catalog();
        let mut document = document();
        document.surfaces.push(Surface {
            id: "scene".into(),
            title: "Scene".into(),
            content: SurfaceContent::Scene {
                scene: Scene {
                    nodes: vec![SceneNode {
                        id: "mesh".into(),
                        parent_id: None,
                        transform: Transform3 {
                            scale: [0.0, 1.0, 1.0],
                            ..Transform3::default()
                        },
                        content: NodeContent::Mesh {
                            asset: AssetRef {
                                sha256: "not-a-digest".into(),
                                media_type: "model/gltf-binary".into(),
                                byte_length: 10,
                            },
                        },
                    }],
                },
            },
            inputs: vec![],
        });
        assert!(
            document
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("scale")
        );

        let SurfaceContent::Scene { scene } = &mut document.surfaces[2].content else {
            unreachable!();
        };
        scene.nodes[0].transform = Transform3::default();
        assert!(
            document
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("sha256")
        );
    }

    #[test]
    fn unsupported_versions_and_unknown_fields_are_rejected() {
        let catalog = catalog();
        let mut document = document();
        document.schema_version = 2;
        assert!(
            document
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("schema_version")
        );

        let json = serde_json::to_string(&WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: "dock-main".into(),
            base_revision: 1,
            revision: 2,
            operations: vec![PatchOperation::SetTheme {
                theme: ThemeRef::Project,
            }],
        })
        .unwrap()
        .replace("\"revision\":2", "\"revision\":2,\"surprise\":true");
        assert!(serde_json::from_str::<WorkspacePatch>(&json).is_err());
    }

    #[test]
    fn workspace_can_override_the_project_theme_without_changing_the_catalog() {
        let catalog = catalog();
        let mut document = document();
        let selected =
            ThemeSelection::coherence(canvas_core::ThemeMode::Light, canvas_core::Accent::Success);
        document.theme = ThemeRef::Selection { theme: selected };
        let json = serde_json::to_string(&document).unwrap();
        let restored = WorkspaceDocument::parse(&json, &catalog).unwrap();
        assert_eq!(restored.resolved_theme(&catalog).unwrap(), selected);
        assert_eq!(
            catalog.project("dock").unwrap().theme,
            ThemeSelection::coherence(canvas_core::ThemeMode::Dark, canvas_core::Accent::Info)
        );
    }

    #[test]
    fn removal_requires_explicitly_repairing_references() {
        let catalog = catalog();
        let document = document();
        let patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: document.id.clone(),
            base_revision: 1,
            revision: 2,
            operations: vec![PatchOperation::RemoveSurface {
                surface_id: "browser".into(),
            }],
        };
        assert!(document.apply_patch(&patch, &catalog).is_err());
        assert_eq!(document.surfaces.len(), 2);
    }

    #[test]
    fn canvas_placements_allow_offscreen_content_but_require_reachability() {
        assert!(
            NormalizedRect {
                x: -0.6,
                y: 0.8,
                width: 0.7,
                height: 0.5,
            }
            .validate()
            .is_ok()
        );
        assert!(
            NormalizedRect {
                x: -0.8,
                y: 0.0,
                width: 0.7,
                height: 0.5,
            }
            .validate()
            .is_err()
        );
        assert!(
            NormalizedRect {
                x: 0.1,
                y: -0.01,
                width: 0.7,
                height: 0.5,
            }
            .validate()
            .is_err()
        );
    }
}
