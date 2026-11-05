//! Capability-scoped Luau scene generation.
//!
//! Scripts receive a single `canvas` table. They can append typed scene nodes,
//! but cannot read files, load packages, open sockets, launch processes, or
//! mutate workspace state.

use super::{
    AssetRef, Error, NodeContent, Result, Scene, SceneNode, SemanticInk, Transform3, identifier,
};
use mlua::{Lua, LuaOptions, StdLib, Table, Value, VmState};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const DEFAULT_SOURCE_BYTES: usize = 64 * 1024;
const DEFAULT_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_INTERRUPT_BUDGET: u64 = 100_000;
const DEFAULT_MAX_NODES: usize = 256;
const HARD_MAX_SOURCE_BYTES: usize = 1024 * 1024;
const HARD_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_INTERRUPTS: u64 = 10_000_000;

/// Host-controlled limits for one isolated scene generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaSceneLimits {
    pub max_source_bytes: usize,
    pub max_memory_bytes: usize,
    /// Maximum Luau VM interrupt callbacks, used to stop runaway scripts.
    pub max_interrupts: u64,
    pub max_nodes: usize,
}

impl Default for LuaSceneLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_SOURCE_BYTES,
            max_memory_bytes: DEFAULT_MEMORY_BYTES,
            max_interrupts: DEFAULT_INTERRUPT_BUDGET,
            max_nodes: DEFAULT_MAX_NODES,
        }
    }
}

/// Immutable assets explicitly granted to a script by a trusted caller.
///
/// Scripts refer to aliases rather than constructing paths or asset hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LuaSceneCapabilities {
    pub assets: BTreeMap<String, AssetRef>,
}

/// Generate a bounded typed scene from an isolated Luau source string.
///
/// The source has no filesystem, network, process, package, dynamic-library,
/// or durable-storage API. Each invocation creates a fresh VM and the only
/// host callback surface is `canvas.group`, `canvas.text`, `canvas.image`, and
/// `canvas.mesh`.
pub fn generate_scene(
    source: &str,
    capabilities: &LuaSceneCapabilities,
    limits: LuaSceneLimits,
) -> Result<Scene> {
    generate_scene_with_setup(source, capabilities, limits, |_| Ok(()))
}

pub(crate) fn generate_scene_with_setup<F>(
    source: &str,
    capabilities: &LuaSceneCapabilities,
    limits: LuaSceneLimits,
    setup: F,
) -> Result<Scene>
where
    F: FnOnce(&Lua) -> Result<()>,
{
    validate_inputs(source, capabilities, limits)?;

    let libraries = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
    let lua = Lua::new_with(libraries, LuaOptions::default()).map_err(lua_error)?;
    lua.set_memory_limit(limits.max_memory_bytes)
        .map_err(lua_error)?;
    remove_dynamic_loading_and_host_introspection(&lua)?;

    let interrupts = Arc::new(AtomicU64::new(0));
    let interrupt_count = Arc::clone(&interrupts);
    lua.set_interrupt(move |_| {
        let count = interrupt_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count > limits.max_interrupts {
            Err(mlua::Error::RuntimeError(
                "Canvas Lua interrupt budget exceeded".into(),
            ))
        } else {
            Ok(VmState::Continue)
        }
    });

    let nodes = Rc::new(RefCell::new(Vec::new()));
    install_canvas_api(&lua, capabilities, limits.max_nodes, Rc::clone(&nodes))?;
    setup(&lua)?;
    lua.sandbox(true).map_err(lua_error)?;
    lua.load(source)
        .set_name("canvas-scene")
        .exec()
        .map_err(lua_error)?;
    drop(lua);

    let scene = Scene {
        nodes: Rc::try_unwrap(nodes)
            .map_err(|_| Error("Canvas Lua retained internal scene state".into()))?
            .into_inner(),
    };
    scene.validate()?;
    Ok(scene)
}

