//! Capability-scoped Luau scene generation.
//!
//! Scripts receive a single `canvas` table. They can append typed scene nodes,
//! but cannot read files, load packages, open sockets, launch processes, or
//! mutate workspace state.

use super::{
    AssetRef, Error, NodeContent, Result, Scene, SceneNode, SceneTint, Transform3, identifier,
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
                    &["id", "parent", "translation", "rotation", "scale", "tint"],
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
                let node = parse_node(
                    &spec,
                    &["id", "parent", "text", "translation", "rotation", "scale", "tint"],
                    NodeContent::Text { text },
                )?;
                append_node(&text_nodes, max_nodes, node)
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
    // An unknown role is refused rather than silently falling back to the default:
    // a typo would otherwise paint an error node the same colour as a healthy one,
    // which is the failure this whole field exists to prevent.
    let tint = match spec.get::<Option<String>>("tint")? {
        Some(name) => SceneTint::parse(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("Unknown Canvas scene tint: {name}"))
        })?,
        None => SceneTint::default(),
    };
    Ok(SceneNode {
        id: required_string(spec, "id")?,
        parent_id: spec.get("parent")?,
        transform: parse_transform(spec)?,
        content,
        tint,
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
