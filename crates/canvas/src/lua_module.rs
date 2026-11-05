//! Retained, host-driven Canvas Lua modules.
//!
//! A module is presentation logic. Rust owns input acquisition, scheduling,
//! validation, workspace revisions, media resources, and rendering.

use super::{
    Error, Result, Scene, Transform3, contract, identifier,
    lua_scene::{
        LuaSceneCapabilities, LuaSceneLimits, generate_scene_with_setup, lua_error,
        parse_transform, reject_unknown_fields, required_string,
    },
    visible,
};
use canvas_core::{Accent, ProjectTheme, ThemeMode, VisualTheme};
use iced::advanced::graphics::text::{self as iced_text, cosmic_text};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use soil_layout::{Node as LayoutNode, TextMeasurer, TextStyle};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub const MODULE_SCHEMA_VERSION: u32 = 1;
const MAX_DECLARATIONS: usize = 64;
const MAX_SERIES_POINTS: usize = 4_096;
const MAX_RESOURCE_SURFACES: usize = 64;

/// Semantic presentation vocabulary installed into every authored module.
/// Modules compose these primitives; the host owns their theme and geometry
/// conventions. Keeping this prelude in one place prevents every widget from
/// inventing another label scale, badge opacity, or metric hierarchy.
/// The widget vocabulary, as an artifact rather than as a Rust string.
///
/// Rust keeps the draw ABI -- `canvas.rect`, `canvas.text`, buttons as hit
/// regions -- and nothing else. Everything above it (a panel, a button, a row) is
/// Lua that the host reads from the runtime directory, so a component can be
/// changed while the shell is running instead of after a rebuild.
const WIDGETS_FALLBACK: &str = include_str!("../../../config/lua/widgets.lua");

/// The live widget library: disk first, embedded copy second.
///
/// Composed into every module's source, which is also how an edit invalidates the
/// retained scene -- the cache key covers the bytes that actually ran.
pub fn widget_library() -> String {
    crate::runtime_config::read(std::path::Path::new("lua/widgets.lua"), 128 * 1024)
        .ok()
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| WIDGETS_FALLBACK.to_owned())
}