fn remove_dynamic_loading_and_host_introspection(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    for name in [
        "collectgarbage",
        "debug",
        "dofile",
        "getfenv",
        "io",
        "load",
        "loadfile",
        "loadstring",
        "os",
        "package",
        "require",
        "setfenv",
    ] {
        globals.set(name, Value::Nil).map_err(lua_error)?;
    }
    let math: Table = globals.get("math").map_err(lua_error)?;
    math.set("random", Value::Nil).map_err(lua_error)?;
    math.set("randomseed", Value::Nil).map_err(lua_error)?;
    Ok(())
}

fn validate_inputs(
    source: &str,
    capabilities: &LuaSceneCapabilities,
    limits: LuaSceneLimits,
) -> Result<()> {
    if source.len() > limits.max_source_bytes {
        return Err(Error("Canvas Lua source exceeds its byte limit".into()));
    }
    if !(1..=HARD_MAX_SOURCE_BYTES).contains(&limits.max_source_bytes)
        || !(1..=HARD_MAX_MEMORY_BYTES).contains(&limits.max_memory_bytes)
        || !(1..=HARD_MAX_INTERRUPTS).contains(&limits.max_interrupts)
        || limits.max_nodes == 0
        || limits.max_nodes > 1_024
    {
        return Err(Error(
            "Canvas Lua limits must be positive and bounded".into(),
        ));
    }
    for (alias, asset) in &capabilities.assets {
        if !identifier(alias) {
            return Err(Error(format!("Invalid Canvas Lua asset alias: {alias}")));
        }
        asset.validate()?;
    }
    Ok(())
}

