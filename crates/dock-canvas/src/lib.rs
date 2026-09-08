//! Renderer-independent Canvas workspace documents.
//!
//! This crate owns durable presentation intent. Renderers own decoded assets,
//! frame state, GPU allocations, windows, and other reconstructible state.

use dock_core::{Catalog, ResourceKind, ThemeSelection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub mod backend;
pub mod chart;
pub mod control;
pub mod hud;
pub mod interaction;
#[cfg(feature = "lua-scenes")]
pub mod lua_module;
#[cfg(feature = "lua-scenes")]
pub mod lua_scene;
pub mod map;
#[cfg(all(feature = "mcp", unix))]
pub mod mcp;
pub mod media;
pub mod reconcile;
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl BackgroundShader {
    pub const ALL: [Self; 7] = [
        Self::Tidepool,
        Self::AuroraDrift,
        Self::DuskEmber,
        Self::SlowRipple,
        Self::Spirales,
        Self::FloatingRectangles,
        Self::SmokeRings,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum SurfaceContent {
    ProjectResource { resource_id: String },
    Asset { asset: AssetRef },
    Chart { chart: chart::ChartDocument },
    Scene { scene: Scene },
    Terminal { resource_id: String },
    Stream { url: String },
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
                SurfaceContent::Scene { scene } => scene.validate()?,
                SurfaceContent::Stream { url } => {
                    check(!url.is_empty(), "Stream URL must not be empty")?;
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
                input.contract == binding.source.contract,
                "Binding source and input contracts differ",
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
        let mut encoded = serde_json::to_value(&invalid).unwrap();
        encoded["background"] = serde_json::json!({
            "kind": "shader",
            "shader_id": "downloaded-code",
            "speed": 1.0
        });
        assert!(serde_json::from_value::<WorkspaceDocument>(encoded).is_err());
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

        let mut document = document();
        document.bindings[0].source.contract = "jarvis.attention.v2".into();
        assert!(
            document
                .validate(&catalog)
                .unwrap_err()
                .to_string()
                .contains("contracts differ")
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