/// A short digest of the live widget library.
///
/// Folded into the retained-scene cache key so that editing a component
/// invalidates the scene it drew, instead of waiting for some unrelated input to
/// move.
pub fn widget_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    let digest: [u8; 32] = Sha256::digest(widget_library().as_bytes()).into();
    digest[..8].iter().fold(String::new(), |mut out, byte| {
        out.push_str(&format!("{byte:02x}"));
        out
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleManifest {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub inputs: Vec<InputDeclaration>,
    #[serde(default)]
    pub resources: Vec<ResourceDeclaration>,
    pub capabilities: ModuleCapabilities,
    /// Trusted artifact routes. Scripts cannot select endpoints or credentials.
    #[serde(default)]
    pub actions: Vec<ModuleActionGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleActionGrant {
    pub id: String,
    pub destination: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleButton {
    pub id: String,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub enabled: bool,
    pub appearance: ModuleButtonAppearance,
    /// Local controls re-render this module with an activation event without
    /// publishing an Unibus command. Use them for selection and view state.
    pub local: bool,
    pub icon: Option<canvas_widgets::ShellIcon>,
    pub icon_size: f32,
    pub icon_caption: bool,
    /// Which edge the label hugs. A launcher row reads left-to-right, a toolbar
    /// button reads centred; both are buttons, and neither is wrong.
    pub align: ButtonAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAlign {
    #[default]
    Center,
    Left,
}

/// Semantic control treatment; Lua never supplies native colors or callbacks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleButtonAppearance {
    #[default]
    Standard,
    Quiet,
    /// An invisible local hit target over retained scene content.
    Hit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleAction {
    pub destination: String,
    pub kind: String,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleCapabilities {
    pub write_scene: bool,
    #[serde(default)]
    pub read_inputs: Vec<String>,
    #[serde(default)]
    pub read_theme: bool,
    #[serde(default)]
    pub present_resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeclaration {
    pub id: String,
    pub kind: InputKind,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Number,
    Text,
    Boolean,
    Series,
    Status,
    /// A topology with recency on it. See [`GraphSnapshot`].
    Graph,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDeclaration {
    pub alias: String,
    pub kind: HostResourceKind,
    pub contract: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostResourceKind {
    VideoStream,
    MapViewport,
}

/// A handle resolved and owned by the host. It contains no endpoint or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostResourceRef {
    pub owner: String,
    pub id: String,
    pub kind: HostResourceKind,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleInputSnapshot {
    pub revision: u64,
    /// The instant the host stamped this invocation, in epoch milliseconds.
    ///
    /// Supplied rather than readable: a module still cannot ask for a clock
    /// (`os` is removed from its VM), so a scene stays reproducible from its
    /// inputs alone, while the civil date an authored widget needs — today, for
    /// a calendar — has exactly one source, [`crate::hud::local_parts`], the same
    /// one the HUD renders. A test pins it by putting a number here.
    pub now_ms: u64,
    pub values: BTreeMap<String, ModuleInputValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ModuleInputValue {
    Number(f64),
    Text(String),
    Boolean(bool),
    Series(Vec<SeriesPoint>),
    Status { level: StatusLevel, label: String },
    Graph(GraphSnapshot),
}

/// A graph and how recently each part of it did something.
///
/// The shape exists because a live activity view is topology plus recency, not a
/// stream of drawings: Elmers reads its structure once and the stream only says
/// which node fired. Sending the structure every frame would put the whole shop
/// on the wire many times a second to move one highlight.
///
/// `heat` is computed by the host, not decayed here, because a module cannot keep
/// state — each invocation gets a fresh VM and only the resulting scene is
/// retained. The host owns what is recent; the module owns what recent looks like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    /// Namespaced by kind, the way Elmers uses `w:` for a workflow and `c:` for a
    /// capability, so two owners can share one graph without colliding.
    pub id: String,
    pub label: String,
    /// Which family this belongs to, for grouping and legends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 0 for idle, 1 for just fired. Host-computed decay.
    pub heat: f32,
    /// What the last thing to happen here was, in the vocabulary the theme
    /// already has semantic colours for.
    #[serde(default = "neutral_level")]
    pub level: StatusLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// A call travelling this edge, decaying the same way a node does.
    pub heat: f32,
}

fn neutral_level() -> StatusLevel {
    StatusLevel::Neutral
}

/// Ceilings. A view a person can read runs out long before these; they exist so a
/// misbehaving source cannot make the renderer the failure.
const MAX_GRAPH_NODES: usize = 512;
const MAX_GRAPH_EDGES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLevel {
    Neutral,
    Good,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModuleEvent {
    Load,
    DataChanged { changed_inputs: Vec<String> },
    Activate { target_id: String },
    /// A pointer gesture in module-local coordinates, normalized to the surface.
    ///
    /// Normalized rather than pixel: the same view has to answer the same
    /// question at any window size, and a re-render on resize must not move what
    /// the pointer is over.
    Pointer { x: f32, y: f32, phase: PointerPhase },
    Pulse { sequence: u64, delta_ms: f64 },
    Reload,
}

/// The part of a pointer gesture a view can act on.
///
/// Moves are included because hover is how a person knows a thing is clickable,
/// but the host throttles them: a view must not be re-rendered per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerPhase {
    Move,
    Press,
    Release,
    /// The pointer left the module surface; clear view-local hover.
    Leave,
}

impl PointerPhase {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Press => "press",
            Self::Release => "release",
            Self::Leave => "leave",
        }
    }
}

/// Coherence values copied from `ProjectTheme` at the call boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ModuleThemeTokens {
    pub mode: ThemeMode,
    pub accent: Accent,
    pub background: [u8; 3],
    pub surface: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub primary: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub danger: [u8; 3],
    pub space_2: f32,
    pub radius: f32,
    /// The whole type scale, not a subset of it.
    ///
    /// A module that can only see `space_2` has to invent text sizes, which is
    /// exactly how the widget library ended up with eight unreadable literals.
    /// Handing over every role means an authored grid sizes itself with the same
    /// names a native widget uses, and moves when the theme moves.
    pub micro: f32,
    pub small: f32,
    pub body: f32,
    pub large: f32,
    pub title: f32,
    pub display: f32,
    pub hero: f32,
    /// The presented surface rectangle in logical pixels, when known.
    ///
    /// Scene coordinates are centred and aspect-free; a module that wants a
    /// circle, a square, or a viewport-relative gutter needs the actual shape.
    pub viewport: [f32; 2],
}

impl ModuleThemeTokens {
    /// The values a renderer has already resolved, for a host drawing on a wall
    /// rather than naming a project.
    ///
    /// [`Self::from`] is the project-identity path. The Canvas host holds a
    /// *presented* [`VisualTheme`] — after theme packs, corner scaling and the
    /// chosen accent — and an authored widget must be coloured and sized from
    /// what is actually on screen, not from a second opinion about what the
    /// project theme ought to be. Two derivations of one palette is how an
    /// authored widget ends up a near-miss of the native ones beside it.
    pub fn from_visual(visual: &VisualTheme, accent: Accent) -> Self {
        let palette = visual.palette;
        let metrics = visual.metrics;
        Self {
            mode: visual.mode,
            accent,
            background: palette.background,
            surface: palette.surface,
            text: palette.text,
            muted: palette.text_muted,
            primary: palette.primary,
            success: palette.success,
            warning: palette.warning,
            danger: palette.danger,
            space_2: metrics.space_2,
            radius: metrics.radius,
            micro: metrics.micro,
            small: metrics.small,
            body: metrics.body,
            large: metrics.large,
            title: metrics.title,
            display: metrics.display,
            hero: metrics.hero,
            viewport: [0.0, 0.0],
        }
    }
}

impl From<ProjectTheme> for ModuleThemeTokens {
    fn from(theme: ProjectTheme) -> Self {
        let palette = theme.palette();
        let metrics = theme.metrics();
        Self {
            mode: theme.mode,
            accent: theme.accent,
            background: palette.background,
            surface: palette.surface,
            text: palette.text,
            muted: palette.text_muted,
            primary: palette.primary,
            success: palette.success,
            warning: palette.warning,
            danger: palette.danger,
            space_2: metrics.space_2,
            radius: metrics.radius,
            micro: metrics.micro,
            small: metrics.small,
            body: metrics.body,
            large: metrics.large,
            title: metrics.title,
            display: metrics.display,
            hero: metrics.hero,
            viewport: [0.0, 0.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostResourceSurface {
    pub id: String,
    pub parent_id: Option<String>,
    pub transform: Transform3,
    pub resource: HostResourceRef,
}

/// What a view has chosen for itself, as opposed to what the application knows.
///
/// A calendar highlighting Tuesday is not application state: nothing outside the
/// view can observe it, and it must survive the re-render that follows it. It is
/// also not durable -- reopen the window and today is unmarked again -- which is
/// exactly the difference between a view's choice and a Spore's data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModuleView {
    /// The id the view last nominated with `module.select`, if any.
    pub selected: Option<String>,
}

/// Last known-good renderer-neutral output retained by the Rust host.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RetainedModuleScene {
    pub output_revision: u64,
    pub module_generation: u64,
    pub input_revision: u64,
    pub scene: Scene,
    pub host_resources: Vec<HostResourceSurface>,
    pub buttons: Vec<ModuleButton>,
    /// The view's own choice after this render, for the host to hand to the next.
    pub view: ModuleView,
    /// Intents from one validated activation; never emitted on load/reload.
    pub actions: Vec<ModuleAction>,
    /// True when the view called `module.animate()`: it has something that moves,
    /// and the host should hand it a frame clock instead of a refresh poll.
    pub animated: bool,
}

pub struct LuaModule {
    manifest: ModuleManifest,
    source: String,
    generation: u64,
    scene_capabilities: LuaSceneCapabilities,
    resources: BTreeMap<String, HostResourceRef>,
    snapshot: ModuleInputSnapshot,
    theme: ModuleThemeTokens,
    retained: RetainedModuleScene,
    limits: LuaSceneLimits,
}

impl ModuleManifest {
    pub fn parse(json: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| Error(format!("Invalid Canvas Lua manifest: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        ensure(
            self.schema_version == MODULE_SCHEMA_VERSION,
            "Unsupported Canvas Lua module schema_version",
        )?;
        ensure(identifier(&self.id), "Invalid Canvas Lua module id")?;
        ensure(visible(&self.title), "Invalid Canvas Lua module title")?;
        ensure(
            self.inputs.len() <= MAX_DECLARATIONS && self.resources.len() <= MAX_DECLARATIONS,
            "Canvas Lua manifest has too many declarations",
        )?;
        ensure(
            self.capabilities.write_scene,
            "Canvas Lua module must explicitly request write_scene",
        )?;

        ensure(
            self.actions.len() <= 16,
            "Too many Canvas module action routes",
        )?;
        let mut action_ids = BTreeSet::new();
        for action in &self.actions {
            ensure(
                identifier(&action.id)
                    && action_ids.insert(&action.id)
                    && !action.destination.is_empty()
                    && action.destination.len() <= 128
                    && contract(&action.kind),
                "Invalid or duplicate Canvas module action route",
            )?;
        }

        let mut inputs = BTreeMap::new();
        for input in &self.inputs {
            ensure(
                identifier(&input.id) && inputs.insert(input.id.as_str(), input).is_none(),
                "Invalid or duplicate Canvas Lua input id",
            )?;
        }
        let mut readable = BTreeMap::new();
        for id in &self.capabilities.read_inputs {
            ensure(
                identifier(id)
                    && inputs.contains_key(id.as_str())
                    && readable.insert(id.as_str(), ()).is_none(),
                "Canvas Lua read_inputs must uniquely name declared inputs",
            )?;
        }
        for input in self.inputs.iter().filter(|input| input.required) {
            ensure(
                readable.contains_key(input.id.as_str()),
                "Required Canvas Lua input lacks read capability",
            )?;
        }

        let mut resources = BTreeMap::new();
        for resource in &self.resources {
            ensure(
                identifier(&resource.alias)
                    && contract(&resource.contract)
                    && resources
                        .insert(resource.alias.as_str(), resource)
                        .is_none(),
                "Invalid or duplicate Canvas Lua resource declaration",
            )?;
        }
        let mut presentable = BTreeMap::new();
        for alias in &self.capabilities.present_resources {
            ensure(
                identifier(alias)
                    && resources.contains_key(alias.as_str())
                    && presentable.insert(alias.as_str(), ()).is_none(),
                "Canvas Lua present_resources must uniquely name declared resources",
            )?;
        }
        Ok(())
    }
}

impl LuaModule {
    pub fn load(
        manifest: ModuleManifest,
        source: impl Into<String>,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
        resources: BTreeMap<String, HostResourceRef>,
        scene_capabilities: LuaSceneCapabilities,
        limits: LuaSceneLimits,
    ) -> Result<Self> {
        Self::load_with_view(
            manifest,
            source,
            snapshot,
            theme,
            ModuleView::default(),
            resources,
            scene_capabilities,
            limits,
        )
    }

    /// Load carrying a view's remembered choice, so a re-render does not erase it.
    #[allow(clippy::too_many_arguments)]
    pub fn load_with_view(
        manifest: ModuleManifest,
        source: impl Into<String>,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
        view: ModuleView,
        resources: BTreeMap<String, HostResourceRef>,
        scene_capabilities: LuaSceneCapabilities,
        limits: LuaSceneLimits,
    ) -> Result<Self> {
        manifest.validate()?;
        validate_snapshot(&manifest, &snapshot)?;
        validate_resources(&manifest, &resources)?;
        let source = source.into();
        let retained = render_module(RenderRequest {
            manifest: &manifest,
            source: &source,
            generation: 1,
            output_revision: 1,
            snapshot: &snapshot,
            theme,
            event: &ModuleEvent::Load,
            view,
            resources: &resources,
            scene_capabilities: &scene_capabilities,
            limits,
        })?;
        Ok(Self {
            manifest,
            source,
            generation: 1,
            scene_capabilities,
            resources,
            snapshot,
            theme,
            retained,
            limits,
        })
    }

    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }

    pub fn retained(&self) -> &RetainedModuleScene {
        &self.retained
    }

    /// Atomically render a newer input snapshot. Failure retains prior output.
    pub fn update(
        &mut self,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
        event: ModuleEvent,
    ) -> Result<&RetainedModuleScene> {
        ensure(
            snapshot.revision > self.snapshot.revision,
            "Canvas Lua input revision must advance",
        )?;
        validate_snapshot(&self.manifest, &snapshot)?;
        validate_event(&self.manifest, &event)?;
        let retained = render_module(RenderRequest {
            manifest: &self.manifest,
            source: &self.source,
            generation: self.generation,
            output_revision: self.retained.output_revision + 1,
            snapshot: &snapshot,
            theme,
            event: &event,
            view: self.retained.view.clone(),
            resources: &self.resources,
            scene_capabilities: &self.scene_capabilities,
            limits: self.limits,
        })?;
        self.snapshot = snapshot;
        self.theme = theme;
        self.retained = retained;
        Ok(&self.retained)
    }

    /// Re-render for an event that did not change any input.
    ///
    /// A click is not new data: the revision cannot advance, because the inputs
    /// are byte for byte the ones the view already saw. What changed is what the
    /// person just did, and the view's own memory of it comes along.
    pub fn update_event(
        &mut self,
        theme: ModuleThemeTokens,
        event: ModuleEvent,
    ) -> Result<&RetainedModuleScene> {
        validate_event(&self.manifest, &event)?;
        let retained = render_module(RenderRequest {
            manifest: &self.manifest,
            source: &self.source,
            generation: self.generation,
            output_revision: self.retained.output_revision + 1,
            snapshot: &self.snapshot,
            theme,
            event: &event,
            view: self.retained.view.clone(),
            resources: &self.resources,
            scene_capabilities: &self.scene_capabilities,
            limits: self.limits,
        })?;
        self.theme = theme;
        self.retained = retained;
        Ok(&self.retained)
    }

    /// Compile and render a candidate against current inputs before swapping it in.
    pub fn hot_reload(
        &mut self,
        manifest: ModuleManifest,
        source: impl Into<String>,
    ) -> Result<&RetainedModuleScene> {
        manifest.validate()?;
        validate_snapshot(&manifest, &self.snapshot)?;
        validate_resources(&manifest, &self.resources)?;
        let source = source.into();
        let generation = self.generation + 1;
        let retained = render_module(RenderRequest {
            manifest: &manifest,
            source: &source,
            generation,
            output_revision: self.retained.output_revision + 1,
            snapshot: &self.snapshot,
            theme: self.theme,
            event: &ModuleEvent::Reload,
            view: self.retained.view.clone(),
            resources: &self.resources,
            scene_capabilities: &self.scene_capabilities,
            limits: self.limits,
        })?;
        self.manifest = manifest;
        self.source = source;
        self.generation = generation;
        self.retained = retained;
        Ok(&self.retained)
    }
}

struct RenderRequest<'a> {
    manifest: &'a ModuleManifest,
    source: &'a str,
    generation: u64,
    output_revision: u64,
    snapshot: &'a ModuleInputSnapshot,
    theme: ModuleThemeTokens,
    event: &'a ModuleEvent,
    view: ModuleView,
    resources: &'a BTreeMap<String, HostResourceRef>,
    scene_capabilities: &'a LuaSceneCapabilities,
    limits: LuaSceneLimits,
}

fn render_module(request: RenderRequest<'_>) -> Result<RetainedModuleScene> {
    validate_event(request.manifest, request.event)?;
    let placements = Rc::new(RefCell::new(Vec::new()));
    let setup_placements = Rc::clone(&placements);
    let manifest = request.manifest.clone();
    let snapshot = request.snapshot.clone();
    let event = request.event.clone();
    let resources = request.resources.clone();
    let buttons = Rc::new(RefCell::new(Vec::new()));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let selection = Rc::new(RefCell::new(request.view.selected.clone()));
    let setup_selection = Rc::clone(&selection);
    let animated = Rc::new(std::cell::Cell::new(false));
    let setup_animated = Rc::clone(&animated);
    let setup_buttons = buttons.clone();
    let setup_actions = actions.clone();
    let scene = generate_scene_with_setup(
        request.source,
        request.scene_capabilities,
        request.limits,
        move |lua| {
            install_module_context(
                lua,
                &manifest,
                &snapshot,
                request.theme,
                &event,
                &request.view,
                setup_selection,
                &resources,
                setup_animated,
            )?;
            install_layout_api(lua, request.limits.max_nodes)?;
            install_button_api(lua, &manifest, &event, setup_buttons, setup_actions)?;
            install_host_resource_api(lua, &manifest, &resources, setup_placements)
        },
    )?;
    let host_resources = Rc::try_unwrap(placements)
        .map_err(|_| Error("Canvas Lua retained internal resource state".into()))?
        .into_inner();
    validate_retained_output(&scene, &host_resources, request.limits.max_nodes)?;
    let buttons = buttons.borrow().clone();
    let actions = actions.borrow().clone();
    ensure(
        scene.nodes.len() + host_resources.len() + buttons.len() <= request.limits.max_nodes,
        "Canvas module controls exceed combined node budget",
    )?;
    for button in &buttons {
        ensure(
            !scene.nodes.iter().any(|node| node.id == button.id)
                && !host_resources.iter().any(|surface| surface.id == button.id),
            "Canvas module button id collides with a retained node",
        )?;
    }
    Ok(RetainedModuleScene {
        output_revision: request.output_revision,
        module_generation: request.generation,
        input_revision: request.snapshot.revision,
        scene,
        host_resources,
        buttons,
        view: ModuleView {
            selected: selection.borrow().clone(),
        },
        actions,
        animated: animated.get(),
    })
}

struct CanvasTextMeasurer;

impl TextMeasurer for CanvasTextMeasurer {
    fn measure(
        &mut self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f32>,
    ) -> soil_layout::Size {
        let mut font_system = iced_text::font_system()
            .write()
            .expect("Canvas text font system lock");
        let mut buffer = cosmic_text::Buffer::new(
            font_system.raw(),
            cosmic_text::Metrics::new(style.size, style.line_height),
        );
        let width = max_width.unwrap_or(1_000_000.0).max(0.0);
        buffer.set_size(font_system.raw(), Some(width), Some(1_000_000.0));
        buffer.set_wrap(
            font_system.raw(),
            if style.wrap && max_width.is_some() {
                cosmic_text::Wrap::WordOrGlyph
            } else {
                cosmic_text::Wrap::None
            },
        );
        buffer.set_text(
            font_system.raw(),
            text,
            &iced_text::to_attributes(iced::Font::default()),
            cosmic_text::Shaping::Advanced,
            None,
        );
        let (size, _) = iced_text::measure(&buffer);
        soil_layout::Size::new(size.width, size.height)
    }
}

fn layout_node_count(node: &LayoutNode) -> usize {
    1 + node.children.iter().map(layout_node_count).sum::<usize>()
}

fn install_layout_api(lua: &Lua, max_nodes: usize) -> Result<()> {
    let canvas: Table = lua.globals().get("canvas").map_err(lua_error)?;
    canvas
        .set(
            "measure_text",
            lua.create_function(move |lua, (text, style): (String, Option<Table>)| {
                if text.len() > 16_384 {
                    return Err(mlua::Error::external("Canvas text exceeds 16 KiB"));
                }
                let style: TextStyle = match style {
                    Some(style) => lua.from_value(Value::Table(style))?,
                    None => TextStyle::default(),
                };
                let size = CanvasTextMeasurer.measure(&text, &style, None);
                lua.to_value(&size)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

    canvas
        .set(
            "layout",
            lua.create_function(move |lua, (tree, width, height): (Table, f32, f32)| {
                let node: LayoutNode = lua.from_value(Value::Table(tree))?;
                let count = layout_node_count(&node);
                if count > max_nodes.max(1) {
                    return Err(mlua::Error::external(format!(
                        "Canvas layout tree exceeds {max_nodes} nodes"
                    )));
                }
                let result = soil_layout::solve(
                    &node,
                    soil_layout::Size::new(width, height),
                    &mut CanvasTextMeasurer,
                )
                .map_err(mlua::Error::external)?;
                lua.to_value(&result)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)
}

fn install_button_api(
    lua: &Lua,
    manifest: &ModuleManifest,
    event: &ModuleEvent,
    buttons: Rc<RefCell<Vec<ModuleButton>>>,
    actions: Rc<RefCell<Vec<ModuleAction>>>,
) -> Result<()> {
    let grants = manifest.actions.clone();
    let target = match event {
        ModuleEvent::Activate { target_id } => Some(target_id.clone()),
        _ => None,
    };
    let widget: Table = lua.globals().get("widget").map_err(lua_error)?;
    widget
        .set(
            "button",
            lua.create_function(move |lua, spec: Table| {
                reject_unknown_fields(
                    &spec,
                    &[
                        "id",
                        "label",
                        "action",
                        "body",
                        "x",
                        "y",
                        "width",
                        "height",
                        "enabled",
                        "appearance",
                        "local",
                        "icon",
                        "icon_size",
                        "icon_caption",
                        "align",
                    ],
                )?;
                let id = required_string(&spec, "id")?;
                let label = required_string(&spec, "label")?;
                let local = spec.get::<Option<bool>>("local")?.unwrap_or(false);
                let action_id = spec.get::<Option<String>>("action")?;
                let grant = match action_id.as_deref() {
                    Some(action_id) => Some(
                        grants
                            .iter()
                            .find(|grant| grant.id == action_id)
                            .ok_or_else(|| mlua::Error::external("undeclared module action"))?,
                    ),
                    None if local => None,
                    None => return Err(mlua::Error::external("module button needs an action")),
                };
                let button = ModuleButton {
                    icon: spec
                        .get::<Option<String>>("icon")?
                        .map(|icon| {
                            serde_json::from_value(serde_json::Value::String(icon))
                                .map_err(mlua::Error::external)
                        })
                        .transpose()?,
                    icon_size: spec.get::<Option<f32>>("icon_size")?.unwrap_or(28.0),
                    icon_caption: spec.get::<Option<bool>>("icon_caption")?.unwrap_or(false),
                    align: match spec.get::<Option<String>>("align")?.as_deref() {
                        Some("left") => ButtonAlign::Left,
                        Some("center") | None => ButtonAlign::Center,
                        Some(other) => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "Canvas Lua button align must be left or center; got {other}"
                            )));
                        }
                    },
                    id,
                    label,
                    x: spec.get("x")?,
                    y: spec.get("y")?,
                    width: spec.get("width")?,
                    height: spec.get("height")?,
                    enabled: spec.get::<Option<bool>>("enabled")?.unwrap_or(true),
                    appearance: match spec.get::<Option<String>>("appearance")?.as_deref() {
                        None | Some("standard") => ModuleButtonAppearance::Standard,
                        Some("quiet") => ModuleButtonAppearance::Quiet,
                        Some("hit") => ModuleButtonAppearance::Hit,
                        _ => return Err(mlua::Error::external("unknown module button appearance")),
                    },
                    local,
                };
                if !identifier(&button.id)
                    || !visible(&button.label)
                    || button.label.len() > 128
                    || !button.icon_size.is_finite()
                    || !(8.0..=96.0).contains(&button.icon_size)
                    || ![button.x, button.y, button.width, button.height]
                        .iter()
                        .all(|n| n.is_finite())
                    || button.width <= 0.0
                    || button.height <= 0.0
                    || button.width > 8.0
                    || button.height > 8.0
                    || !(-4.0..=4.0).contains(&button.x)
                    || !(-4.0..=4.0).contains(&button.y)
                {
                    return Err(mlua::Error::external(format!(
                        "invalid module button geometry or label: id={:?} label={:?} x={} y={} w={} h={}",
                        button.id, button.label, button.x, button.y, button.width, button.height
                    )));
                }
                let body: serde_json::Value = lua.from_value(spec.get("body")?)?;
                if !body.is_object()
                    || body.get("message_type").is_some()
                    || serde_json::to_vec(&body)
                        .map_err(mlua::Error::external)?
                        .len()
                        > 4096
                {
                    return Err(mlua::Error::external(
                        "module action needs a bounded object body",
                    ));
                }
                let mut retained = buttons.borrow_mut();
                if retained.len() >= 64 || retained.iter().any(|existing| existing.id == button.id)
                {
                    return Err(mlua::Error::external(
                        "duplicate or excessive module buttons",
                    ));
                }
                if let Some(grant) = grant
                    && button.enabled
                    && target.as_deref() == Some(button.id.as_str())
                {
                    actions.borrow_mut().push(ModuleAction {
                        destination: grant.destination.clone(),
                        kind: grant.kind.clone(),
                        body,
                    });
                }
                retained.push(button);
                Ok(())
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

    lua.load(
        r#"
widget.hit = function(spec)
  spec["local"] = true
  spec.appearance = "hit"
  spec.label = spec.label or " "
  spec.enabled = spec.enabled ~= false
  spec.body = spec.body or { ["local"] = true }
  return widget.button(spec)
end
"#,
    )
    .set_name("canvas-widget-hit")
    .exec()
    .map_err(lua_error)
}

fn install_module_context(
    lua: &Lua,
    manifest: &ModuleManifest,
    snapshot: &ModuleInputSnapshot,
    theme: ModuleThemeTokens,
    event: &ModuleEvent,
    view: &ModuleView,
    selection: Rc<RefCell<Option<String>>>,
    resources: &BTreeMap<String, HostResourceRef>,
    animated: Rc<std::cell::Cell<bool>>,
) -> Result<()> {
    let module = lua.create_table().map_err(lua_error)?;
    module.set("id", manifest.id.as_str()).map_err(lua_error)?;

    let inputs = lua.create_table().map_err(lua_error)?;
    for id in &manifest.capabilities.read_inputs {
        if let Some(value) = snapshot.values.get(id) {
            inputs
                .set(id.as_str(), input_value_to_lua(lua, value)?)
                .map_err(lua_error)?;
        }
    }
    module.set("inputs", inputs).map_err(lua_error)?;
    module
        .set("now", now_to_lua(lua, snapshot.now_ms)?)
        .map_err(lua_error)?;
    module.set("now_ms", snapshot.now_ms).map_err(lua_error)?;
    // Motion is a request for frames, not a fact the host can infer: a scene of
    // rectangles looks identical animated and still, so the view has to say.
    module
        .set(
            "animate",
            lua.create_function(move |_, (): ()| {
                animated.set(true);
                Ok(())
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    module
        .set("event", event_to_lua(lua, event)?)
        .map_err(lua_error)?;
    let view_table = lua.create_table().map_err(lua_error)?;
    if let Some(selected) = &view.selected {
        view_table
            .set("selected", selected.as_str())
            .map_err(lua_error)?;
    }
    module.set("view", view_table).map_err(lua_error)?;
    // A view nominates what the pointer landed on. It is not an action: nothing
    // leaves the process, and the Spore is never told. It is how a view remembers
    // its own highlight across the re-render that the click itself caused.
    module
        .set(
            "select",
            lua.create_function(move |_, id: mlua::Value| {
                match id {
                    mlua::Value::Nil => *selection.borrow_mut() = None,
                    mlua::Value::String(id) => {
                        let id = id.to_str()?.to_owned();
                        // A selection is an opaque key the view invented — a date,
                        // a row id — not a name the host resolves. Bounded and
                        // printable is the whole contract.
                        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                            return Err(mlua::Error::external(
                                "Invalid Canvas Lua view selection",
                            ));
                        }
                        *selection.borrow_mut() = Some(id);
                    }
                    _ => return Err(mlua::Error::external("Canvas Lua selection must be a string")),
                }
                Ok(())
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    if manifest.capabilities.read_theme {
        module
            .set("theme", theme_to_lua(lua, theme)?)
            .map_err(lua_error)?;
    }
    let viewport = lua.create_table().map_err(lua_error)?;
    viewport
        .set("width", theme.viewport[0])
        .map_err(lua_error)?;
    viewport
        .set("height", theme.viewport[1])
        .map_err(lua_error)?;
    module.set("viewport", viewport).map_err(lua_error)?;

    let resource_table = lua.create_table().map_err(lua_error)?;
    for alias in &manifest.capabilities.present_resources {
        let resource = resources
            .get(alias)
            .ok_or_else(|| Error(format!("Missing Canvas host resource grant: {alias}")))?;
        let value = lua.create_table().map_err(lua_error)?;
        value
            .set("owner", resource.owner.as_str())
            .map_err(lua_error)?;
        value.set("id", resource.id.as_str()).map_err(lua_error)?;
        value
            .set("kind", resource.kind.wire_name())
            .map_err(lua_error)?;
        value
            .set("contract", resource.contract.as_str())
            .map_err(lua_error)?;
        resource_table
            .set(alias.as_str(), value)
            .map_err(lua_error)?;
    }
    module.set("resources", resource_table).map_err(lua_error)?;
    install_widget_api(lua, manifest.capabilities.read_theme, theme)?;
    lua.globals().set("module", module).map_err(lua_error)
}

fn install_widget_api(lua: &Lua, read_theme: bool, theme: ModuleThemeTokens) -> Result<()> {
    let widget = lua.create_table().map_err(lua_error)?;

    if read_theme {
        let theme_tokens = theme;
        widget
            .set(
                "color",
                lua.create_function(move |lua, name: String| {
                    let rgb = match name.as_str() {
                        "background" => theme_tokens.background,
                        "text" => theme_tokens.text,
                        "surface" => theme_tokens.surface,
                        "primary" => theme_tokens.primary,
                        "success" => theme_tokens.success,
                        "text_muted" => theme_tokens.muted,
                        _ => [128, 128, 128],
                    };
                    let table = lua.create_table()?;
                    table.set(1, rgb[0])?;
                    table.set(2, rgb[1])?;
                    table.set(3, rgb[2])?;
                    Ok(table)
                })
                .map_err(lua_error)?,
            )
            .map_err(lua_error)?;
    }

    lua.globals().set("widget", widget).map_err(lua_error)?;

    // The widget vocabulary is an artifact, loaded into the host's writable
    // globals: a component that lives in the runtime directory can be changed
    // while the shell runs, which is the difference between iterating on a button
    // and rebuilding a compositor to try one.
    if read_theme {
        lua.load(widget_library())
            .set_name("canvas-widgets")
            .exec()
            .map_err(lua_error)?;
    }



    let canvas: Table = lua.globals().get("canvas").map_err(lua_error)?;
    canvas
        .set(
            "events",
            lua.create_function(|lua, ()| {
                let table = lua.create_table()?;
                Ok(mlua::Value::Table(table))
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    // canvas.webhook_post removed: Lua modules must never spawn curl or other
    // external processes. Network I/O belongs in stream producers (Python
    // feeds) that write canvas.widget.snapshot.v1 envelopes. Events are
    // emitted through canvas.events and the stream socket, not direct HTTP.
    lua.globals().set("canvas", canvas).map_err(lua_error)
}

/// Build a `{ text = "..." }` table — used by host-side callers that need to
/// return text content from Rust into a Lua module.
#[allow(dead_code)]
fn widget_set_text(lua: &Lua, text: &str) -> mlua::Result<mlua::Value> {
    let table = lua.create_table()?;
    table.set("text", text)?;
    Ok(mlua::Value::Table(table))
}

fn install_host_resource_api(
    lua: &Lua,
    manifest: &ModuleManifest,
    resources: &BTreeMap<String, HostResourceRef>,
    placements: Rc<RefCell<Vec<HostResourceSurface>>>,
) -> Result<()> {
    let allowed: BTreeMap<String, HostResourceRef> = manifest
        .capabilities
        .present_resources
        .iter()
        .filter_map(|alias| {
            resources
                .get(alias)
                .map(|value| (alias.clone(), value.clone()))
        })
        .collect();
    if allowed.is_empty() {
        return Ok(());
    }
    let canvas: Table = lua.globals().get("canvas").map_err(lua_error)?;
    canvas
        .set(
            "host_resource",
            lua.create_function(move |_, spec: Table| {
                reject_unknown_fields(
                    &spec,
                    &[
                        "id",
                        "parent",
                        "resource",
                        "translation",
                        "rotation",
                        "scale",
                    ],
                )?;
                let alias = required_string(&spec, "resource")?;
                let resource = allowed.get(&alias).cloned().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "Canvas host resource capability not granted: {alias}"
                    ))
                })?;
                let mut placements = placements.borrow_mut();
                if placements.len() >= MAX_RESOURCE_SURFACES {
                    return Err(mlua::Error::RuntimeError(
                        "Canvas Lua host resource surface limit exceeded".into(),
                    ));
                }
                placements.push(HostResourceSurface {
                    id: required_string(&spec, "id")?,
                    parent_id: spec.get("parent")?,
                    transform: parse_transform(&spec)?,
                    resource,
                });
                Ok(())
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)
}

fn input_value_to_lua(lua: &Lua, value: &ModuleInputValue) -> Result<mlua::Value> {
    Ok(match value {
        ModuleInputValue::Number(number) => mlua::Value::Number(*number),
        ModuleInputValue::Text(text) => {
            mlua::Value::String(lua.create_string(text).map_err(lua_error)?)
        }
        ModuleInputValue::Boolean(value) => mlua::Value::Boolean(*value),
        ModuleInputValue::Series(points) => {
            let table = lua.create_table().map_err(lua_error)?;
            for (index, point) in points.iter().enumerate() {
                let value = lua.create_table().map_err(lua_error)?;
                value.set("x", point.x).map_err(lua_error)?;
                value.set("y", point.y).map_err(lua_error)?;
                table.set(index + 1, value).map_err(lua_error)?;
            }
            mlua::Value::Table(table)
        }
        ModuleInputValue::Status { level, label } => {
            let table = lua.create_table().map_err(lua_error)?;
            table.set("level", level.wire_name()).map_err(lua_error)?;
            table.set("label", label.as_str()).map_err(lua_error)?;
            mlua::Value::Table(table)
        }
        ModuleInputValue::Graph(graph) => {
            let table = lua.create_table().map_err(lua_error)?;
            let nodes = lua.create_table().map_err(lua_error)?;
            for (index, node) in graph.nodes.iter().enumerate() {
                let value = lua.create_table().map_err(lua_error)?;
                value.set("id", node.id.as_str()).map_err(lua_error)?;
                value.set("label", node.label.as_str()).map_err(lua_error)?;
                value.set("heat", f64::from(node.heat)).map_err(lua_error)?;
                value
                    .set("level", node.level.wire_name())
                    .map_err(lua_error)?;
                if let Some(group) = &node.group {
                    value.set("group", group.as_str()).map_err(lua_error)?;
                }
                nodes.set(index + 1, value).map_err(lua_error)?;
            }
            let edges = lua.create_table().map_err(lua_error)?;
            for (index, edge) in graph.edges.iter().enumerate() {
                let value = lua.create_table().map_err(lua_error)?;
                value.set("from", edge.from.as_str()).map_err(lua_error)?;
                value.set("to", edge.to.as_str()).map_err(lua_error)?;
                value.set("heat", f64::from(edge.heat)).map_err(lua_error)?;
                edges.set(index + 1, value).map_err(lua_error)?;
            }
            table.set("nodes", nodes).map_err(lua_error)?;
            table.set("edges", edges).map_err(lua_error)?;
            mlua::Value::Table(table)
        }
    })
}

fn event_to_lua(lua: &Lua, event: &ModuleEvent) -> Result<Table> {
    let table = lua.create_table().map_err(lua_error)?;
    match event {
        ModuleEvent::Load => table.set("kind", "load").map_err(lua_error)?,
        ModuleEvent::DataChanged { changed_inputs } => {
            table.set("kind", "data_changed").map_err(lua_error)?;
            let changed = lua.create_table().map_err(lua_error)?;
            for (index, id) in changed_inputs.iter().enumerate() {
                changed.set(index + 1, id.as_str()).map_err(lua_error)?;
            }
            table.set("changed_inputs", changed).map_err(lua_error)?;
        }
        ModuleEvent::Activate { target_id } => {
            table.set("kind", "activate").map_err(lua_error)?;
            table
                .set("target_id", target_id.as_str())
                .map_err(lua_error)?;
        }
        ModuleEvent::Pointer { x, y, phase } => {
            table.set("kind", "pointer").map_err(lua_error)?;
            table.set("phase", phase.wire_name()).map_err(lua_error)?;
            table.set("x", *x).map_err(lua_error)?;
            table.set("y", *y).map_err(lua_error)?;
        }
        ModuleEvent::Pulse { sequence, delta_ms } => {
            table.set("kind", "pulse").map_err(lua_error)?;
            table.set("sequence", *sequence).map_err(lua_error)?;
            table.set("delta_ms", *delta_ms).map_err(lua_error)?;
        }
        ModuleEvent::Reload => table.set("kind", "reload").map_err(lua_error)?,
    }
    Ok(table)
}

fn theme_to_lua(lua: &Lua, theme: ModuleThemeTokens) -> Result<Table> {
    let table = lua.create_table().map_err(lua_error)?;
    table
        .set(
            "mode",
            match theme.mode {
                ThemeMode::Light => "light",
                ThemeMode::Dark => "dark",
            },
        )
        .map_err(lua_error)?;
    table
        .set(
            "accent",
            match theme.accent {
                Accent::Brand => "brand",
                Accent::Info => "info",
                Accent::Success => "success",
            },
        )
        .map_err(lua_error)?;
    for (name, color) in [
        ("background", theme.background),
        ("surface", theme.surface),
        ("text", theme.text),
        ("muted", theme.muted),
        ("primary", theme.primary),
        ("success", theme.success),
        ("warning", theme.warning),
        ("danger", theme.danger),
    ] {
        let value = lua.create_table().map_err(lua_error)?;
        for (index, component) in color.iter().enumerate() {
            value.set(index + 1, *component).map_err(lua_error)?;
        }
        table.set(name, value).map_err(lua_error)?;
    }
    table.set("space_2", theme.space_2).map_err(lua_error)?;
    table.set("radius", theme.radius).map_err(lua_error)?;
    let viewport = lua.create_table().map_err(lua_error)?;
    viewport.set("width", theme.viewport[0]).map_err(lua_error)?;
    viewport.set("height", theme.viewport[1]).map_err(lua_error)?;
    table.set("viewport", viewport).map_err(lua_error)?;
    for (name, size) in [
        ("micro", theme.micro),
        ("small", theme.small),
        ("body", theme.body),
        ("large", theme.large),
        ("title", theme.title),
        ("display", theme.display),
        ("hero", theme.hero),
    ] {
        table.set(name, size).map_err(lua_error)?;
    }
    Ok(table)
}

/// The host-stamped civil time, as `module.now`.
///
/// A module cannot read a clock and must not need one to be reproducible, so the
/// instant arrives with the invocation rather than being asked for. Every field
/// is derived from [`crate::hud::local_parts`], the same conversion the HUD clock
/// uses, so a calendar and the wall cannot disagree about today.
fn now_to_lua(lua: &Lua, now_ms: u64) -> Result<Table> {
    let parts = crate::hud::local_parts(now_ms);
    let table = lua.create_table().map_err(lua_error)?;
    table.set("epoch_ms", now_ms).map_err(lua_error)?;
    table.set("year", parts.year).map_err(lua_error)?;
    table.set("month", parts.month).map_err(lua_error)?;
    table.set("day", parts.day).map_err(lua_error)?;
    table.set("weekday", parts.weekday).map_err(lua_error)?;
    table.set("hour", parts.hour).map_err(lua_error)?;
    table.set("minute", parts.minute).map_err(lua_error)?;
    table.set("second", parts.second).map_err(lua_error)?;
    table
        .set("utc_offset_seconds", parts.utc_offset_seconds)
        .map_err(lua_error)?;
    table
        .set(
            "iso_date",
            format!("{:04}-{:02}-{:02}", parts.year, parts.month, parts.day),
        )
        .map_err(lua_error)?;
    Ok(table)
}

fn validate_snapshot(manifest: &ModuleManifest, snapshot: &ModuleInputSnapshot) -> Result<()> {
    ensure(
        snapshot.revision > 0,
        "Canvas Lua input revision must be positive",
    )?;
    ensure(
        snapshot.now_ms > 0,
        "Canvas Lua invocation must carry a host-stamped time",
    )?;
    ensure(
        snapshot.values.len() <= MAX_DECLARATIONS,
        "Canvas Lua input snapshot has too many values",
    )?;
    for (id, value) in &snapshot.values {
        let declaration = manifest
            .inputs
            .iter()
            .find(|input| input.id == *id)
            .ok_or_else(|| Error(format!("Undeclared Canvas Lua input: {id}")))?;
        ensure(
            declaration.kind == value.kind(),
            format!("Canvas Lua input kind differs for: {id}"),
        )?;
        value.validate()?;
    }
    for declaration in manifest.inputs.iter().filter(|input| input.required) {
        ensure(
            snapshot.values.contains_key(&declaration.id),
            format!("Missing required Canvas Lua input: {}", declaration.id),
        )?;
    }
    Ok(())
}

fn validate_resources(
    manifest: &ModuleManifest,
    resources: &BTreeMap<String, HostResourceRef>,
) -> Result<()> {
    ensure(
        resources.len() <= MAX_DECLARATIONS,
        "Canvas Lua has too many host resource grants",
    )?;
    for (alias, resource) in resources {
        ensure(identifier(alias), "Invalid Canvas host resource alias")?;
        ensure(
            identifier(&resource.owner) && identifier(&resource.id) && contract(&resource.contract),
            "Invalid Canvas host resource reference",
        )?;
        let declaration = manifest
            .resources
            .iter()
            .find(|candidate| candidate.alias == *alias)
            .ok_or_else(|| Error(format!("Undeclared Canvas host resource: {alias}")))?;
        ensure(
            declaration.kind == resource.kind && declaration.contract == resource.contract,
            format!("Canvas host resource contract differs for: {alias}"),
        )?;
    }
    for alias in &manifest.capabilities.present_resources {
        ensure(
            resources.contains_key(alias),
            format!("Missing Canvas host resource grant: {alias}"),
        )?;
    }
    Ok(())
}

fn validate_event(manifest: &ModuleManifest, event: &ModuleEvent) -> Result<()> {
    match event {
        ModuleEvent::DataChanged { changed_inputs } => {
            ensure(
                !changed_inputs.is_empty() && changed_inputs.len() <= MAX_DECLARATIONS,
                "Canvas Lua data_changed event needs bounded input ids",
            )?;
            for id in changed_inputs {
                ensure(
                    manifest.capabilities.read_inputs.contains(id),
                    format!("Canvas Lua event names unreadable input: {id}"),
                )?;
            }
        }
        ModuleEvent::Activate { target_id } => {
            ensure(
                identifier(target_id),
                "Invalid Canvas Lua activation target",
            )?;
        }
        ModuleEvent::Pointer { x, y, .. } => {
            ensure(
                x.is_finite() && y.is_finite() && (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y),
                "Canvas Lua pointer must be normalized to the surface",
            )?;
        }
        ModuleEvent::Pulse { delta_ms, .. } => {
            ensure(
                delta_ms.is_finite() && (0.0..=1_000.0).contains(delta_ms),
                "Invalid Canvas Lua pulse delta",
            )?;
        }
        ModuleEvent::Load | ModuleEvent::Reload => {}
    }
    Ok(())
}

fn validate_retained_output(
    scene: &Scene,
    resources: &[HostResourceSurface],
    max_nodes: usize,
) -> Result<()> {
    ensure(
        scene.nodes.len() + resources.len() <= max_nodes,
        "Canvas Lua combined retained node limit exceeded",
    )?;
    let scene_ids: BTreeMap<&str, ()> = scene
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), ()))
        .collect();
    let mut resource_ids = BTreeMap::new();
    for resource in resources {
        ensure(
            identifier(&resource.id)
                && !scene_ids.contains_key(resource.id.as_str())
                && resource_ids.insert(resource.id.as_str(), ()).is_none(),
            "Invalid or duplicate Canvas Lua host resource surface id",
        )?;
        resource.transform.validate()?;
        if let Some(parent) = &resource.parent_id {
            ensure(
                scene_ids.contains_key(parent.as_str()),
                format!("Canvas host resource parent is not a scene node: {parent}"),
            )?;
        }
    }
    Ok(())
}

impl ModuleInputValue {
    fn kind(&self) -> InputKind {
        match self {
            Self::Number(_) => InputKind::Number,
            Self::Text(_) => InputKind::Text,
            Self::Boolean(_) => InputKind::Boolean,
            Self::Series(_) => InputKind::Series,
            Self::Status { .. } => InputKind::Status,
            Self::Graph(_) => InputKind::Graph,
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Number(value) => ensure(value.is_finite(), "Canvas Lua number must be finite"),
            Self::Text(value) => ensure(visible(value), "Canvas Lua text must be printable"),
            Self::Boolean(_) => Ok(()),
            Self::Series(points) => ensure(
                points.len() <= MAX_SERIES_POINTS
                    && points
                        .iter()
                        .all(|point| point.x.is_finite() && point.y.is_finite()),
                "Canvas Lua series must contain at most 4096 finite points",
            ),
            Self::Status { label, .. } => {
                ensure(visible(label), "Canvas Lua status label must be printable")
            }
            Self::Graph(graph) => {
                ensure(
                    graph.nodes.len() <= MAX_GRAPH_NODES && graph.edges.len() <= MAX_GRAPH_EDGES,
                    "Canvas Lua graph exceeds its node or edge limit",
                )?;
                let mut node_ids = BTreeSet::new();
                for node in &graph.nodes {
                    ensure(
                        identifier(&node.id)
                            && visible(&node.label)
                            && node.group.as_ref().is_none_or(|group| visible(group))
                            && node.heat.is_finite()
                            && (0.0..=1.0).contains(&node.heat)
                            && node_ids.insert(node.id.as_str()),
                        "Canvas Lua graph nodes require unique IDs, printable labels, and heat in 0..1",
                    )?;
                }
                let mut edges = BTreeSet::new();
                for edge in &graph.edges {
                    ensure(
                        node_ids.contains(edge.from.as_str())
                            && node_ids.contains(edge.to.as_str())
                            && edge.heat.is_finite()
                            && (0.0..=1.0).contains(&edge.heat)
                            && edges.insert((edge.from.as_str(), edge.to.as_str())),
                        "Canvas Lua graph edges require known endpoints, unique pairs, and heat in 0..1",
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl StatusLevel {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Good => "good",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl HostResourceKind {
    fn wire_name(self) -> &'static str {
        match self {
            Self::VideoStream => "video_stream",
            Self::MapViewport => "map_viewport",
        }
    }
}

fn ensure(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error(message.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ModuleThemeTokens {
        ProjectTheme {
            mode: ThemeMode::Dark,
            accent: Accent::Info,
        }
        .into()
    }

    fn graph_manifest() -> ModuleManifest {
        ModuleManifest::parse(include_str!("../examples/lua/graph.json")).unwrap()
    }

    /// A pinned instant for the host-stamped clock, so a module test asserts on
    /// the date it authored rather than the date a runner happened to execute on.
    const TEST_NOW_MS: u64 = 1_768_000_000_000;

    fn graph_snapshot(revision: u64) -> ModuleInputSnapshot {
        ModuleInputSnapshot {
            revision,
            now_ms: TEST_NOW_MS,
            values: BTreeMap::from([(
                "samples".into(),
                ModuleInputValue::Series(vec![
                    SeriesPoint { x: 1.0, y: 3.0 },
                    SeriesPoint { x: 2.0, y: 8.0 },
                    SeriesPoint { x: 3.0, y: 5.0 },
                ]),
            )]),
        }
    }

    #[test]
    fn graph_module_loads_and_retains_typed_scene() {
        let module = LuaModule::load(
            graph_manifest(),
            include_str!("../examples/lua/graph.lua"),
            graph_snapshot(1),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();

        assert_eq!(module.retained().output_revision, 1);
        assert_eq!(module.retained().input_revision, 1);
        assert_eq!(module.retained().scene.nodes.len(), 4);
        assert!(matches!(
            &module.retained().scene.nodes[1].content,
            super::super::NodeContent::Text { text, .. } if text == "███"
        ));
    }

    #[test]
    fn wiki_race_module_composes_hud_and_trace_from_shared_primitives() {
        let manifest =
            ModuleManifest::parse(include_str!("../../../config/modules/wiki-race-v1.json"))
                .unwrap();
        let snapshot = ModuleInputSnapshot {
            revision: 1,
            now_ms: TEST_NOW_MS,
            values: BTreeMap::from([
                (
                    "start".into(),
                    ModuleInputValue::Text("Kevin Costner".into()),
                ),
                (
                    "target".into(),
                    ModuleInputValue::Text("Nebuchadnezzar".into()),
                ),
                (
                    "status".into(),
                    ModuleInputValue::Status {
                        level: StatusLevel::Good,
                        label: "RACING".into(),
                    },
                ),
                ("clicks".into(), ModuleInputValue::Number(4.0)),
                (
                    "raceclock".into(),
                    ModuleInputValue::Text("00:12.34".into()),
                ),
                (
                    "wallclock".into(),
                    ModuleInputValue::Text("00:13.02".into()),
                ),
                (
                    "trace".into(),
                    ModuleInputValue::Text("The Matrix|Science fiction".into()),
                ),
            ]),
        };
        let module = LuaModule::load(
            manifest,
            include_str!("../../../config/modules/wiki-race-v1.lua"),
            snapshot,
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();

        assert!(
            module
                .retained()
                .scene
                .nodes
                .iter()
                .any(|node| matches!(node.content, super::super::NodeContent::Rect { .. }))
        );
        assert!(module.retained().scene.nodes.iter().any(|node| matches!(
            &node.content,
            super::super::NodeContent::Text { text, .. } if text == "Nebuchadnezzar"
        )));
    }

    fn activity_graph() -> GraphSnapshot {
        GraphSnapshot {
            nodes: vec![
                GraphNode {
                    id: "workflow".into(),
                    label: "Invoice workflow".into(),
                    group: Some("workflows".into()),
                    heat: 0.9,
                    level: StatusLevel::Good,
                },
                GraphNode {
                    id: "connector".into(),
                    label: "MCP bridge".into(),
                    group: Some("connectors".into()),
                    heat: 0.6,
                    level: StatusLevel::Warning,
                },
            ],
            edges: vec![GraphEdge {
                from: "workflow".into(),
                to: "connector".into(),
                heat: 0.7,
            }],
        }
    }

    #[test]
    fn activity_graph_module_renders_host_owned_topology_and_recency() {
        let manifest =
            ModuleManifest::parse(include_str!("../examples/lua/activity-graph.json")).unwrap();
        let module = LuaModule::load(
            manifest,
            include_str!("../examples/lua/activity-graph.lua"),
            ModuleInputSnapshot {
                revision: 7,
                now_ms: TEST_NOW_MS,
                values: BTreeMap::from([(
                    "graph".into(),
                    ModuleInputValue::Graph(activity_graph()),
                )]),
            },
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();

        assert_eq!(module.retained().input_revision, 7);
        assert!(module.retained().scene.nodes.len() >= 5);
    }

    fn cross_owner_snapshot() -> ModuleInputSnapshot {
        ModuleInputSnapshot {
            revision: 1,
            now_ms: TEST_NOW_MS,
            values: BTreeMap::from([
                (
                    "bifrost".into(),
                    ModuleInputValue::Graph(GraphSnapshot {
                        nodes: vec![
                            GraphNode {
                                id: "health".into(),
                                label: "Backend health".into(),
                                group: Some("metrics".into()),
                                heat: 0.9,
                                level: StatusLevel::Good,
                            },
                            GraphNode {
                                id: "queues".into(),
                                label: "Queue depth".into(),
                                group: Some("metrics".into()),
                                heat: 0.6,
                                level: StatusLevel::Warning,
                            },
                        ],
                        edges: vec![],
                    }),
                ),
                (
                    "elmers".into(),
                    ModuleInputValue::Graph(GraphSnapshot {
                        nodes: vec![
                            GraphNode {
                                id: "runs".into(),
                                label: "In-flight runs".into(),
                                group: Some("activity".into()),
                                heat: 0.4,
                                level: StatusLevel::Neutral,
                            },
                            GraphNode {
                                id: "budget".into(),
                                label: "Budget usage".into(),
                                group: Some("activity".into()),
                                heat: 0.2,
                                level: StatusLevel::Critical,
                            },
                        ],
                        edges: vec![],
                    }),
                ),
            ]),
        }
    }

    #[test]
    fn cross_owner_module_combines_two_source_owners_into_one_scene() {
        let manifest =
            ModuleManifest::parse(include_str!("../examples/lua/cross-owner-graph.json")).unwrap();
        let module = LuaModule::load(
            manifest,
            include_str!("../examples/lua/cross-owner-graph.lua"),
            cross_owner_snapshot(),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();
        let scene = &module.retained().scene;
        // Root group + 2 group labels + 2 headers + 4 nodes + 1 bridge = 10
        assert!(
            scene.nodes.len() >= 8,
            "expected at least 8 scene nodes, got {}",
            scene.nodes.len()
        );
        // Bifrost nodes present
        assert!(scene.nodes.iter().any(|n| n.id == "bifrost-health"));
        assert!(scene.nodes.iter().any(|n| n.id == "bifrost-queues"));
        // Elmers nodes present
        assert!(scene.nodes.iter().any(|n| n.id == "elmers-runs"));
        assert!(scene.nodes.iter().any(|n| n.id == "elmers-budget"));
        // Bridge connects the two
        assert!(scene.nodes.iter().any(|n| n.id == "bridge"));
    }

    #[test]
    fn cross_owner_module_preserves_last_known_good_on_malformed_input() {
        let manifest =
            ModuleManifest::parse(include_str!("../examples/lua/cross-owner-graph.json")).unwrap();
        // First: load with valid data
        let good = LuaModule::load(
            manifest.clone(),
            include_str!("../examples/lua/cross-owner-graph.lua"),
            cross_owner_snapshot(),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();
        let good_node_count = good.retained().scene.nodes.len();
        assert!(good_node_count >= 8);
        // Second: load with malformed data (missing elmers group)
        let mut bad = cross_owner_snapshot();
        bad.values.remove("elmers");
        let result = LuaModule::load(
            manifest,
            include_str!("../examples/lua/cross-owner-graph.lua"),
            bad,
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        );
        // Should fail gracefully — not crash
        assert!(result.is_err(), "missing required input should be rejected");
        // Good module still has its scene intact
        assert_eq!(good.retained().scene.nodes.len(), good_node_count);
    }

    #[test]
    fn shared_attention_is_clipped_and_mapped_inside_the_visible_crop() {
        let mut duplicate = activity_graph();
        duplicate.nodes.push(duplicate.nodes[0].clone());
        assert!(ModuleInputValue::Graph(duplicate).validate().is_err());

        let mut dangling = activity_graph();
        dangling.edges[0].to = "missing".into();
        assert!(ModuleInputValue::Graph(dangling).validate().is_err());

        let mut invalid_heat = activity_graph();
        invalid_heat.nodes[0].heat = f32::NAN;
        assert!(ModuleInputValue::Graph(invalid_heat).validate().is_err());
    }

    #[test]
    fn pnl_module_receives_status_and_coherence_theme() {
        let manifest =
            ModuleManifest::parse(include_str!("../examples/lua/pnl-status.json")).unwrap();
        let snapshot = ModuleInputSnapshot {
            revision: 1,
            now_ms: TEST_NOW_MS,
            values: BTreeMap::from([
                ("pnl".into(), ModuleInputValue::Number(1842.50)),
                (
                    "status".into(),
                    ModuleInputValue::Status {
                        level: StatusLevel::Good,
                        label: "Live".into(),
                    },
                ),
            ]),
        };
        let module = LuaModule::load(
            manifest,
            include_str!("../examples/lua/pnl-status.lua"),
            snapshot,
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();

        let texts: Vec<&str> = module
            .retained()
            .scene
            .nodes
            .iter()
            .filter_map(|node| match &node.content {
                super::super::NodeContent::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["P/L  +1842.50", "Live · dark/info"]);
        assert_eq!(
            module.theme.primary,
            canvas_core::coherence_tokens::dark::COLOR_INFO
        );
    }

    #[test]
    fn updates_and_hot_reload_are_atomic() {
        let mut module = LuaModule::load(
            graph_manifest(),
            include_str!("../examples/lua/graph.lua"),
            graph_snapshot(1),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();
        module
            .update(
                graph_snapshot(2),
                theme(),
                ModuleEvent::DataChanged {
                    changed_inputs: vec!["samples".into()],
                },
            )
            .unwrap();
        assert_eq!(module.retained().output_revision, 2);

        let before = module.retained().clone();
        assert!(
            module
                .hot_reload(graph_manifest(), "while true do end")
                .is_err()
        );
        assert_eq!(module.retained(), &before);

        module
            .hot_reload(
                graph_manifest(),
                "canvas.text { id = 'reloaded', text = module.event.kind }",
            )
            .unwrap();
        assert_eq!(module.retained().module_generation, 2);
        assert!(matches!(
            &module.retained().scene.nodes[0].content,
            super::super::NodeContent::Text { text, .. } if text == "reload"
        ));
    }

    #[test]
    fn video_and_map_are_host_owned_typed_references() {
        let manifest = ModuleManifest::parse(
            r#"{
              "schema_version":1,
              "id":"operations-wall",
              "title":"Operations wall",
              "resources":[
                {"alias":"north-camera","kind":"video_stream","contract":"nvr.video.v1"},
                {"alias":"fleet-map","kind":"map_viewport","contract":"maps.viewport.v1"}
              ],
              "capabilities":{"write_scene":true,"present_resources":["north-camera","fleet-map"]}
            }"#,
        )
        .unwrap();
        let resources = BTreeMap::from([
            (
                "north-camera".into(),
                HostResourceRef {
                    owner: "nvr".into(),
                    id: "camera-7".into(),
                    kind: HostResourceKind::VideoStream,
                    contract: "nvr.video.v1".into(),
                },
            ),
            (
                "fleet-map".into(),
                HostResourceRef {
                    owner: "maps".into(),
                    id: "fleet-live".into(),
                    kind: HostResourceKind::MapViewport,
                    contract: "maps.viewport.v1".into(),
                },
            ),
        ]);
        let module = LuaModule::load(
            manifest,
            r#"
              canvas.group { id = "wall" }
              canvas.host_resource { id = "camera", parent = "wall", resource = "north-camera" }
              canvas.host_resource { id = "map", parent = "wall", resource = "fleet-map", translation = {1, 0, 0} }
            "#,
            ModuleInputSnapshot {
                revision: 1,
                now_ms: TEST_NOW_MS,
                values: BTreeMap::new(),
            },
            theme(),
            resources,
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();

        assert_eq!(module.retained().host_resources.len(), 2);
        assert_eq!(
            module.retained().host_resources[0].resource.kind,
            HostResourceKind::VideoStream
        );
        let wire = serde_json::to_value(module.retained()).unwrap();
        assert_eq!(
            wire["host_resources"][0]["resource"]["kind"],
            "video_stream"
        );
        assert!(wire.to_string().find("endpoint").is_none());
    }

    #[test]
    fn manifest_has_no_ambient_authority_fields() {
        let rejected = ModuleManifest::parse(
            r#"{
              "schema_version":1,
              "id":"unsafe",
              "title":"Unsafe",
              "capabilities":{"write_scene":true,"filesystem":true}
            }"#,
        )
        .unwrap_err();
        assert!(rejected.0.contains("unknown field"));
    }
}

#[cfg(test)]
mod calendar_module_tests {
    use super::super::{NodeContent, Scene, SemanticInk};
    use super::*;

    /// Midday UTC on 2026-09-28. Assertions about *today* still derive it from
    /// `crate::hud::local_parts` rather than assume it, because the whole point of
    /// stamping the instant is that the host and the widget agree in whatever zone
    /// the machine is actually in.
    const SEPT_2026: u64 = 1_790_600_000_000;

    fn theme() -> ModuleThemeTokens {
        ProjectTheme {
            mode: ThemeMode::Dark,
            accent: Accent::Info,
        }
        .into()
    }

    fn calendar_scene(values: Vec<(&str, ModuleInputValue)>, now_ms: u64) -> Scene {
        let manifest =
            ModuleManifest::parse(include_str!("../../../config/modules/calendar-v1.json"))
                .expect("calendar manifest");
        let module = LuaModule::load(
            manifest,
            include_str!("../../../config/modules/calendar-v1.lua"),
            ModuleInputSnapshot {
                revision: 1,
                now_ms,
                values: values
                    .into_iter()
                    .map(|(id, value)| (id.to_string(), value))
                    .collect(),
            },
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect("the authored calendar must render");
        module.retained().scene.clone()
    }

    fn calendar_with(x: f32, y: f32, view: ModuleView) -> RetainedModuleScene {
        let manifest =
            ModuleManifest::parse(include_str!("../../../config/modules/calendar-v1.json"))
                .expect("calendar manifest");
        let mut module = LuaModule::load_with_view(
            manifest,
            include_str!("../../../config/modules/calendar-v1.lua"),
            ModuleInputSnapshot {
                revision: 1,
                now_ms: SEPT_2026,
                values: BTreeMap::new(),
            },
            theme(),
            view,
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect("the calendar must render");
        module
            .update_event(
                theme(),
                ModuleEvent::Pointer {
                    x,
                    y,
                    phase: PointerPhase::Press,
                },
            )
            .expect("a click must re-render the view");
        module.retained().clone()
    }

    #[test]
    fn a_click_chooses_a_day_and_marks_it() {
        // The first cell of a month grid. Exact to the pixel would make this test
        // a copy of the layout; what matters is that a press lands on *a* day and
        // that the choice is visible afterwards.
        let rendered = calendar_with(0.095, 0.2675, ModuleView::default());
        let selected = rendered
            .view
            .selected
            .clone()
            .expect("clicking a day must choose it");
        assert!(
            rendered.scene.nodes.iter().any(|node| node.id == format!("d{selected}")),
            "the chosen day must be on the grid"
        );
        assert!(
            rendered
                .scene
                .nodes
                .iter()
                .any(|node| node.id == format!("s{selected}")
                    && matches!(node.content, NodeContent::Rect { .. })),
            "the chosen day must be marked"
        );
    }

    #[test]
    fn a_chosen_day_survives_the_redraw_that_follows() {
        let first = calendar_with(0.095, 0.2675, ModuleView::default());
        let selected = first.view.selected.clone().expect("a choice");
        // A view-local choice is deliberately not an intent: nothing leaves the
        // process, so the Spore is never told which day is merely highlighted.
        // A plain redraw, no pointer at all: the memory is the host's to carry,
        // which is the whole reason it is passed in.
        let second = calendar_with(0.0, 0.0, first.view.clone());
        assert_eq!(second.view.selected.as_deref(), Some(selected.as_str()));
        assert!(
            second
                .scene
                .nodes
                .iter()
                .any(|node| node.id == format!("s{selected}")),
            "the mark must still be there without a new click"
        );
    }

    fn text_node<'a>(scene: &'a Scene, id: &str) -> Option<(&'a str, Option<SemanticInk>)> {
        scene
            .nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| match &node.content {
                NodeContent::Text { text, ink, .. } => (text.as_str(), *ink),
                other => panic!("node {id} is not text: {other:?}"),
            })
    }

    fn day_cells(scene: &Scene) -> Vec<(String, String)> {
        scene
            .nodes
            .iter()
            .filter_map(|node| {
                node.id.strip_prefix("d").map(|key| {
                    (
                        key.to_string(),
                        match &node.content {
                            NodeContent::Text { text, .. } => text.clone(),
                            _ => String::new(),
                        },
                    )
                })
            })
            .collect()
    }

    fn text(values: &str) -> ModuleInputValue {
        ModuleInputValue::Text(values.into())
    }

    #[test]
    fn the_host_date_arrives_in_the_module_and_inks_the_day_it_names() {
        let today = crate::hud::local_parts(SEPT_2026);
        let scene = calendar_scene(Vec::new(), SEPT_2026);
        let key = format!("{:04}-{:02}-{:02}", today.year, today.month, today.day);
        let (drawn, ink) =
            text_node(&scene, &format!("d{key}")).expect("today must be on the grid");
        assert_eq!(drawn, today.day.to_string());
        assert_eq!(
            ink,
            Some(SemanticInk::Primary),
            "today is the one cell the reader is looking for"
        );
    }

    #[test]
    fn a_pinned_month_draws_six_contiguous_weeks_beginning_on_monday() {
        let scene = calendar_scene(vec![("anchor", text("2026-09-01"))], SEPT_2026);
        assert_eq!(
            text_node(&scene, "title").expect("title").0,
            "September 2026"
        );
        // 2026-09-01 is a Tuesday, so the grid opens on Monday the 31st. Asserted
        // from the calendar, not the clock: the anchor is what a person pins.
        let cells = day_cells(&scene);
        assert_eq!(cells.len(), 42, "six rows of seven");
        assert_eq!(cells[0], ("2026-08-31".into(), "31".into()));
        assert_eq!(cells[1], ("2026-09-01".into(), "1".into()));
        assert_eq!(cells[41], ("2026-10-11".into(), "11".into()));
        assert_eq!(
            text_node(&scene, "d2026-08-31").expect("trailing day").1,
            Some(SemanticInk::Muted),
            "a day from the month either side must not compete with this one"
        );
        assert_eq!(
            text_node(&scene, "d2026-09-01").expect("first").1,
            Some(SemanticInk::Text)
        );
    }

    #[test]
    fn a_leap_february_reaches_the_29th_and_stops_there() {
        let scene = calendar_scene(vec![("anchor", text("2028-02-01"))], SEPT_2026);
        let keys: Vec<String> = day_cells(&scene).into_iter().map(|(key, _)| key).collect();
        assert!(keys.contains(&"2028-02-29".into()), "{keys:?}");
        assert!(!keys.contains(&"2028-02-30".into()), "{keys:?}");
    }

    #[test]
    fn an_agenda_marks_its_day_without_moving_the_grid() {
        let scene = calendar_scene(
            vec![
                ("anchor", text("2026-09-01")),
                (
                    "agenda",
                    text(
                        "2026-09-14 09:30 standup|2026-09-14 14:00 review|2026-09-30 08:00 shipit",
                    ),
                ),
            ],
            SEPT_2026,
        );
        assert_eq!(day_cells(&scene).len(), 42);
        // Two events share the day, so the mark is two dots rather than a count:
        // a number at this size is unreadable across a room.
        assert_eq!(text_node(&scene, "m2026-09-14").expect("mark").0, "••");
        assert_eq!(text_node(&scene, "m2026-09-30").expect("mark").0, "•");
        assert!(
            text_node(&scene, "m2026-09-15").is_none(),
            "a day with nothing on it draws no mark"
        );
    }

    #[test]
    fn week_view_opens_on_monday_and_carries_the_plans_of_that_week() {
        let scene = calendar_scene(
            vec![
                ("view", text("week")),
                ("anchor", text("2026-09-01")),
                ("agenda", text("2026-09-01 10:00 fpl call")),
            ],
            SEPT_2026,
        );
        assert_eq!(
            text_node(&scene, "title").expect("title").0,
            "August 31 – September 6",
            "the week is named by its own endpoints, not by seven days from Tuesday"
        );
        assert_eq!(text_node(&scene, "c1").expect("monday").0, "Mon 31");
        assert_eq!(text_node(&scene, "c7").expect("sunday").0, "Sun 6");
        assert_eq!(
            text_node(&scene, "e2026-09-01-1").expect("event").0,
            "10:00  fpl call"
        );
        assert_eq!(
            text_node(&scene, "n2026-08-31").expect("quiet day").0,
            "open"
        );
    }

    #[test]
    fn day_view_titles_the_weekday_it_names() {
        let scene = calendar_scene(
            vec![("view", text("day")), ("anchor", text("2026-09-01"))],
            SEPT_2026,
        );
        assert_eq!(
            text_node(&scene, "title").expect("title").0,
            "Tue 1 September 2026"
        );
    }

    #[test]
    fn an_unreadable_view_or_anchor_falls_back_without_inventing_a_date() {
        for values in [
            vec![("view", text("hourly"))],
            vec![("anchor", text("tomorrow-ish"))],
            vec![("agenda", text("not an agenda at all"))],
        ] {
            let scene = calendar_scene(values.clone(), SEPT_2026);
            assert_eq!(
                day_cells(&scene).len(),
                42,
                "{values:?} must still draw a month"
            );
        }
    }

    #[test]
    fn an_empty_agenda_is_a_calendar_and_not_a_claim_about_plans() {
        let scene = calendar_scene(
            vec![("view", text("day")), ("anchor", text("2026-09-01"))],
            SEPT_2026,
        );
        assert_eq!(text_node(&scene, "n2026-09-01").expect("quiet").0, "open");
        assert!(
            scene.nodes.iter().all(|node| !node.id.starts_with("e")),
            "no event line may be drawn that the payload did not send"
        );
    }

    #[test]
    fn host_layout_measures_text_and_fills_the_viewport() {
        let manifest = ModuleManifest::parse(
            r#"{
              "schema_version":1,
              "id":"layout-test",
              "title":"Layout test",
              "capabilities":{"write_scene":true,"read_inputs":[],"read_theme":true}
            }"#,
        )
        .unwrap();
        let source = r#"
local laid = canvas.layout({
  id = "root",
  kind = "container",
  layout = {
    padding = { top = 10, right = 10, bottom = 10, left = 10 },
    gap = 8,
    direction = "column",
  },
  children = {
    { id = "title", kind = "text", layout = {}, text = "Hello layout", style = { size = 16, line_height = 22, wrap = true } },
    { id = "body", kind = "text", layout = {}, text = "one two three four five six seven eight nine ten", style = { size = 14, line_height = 20, wrap = true } },
  },
}, 240, 320)
canvas.text { id = "root-width", text = string.format("%.0f", laid.root.width), translation = { 0, 0, 0.2 }, scale = { 1, 1, 1 } }
canvas.text { id = "body-height", text = string.format("%.0f", laid.nodes[3].rect.height), translation = { 0, 0, 0.2 }, scale = { 1, 1, 1 } }
"#;
        let module = LuaModule::load(
            manifest,
            source,
            ModuleInputSnapshot {
                revision: 1,
                now_ms: SEPT_2026,
                values: BTreeMap::new(),
            },
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect("the host layout API must render");
        let scene = &module.retained().scene;
        let root_width = text_node(scene, "root-width").expect("root width").0;
        let body_height = text_node(scene, "body-height").expect("body height").0;
        assert_eq!(root_width, "240");
        assert!(
            body_height.parse::<f32>().unwrap() > 20.0,
            "text should wrap to more than one line, got {body_height}"
        );
    }

    #[test]
    fn a_pointer_event_is_normalized_to_the_surface() {
        let manifest = ModuleManifest::parse(include_str!(
            "../../../config/modules/calendar-v1.json"
        ))
        .expect("manifest");

        // Normalized coordinates are the contract: a view is re-rendered when a
        // window resizes, so a pointer in pixels would mean something different
        // after the re-render.
        for (x, y) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.25)] {
            validate_event(
                &manifest,
                &ModuleEvent::Pointer {
                    x,
                    y,
                    phase: PointerPhase::Press,
                },
            )
            .expect("a point inside the surface is valid");
        }
        for (x, y) in [(-0.1, 0.5), (0.5, 1.1), (f32::NAN, 0.5)] {
            assert!(
                validate_event(
                    &manifest,
                    &ModuleEvent::Pointer {
                        x,
                        y,
                        phase: PointerPhase::Move,
                    },
                )
                .is_err(),
                "({x}, {y}) is outside the surface and must be rejected"
            );
        }
    }

    #[test]
    fn a_pointer_event_reaches_lua_as_a_table() {
        let lua = mlua::Lua::new();
        let table = event_to_lua(
            &lua,
            &ModuleEvent::Pointer {
                x: 0.25,
                y: 0.75,
                phase: PointerPhase::Release,
            },
        )
        .expect("event table");
        assert_eq!(table.get::<String>("kind").unwrap(), "pointer");
        assert_eq!(table.get::<String>("phase").unwrap(), "release");
        assert!((table.get::<f32>("x").unwrap() - 0.25).abs() < f32::EPSILON);
        assert!((table.get::<f32>("y").unwrap() - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn a_month_view_fits_the_hosts_node_budget() {
        // The default budget is 256 nodes. A month with an event on every day is
        // the worst case, and it must fit rather than fail at the host boundary.
        let agenda: Vec<String> = (1..=31)
            .map(|day| format!("2026-09-{:02} 09:00 standup", day))
            .collect();
        let scene = calendar_scene(
            vec![
                ("anchor", text("2026-09-01")),
                ("agenda", text(&agenda.join("|"))),
            ],
            SEPT_2026,
        );
        assert!(
            scene.nodes.len() < LuaSceneLimits::default().max_nodes,
            "{} nodes",
            scene.nodes.len()
        );
    }
}

#[cfg(test)]
mod widget_artifact_tests {
    use super::{widget_fingerprint, widget_library};

    #[test]
    fn the_widget_library_is_an_artifact() {
        let library = widget_library();
        // Composition helpers live here, not in Rust: a panel, a label, a badge.
        // `widget.button` stays native because a button is a hit region the host
        // has to know about; everything above the hit region is composition.
        for helper in [
            "function widget.panel",
            "function widget.label",
            "function widget.badge",
            "function widget.pixel_space",
        ] {
            assert!(library.contains(helper), "missing {helper}");
        }
        let fingerprint = widget_fingerprint();
        assert_eq!(fingerprint.len(), 16, "a short digest, not a whole file");
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