fn install_canvas_api(
    lua: &Lua,
    capabilities: &LuaSceneCapabilities,
    max_nodes: usize,
    nodes: Rc<RefCell<Vec<SceneNode>>>,
) -> Result<()> {
    let canvas = lua.create_table().map_err(lua_error)?;

    let group_nodes = Rc::clone(&nodes);
    canvas
        .set(
            "group",
            lua.create_function(move |_, spec: Table| {
                let node = parse_node(
                    &spec,
                    &["id", "parent", "translation", "rotation", "scale"],
                    NodeContent::Group,
                )?;
                append_node(&group_nodes, max_nodes, node)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

    let text_nodes = Rc::clone(&nodes);
    canvas
        .set(
            "text",
            lua.create_function(move |_, spec: Table| {
                let text = required_string(&spec, "text")?;
                let ink = optional_ink(&spec)?;
                let rgb = optional_rgb(&spec)?;
                let opacity = spec.get::<Option<f32>>("opacity")?.unwrap_or(1.0);
                let node = parse_node(
                    &spec,
                    &[
                        "id",
                        "parent",
                        "text",
                        "ink",
                        "rgb",
                        "opacity",
                        "translation",
                        "rotation",
                        "scale",
                        "transition_ms",
                    ],
                    NodeContent::Text {
                        text,
                        ink,
                        rgb,
                        opacity,
                    },
                )?;
                append_node(&text_nodes, max_nodes, node)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

    let rect_nodes = Rc::clone(&nodes);
    canvas
        .set(
            "rect",
            lua.create_function(move |_, spec: Table| {
                let ink = optional_ink(&spec)?;
                let rgb = optional_rgb(&spec)?;
                let opacity = spec.get::<Option<f32>>("opacity")?.unwrap_or(1.0);
                let radius = spec.get::<Option<f32>>("radius")?.unwrap_or(0.0);
                let node = parse_node(
                    &spec,
                    &[
                        "id",
                        "parent",
                        "ink",
                        "opacity",
                        "radius",
                        "rgb",
                        "translation",
                        "rotation",
                        "scale",
                        "transition_ms",
                    ],
                    NodeContent::Rect {
                        ink,
                        rgb,
                        opacity,
                        radius,
                    },
                )?;
                append_node(&rect_nodes, max_nodes, node)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;

    install_asset_node(
        lua,
        &canvas,
        "image",
        capabilities.assets.clone(),
        max_nodes,
        Rc::clone(&nodes),
        |asset| NodeContent::Image { asset },
    )?;
    install_asset_node(
        lua,
        &canvas,
        "mesh",
        capabilities.assets.clone(),
        max_nodes,
        nodes,
        |asset| NodeContent::Mesh { asset },
    )?;

    lua.globals().set("canvas", canvas).map_err(lua_error)
}

fn install_asset_node(
    lua: &Lua,
    canvas: &Table,
    function_name: &'static str,
    assets: BTreeMap<String, AssetRef>,
    max_nodes: usize,
    nodes: Rc<RefCell<Vec<SceneNode>>>,
    content: fn(AssetRef) -> NodeContent,
) -> Result<()> {
    canvas
        .set(
            function_name,
            lua.create_function(move |_, spec: Table| {
                let alias = required_string(&spec, "asset")?;
                let asset = assets.get(&alias).cloned().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "Canvas asset capability not granted: {alias}"
                    ))
                })?;
                let node = parse_node(
                    &spec,
                    &["id", "parent", "asset", "translation", "rotation", "scale"],
                    content(asset),
                )?;
                append_node(&nodes, max_nodes, node)
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)
}

fn append_node(
    nodes: &RefCell<Vec<SceneNode>>,
    max_nodes: usize,
    node: SceneNode,
) -> mlua::Result<()> {
    let mut nodes = nodes.borrow_mut();
    if nodes.len() >= max_nodes {
        return Err(mlua::Error::RuntimeError(
            "Canvas Lua scene exceeds its node limit".into(),
        ));
    }
    nodes.push(node);
    Ok(())
}

fn parse_node(spec: &Table, allowed: &[&str], content: NodeContent) -> mlua::Result<SceneNode> {
    reject_unknown_fields(spec, allowed)?;
    Ok(SceneNode {
        id: required_string(spec, "id")?,
        parent_id: spec.get("parent")?,
        transform: parse_transform(spec)?,
        content,
        // Lua may claim a duration for its own node; the host decides what it
        // means and when it starts.
        transition_ms: spec.get::<Option<u64>>("transition_ms")?,
    })
}

pub(crate) fn parse_transform(spec: &Table) -> mlua::Result<Transform3> {
    Ok(Transform3 {
        translation: optional_vector(spec, "translation", [0.0; 3])?,
        rotation: optional_vector(spec, "rotation", [0.0, 0.0, 0.0, 1.0])?,
        scale: optional_vector(spec, "scale", [1.0; 3])?,
    })
}

pub(crate) fn required_string(table: &Table, key: &str) -> mlua::Result<String> {
    table.get::<Option<String>>(key)?.ok_or_else(|| {
        mlua::Error::RuntimeError(format!("Canvas Lua field '{key}' must be a string"))
    })
}

/// The semantic ink role of a label, or `None` for the default body ink.
///
/// An unknown role is an error rather than a silent fall back to `text`: a scene
/// that meant to shout "today" and quietly drew body-grey is the kind of wrong
/// that is only noticed by someone who already distrusts the wall.
/// A literal colour as `{r, g, b}` in 0..=1, or `#rrggbb`.
///
/// Kept separate from `ink` on purpose: a semantic ink means "the theme decides",
/// which is what composition should normally say. A literal colour means "this
/// thing has its own identity" -- an app icon, a brand mark -- and it should be
/// visible that the author reached for that.
fn optional_rgb(table: &Table) -> mlua::Result<Option<[u8; 3]>> {
    let value: mlua::Value = table.get("rgb")?;
    match value {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(text) => {
            let text = text.to_str()?.to_string();
            let hex = text.strip_prefix('#').unwrap_or(&text);
            if hex.len() != 6 {
                return Err(mlua::Error::RuntimeError(
                    "Canvas Lua rgb must be #rrggbb".into(),
                ));
            }
            let channel = |range: std::ops::Range<usize>| {
                u8::from_str_radix(&hex[range], 16).map_err(|_| {
                    mlua::Error::RuntimeError("Canvas Lua rgb must be #rrggbb".into())
                })
            };
            Ok(Some([channel(0..2)?, channel(2..4)?, channel(4..6)?]))
        }
        mlua::Value::Table(channels) => {
            let mut rgb = [0u8; 3];
            for (index, slot) in rgb.iter_mut().enumerate() {
                let value = channels.get::<f32>(index + 1)?;
                if !value.is_finite() {
                    return Err(mlua::Error::RuntimeError(
                        "Canvas Lua rgb channels must be finite".into(),
                    ));
                }
                *slot = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            Ok(Some(rgb))
        }
        _ => Err(mlua::Error::RuntimeError(
            "Canvas Lua rgb must be #rrggbb or { r, g, b }".into(),
        )),
    }
}

fn optional_ink(table: &Table) -> mlua::Result<Option<SemanticInk>> {
    let Some(name) = table.get::<Option<String>>("ink")? else {
        return Ok(None);
    };
    let ink = match name.as_str() {
        "text" => SemanticInk::Text,
        "surface" => SemanticInk::Surface,
        "background" => SemanticInk::Background,
        "muted" => SemanticInk::Muted,
        "primary" => SemanticInk::Primary,
        "success" => SemanticInk::Success,
        "warning" => SemanticInk::Warning,
        "danger" => SemanticInk::Danger,
        "info" => SemanticInk::Info,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Canvas Lua ink must be text, muted, primary, success, warning, danger or info; got {other}"
            )));
        }
    };
    Ok(Some(ink))
}

fn optional_vector<const N: usize>(
    table: &Table,
    key: &str,
    default: [f32; N],
) -> mlua::Result<[f32; N]> {
    let Some(values) = table.get::<Option<Table>>(key)? else {
        return Ok(default);
    };
    if values.raw_len() != N {
        return Err(mlua::Error::RuntimeError(format!(
            "Canvas Lua field '{key}' must contain exactly {N} numbers"
        )));
    }
    let mut result = [0.0; N];
    for (index, target) in result.iter_mut().enumerate() {
        let value = values.get::<f64>(index + 1)?;
        if !value.is_finite() || value < f32::MIN as f64 || value > f32::MAX as f64 {
            return Err(mlua::Error::RuntimeError(format!(
                "Canvas Lua field '{key}' contains a non-finite or out-of-range number"
            )));
        }
        *target = value as f32;
    }
    Ok(result)
}

pub(crate) fn reject_unknown_fields(table: &Table, allowed: &[&str]) -> mlua::Result<()> {
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        let Value::String(key) = key else {
            return Err(mlua::Error::RuntimeError(
                "Canvas Lua node fields must have string keys".into(),
            ));
        };
        let key = key.to_str()?;
        if !allowed.contains(&key.as_ref()) {
            return Err(mlua::Error::RuntimeError(format!(
                "Unknown Canvas Lua node field: {key}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn lua_error(error: mlua::Error) -> Error {
    Error(format!("Canvas Lua rejected: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> AssetRef {
        AssetRef {
            sha256: "a".repeat(64),
            media_type: "image/png".into(),
            byte_length: 42,
        }
    }

    /// The ink a scene asks for is the ink it gets, and the absence of one is the
    /// body default rather than a grey invented by whoever wrote the renderer.
    fn ink_of(source: &str) -> Option<SemanticInk> {
        let scene = generate_scene(
            source,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect("scene");
        match &scene.nodes[0].content {
            NodeContent::Text { ink, .. } => *ink,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn ink_is_accepted_per_role_and_absent_means_body_text() {
        assert_eq!(
            ink_of(r#"canvas.text { id = "a", text = "hi", ink = "primary" }"#),
            Some(SemanticInk::Primary)
        );
        assert_eq!(
            ink_of(r#"canvas.text { id = "a", text = "hi", ink = "muted" }"#),
            Some(SemanticInk::Muted)
        );
        assert_eq!(ink_of(r#"canvas.text { id = "a", text = "hi" }"#), None);
    }

    #[test]
    fn rectangle_is_a_bounded_theme_resolved_primitive() {
        let scene = generate_scene(
            r#"canvas.rect { id = "panel", ink = "primary", opacity = 0.25, radius = 12, scale = { 4, 2, 1 } }"#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect("rectangle scene");
        assert!(matches!(
            scene.nodes[0].content,
            NodeContent::Rect {
                ink: Some(SemanticInk::Primary),
                opacity,
                radius,
                ..
            } if opacity == 0.25 && radius == 12.0
        ));

        let error = generate_scene(
            r#"canvas.rect { id = "panel", opacity = 2 }"#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect_err("opacity outside the contract");
        assert!(error.0.contains("opacity"), "{error}");
    }

    #[test]
    fn an_unknown_ink_role_fails_loudly_rather_than_drawing_grey() {
        // A scene that meant to mark today and silently drew body text would be
        // unreadable in exactly the way a wall widget must not be.
        let error = generate_scene(
            r#"canvas.text { id = "a", text = "hi", ink = "chartreuse" }"#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .expect_err("unknown role");
        let message = error.0;
        assert!(message.contains("ink"), "{message}");
        assert!(message.contains("chartreuse"), "{message}");
    }

    #[test]
    fn a_scene_without_ink_still_parses_because_authored_docs_predate_it() {
        let scene: Scene =
            serde_json::from_str(r#"{"nodes":[{"id":"a","content":{"kind":"text","text":"hi"}}]}"#)
                .expect("pre-ink scene");
        assert!(matches!(
            scene.nodes[0].content,
            NodeContent::Text { ink: None, .. }
        ));
    }

    #[test]
    fn generates_valid_typed_scene_in_call_order() {
        let capabilities = LuaSceneCapabilities {
            assets: BTreeMap::from([("logo".into(), asset())]),
        };
        let scene = generate_scene(
            r#"
                canvas.group { id = "root" }
                canvas.text {
                    id = "title",
                    parent = "root",
                    text = "Fast, spatial, data-first",
                    translation = { 0, 1.5, -2 }
                }
                canvas.image { id = "logo", parent = "root", asset = "logo" }
            "#,
            &capabilities,
            LuaSceneLimits::default(),
        )
        .unwrap();

        assert_eq!(scene.nodes.len(), 3);
        assert_eq!(scene.nodes[1].parent_id.as_deref(), Some("root"));
        assert_eq!(scene.nodes[1].transform.translation, [0.0, 1.5, -2.0]);
        assert!(matches!(scene.nodes[2].content, NodeContent::Image { .. }));
    }

    #[test]
    fn host_access_and_ungranted_assets_are_absent() {
        let error = generate_scene(
            r#"
                assert(io == nil)
                assert(os == nil)
                assert(package == nil)
                assert(require == nil)
                assert(dofile == nil)
                assert(loadfile == nil)
                assert(loadstring == nil)
                assert(getfenv == nil)
                assert(setfenv == nil)
                assert(math.random == nil)
                assert(math.randomseed == nil)
                assert(not pcall(function() canvas.text = nil end))
                canvas.image { id = "secret", asset = "not-granted" }
            "#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap_err();

        assert!(error.0.contains("capability not granted"), "{error}");
    }

    #[test]
    fn rejects_unknown_fields_and_invalid_scene_graphs() {
        let unknown = generate_scene(
            r#"canvas.text { id = "title", text = "Hi", command = "open" }"#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap_err();
        assert!(unknown.0.contains("Unknown Canvas Lua node field"));

        let invalid = generate_scene(
            r#"canvas.group { id = "child", parent = "missing" }"#,
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap_err();
        assert!(
            invalid
                .0
                .contains("Unknown or self-referential scene parent")
        );
    }

    #[test]
    fn enforces_node_and_cpu_budgets() {
        let limits = LuaSceneLimits {
            max_nodes: 1,
            max_interrupts: 10,
            ..LuaSceneLimits::default()
        };
        let nodes = generate_scene(
            r#"
                canvas.group { id = "one" }
                canvas.group { id = "two" }
            "#,
            &LuaSceneCapabilities::default(),
            limits,
        )
        .unwrap_err();
        assert!(nodes.0.contains("node limit"));

        let runaway = generate_scene(
            "while true do end",
            &LuaSceneCapabilities::default(),
            limits,
        )
        .unwrap_err();
        assert!(runaway.0.contains("interrupt budget"));
    }

    #[test]
    fn each_execution_has_fresh_globals() {
        generate_scene(
            "temporary = 41",
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap();
        let error = generate_scene(
            "assert(temporary == nil); error('fresh')",
            &LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )
        .unwrap_err();
        assert!(error.0.contains("fresh"));
    }
}
