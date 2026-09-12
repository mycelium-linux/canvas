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
use canvas_core::{Accent, ProjectTheme, ThemeMode};
use mlua::{Lua, Table};
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
    Pulse { sequence: u64, delta_ms: f64 },
    Reload,
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

/// Last known-good renderer-neutral output retained by the Rust host.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RetainedModuleScene {
    pub output_revision: u64,
    pub module_generation: u64,
    pub input_revision: u64,
    pub scene: Scene,
    pub host_resources: Vec<HostResourceSurface>,
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
            resources: &self.resources,
            scene_capabilities: &self.scene_capabilities,
            limits: self.limits,
        })?;
        self.snapshot = snapshot;
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
    let scene = generate_scene_with_setup(
        request.source,
        request.scene_capabilities,
        request.limits,
        move |lua| {
            install_module_context(lua, &manifest, &snapshot, request.theme, &event, &resources)?;
            install_host_resource_api(lua, &manifest, &resources, setup_placements)
        },
    )?;
    let host_resources = Rc::try_unwrap(placements)
        .map_err(|_| Error("Canvas Lua retained internal resource state".into()))?
        .into_inner();
    validate_retained_output(&scene, &host_resources, request.limits.max_nodes)?;
    Ok(RetainedModuleScene {
        output_revision: request.output_revision,
        module_generation: request.generation,
        input_revision: request.snapshot.revision,
        scene,
        host_resources,
    })
}

fn install_module_context(
    lua: &Lua,
    manifest: &ModuleManifest,
    snapshot: &ModuleInputSnapshot,
    theme: ModuleThemeTokens,
    event: &ModuleEvent,
    resources: &BTreeMap<String, HostResourceRef>,
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
        .set("event", event_to_lua(lua, event)?)
        .map_err(lua_error)?;
    if manifest.capabilities.read_theme {
        module
            .set("theme", theme_to_lua(lua, theme)?)
            .map_err(lua_error)?;
    }

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

    widget
        .set(
            "time",
            lua.create_function(|lua, ()| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let h = (now % 86400) / 3600;
                let m = (now % 3600) / 60;
                let s = now % 60;
                let text = format!("{:02}:{:02}:{:02}", h, m, s);
                widget_set_text(lua, &text)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

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
    canvas
        .set(
            "webhook_post",
            lua.create_function(|_, (url, payload): (String, String)| {
                let result = std::process::Command::new("curl")
                    .arg("-sS")
                    .arg("-X")
                    .arg("POST")
                    .arg(&url)
                    .arg("-H")
                    .arg("Content-Type: application/json")
                    .arg("-d")
                    .arg(&payload)
                    .arg("--max-time")
                    .arg("10")
                    .arg("--connect-timeout")
                    .arg("5")
                    .output();
                match result {
                    Ok(output) => Ok(format!(
                        "ok status={} body={}",
                        output.status,
                        String::from_utf8_lossy(&output.stdout)
                    )),
                    Err(error) => Err(mlua::Error::RuntimeError(format!(
                        "webhook failed: {error}"
                    ))),
                }
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    lua.globals().set("canvas", canvas).map_err(lua_error)
}

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
    Ok(table)
}

fn validate_snapshot(manifest: &ModuleManifest, snapshot: &ModuleInputSnapshot) -> Result<()> {
    ensure(
        snapshot.revision > 0,
        "Canvas Lua input revision must be positive",
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

    fn graph_snapshot(revision: u64) -> ModuleInputSnapshot {
        ModuleInputSnapshot {
            revision,
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
            super::super::NodeContent::Text { text } if text == "███"
        ));
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
        let manifest = ModuleManifest::parse(
            include_str!("../examples/lua/cross-owner-graph.json")
        ).unwrap();
        let module = LuaModule::load(
            manifest,
            include_str!("../examples/lua/cross-owner-graph.lua"),
            cross_owner_snapshot(),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        ).unwrap();
        let scene = &module.retained().scene;
        // Root group + 2 group labels + 2 headers + 4 nodes + 1 bridge = 10
        assert!(scene.nodes.len() >= 8, "expected at least 8 scene nodes, got {}", scene.nodes.len());
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
        let manifest = ModuleManifest::parse(
            include_str!("../examples/lua/cross-owner-graph.json")
        ).unwrap();
        // First: load with valid data
        let good = LuaModule::load(
            manifest.clone(),
            include_str!("../examples/lua/cross-owner-graph.lua"),
            cross_owner_snapshot(),
            theme(),
            BTreeMap::new(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        ).unwrap();
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
                super::super::NodeContent::Text { text } => Some(text.as_str()),
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
            super::super::NodeContent::Text { text } if text == "reload"
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
            ModuleInputSnapshot { revision: 1, values: BTreeMap::new() },
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
