//! Bounded, change-time Lua scene construction.
//!
//! Scripts return declarative desired state. They receive no filesystem,
//! process, clock, network, or GPU handles and are never retained in the frame
//! loop; the validated [`SceneState`](crate::SceneState) is.
//!
//! A scene that follows a build still needs to know what exists, so the host
//! enumerates the asset root and passes the result in as a plain table. That
//! keeps the choice of *which* mesh to show in the scene, where it belongs,
//! without handing the sandbox a directory to read.

use crate::{SceneContext, SceneState};
use anyhow::{Result, anyhow, ensure};
use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_INTERRUPTS: usize = 20_000;

pub fn evaluate(source: &str) -> Result<SceneState> {
    evaluate_with(source, &SceneContext::default())
}

pub fn evaluate_with(source: &str, context: &SceneContext) -> Result<SceneState> {
    ensure!(
        source.len() <= MAX_SCRIPT_BYTES,
        "model scene Lua exceeds 64 KiB"
    );
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|error| anyhow!(error.to_string()))?;
    let interrupts = Arc::new(AtomicUsize::new(0));
    lua.set_interrupt({
        let interrupts = Arc::clone(&interrupts);
        move |_| {
            if interrupts.fetch_add(1, Ordering::Relaxed) >= MAX_INTERRUPTS {
                Err(mlua::Error::RuntimeError(
                    "model scene Lua instruction budget exceeded".into(),
                ))
            } else {
                Ok(VmState::Continue)
            }
        }
    });
    let assets = lua
        .to_value(context)
        .map_err(|error| anyhow!("encode model scene context: {error}"))?;
    lua.globals()
        .set("assets", assets)
        .map_err(|error| anyhow!("bind model scene context: {error}"))?;
    let value: Value = lua
        .load(source)
        .set_name("model-scene")
        .eval()
        .map_err(|error| anyhow!("evaluate model scene Lua: {error}"))?;
    let scene: SceneState = lua
        .from_value(value)
        .map_err(|error| anyhow!("decode model scene Lua result: {error}"))?;
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CameraPreset;

    #[test]
    fn lua_constructs_wireframe_orbit_once() {
        let scene = evaluate(
            r#"
            return {
              scene_id = "bearing",
              asset = "sha256-bearing.stl",
              camera = "iso",
              orbit = { axis = {0, 1, 0}, degrees_per_second = 12 },
              material = { color = {0, 220, 255}, glow = 1.4, wireframe = true, opacity = 0.85 },
              transition_ms = 700,
              fly_in = true,
            }
        "#,
        )
        .unwrap();
        assert_eq!(scene.camera, CameraPreset::Iso);
        assert!(scene.material.wireframe);
        assert_eq!(scene.orbit.unwrap().degrees_per_second, 12.0);
    }

    #[test]
    fn model_coordinate_adaptation_is_scene_local() {
        let scene = evaluate(
            r#"
            return {
              scene_id = "z-up-part",
              asset = "sha256-z-up-part.stl",
              model_rotation_degrees = {-90, 0, 0},
            }
        "#,
        )
        .unwrap();

        assert_eq!(scene.model_rotation_degrees, [-90.0, 0.0, 0.0]);

        let unadapted = evaluate(
            r#"
            return {
              scene_id = "native-part",
              asset = "sha256-native-part.stl",
            }
        "#,
        )
        .unwrap();
        assert_eq!(unadapted.model_rotation_degrees, [0.0, 0.0, 0.0]);
    }

    /// A scene declares a field the same way it declares a material — through
    /// serde — so painting needs no binding code of its own. This is the check
    /// that the Lua spelling and the Rust shape actually line up.
    #[test]
    fn lua_declares_a_thermal_field() {
        let scene = evaluate(
            r#"
            return {
              scene_id = "thermal",
              asset = "step-7.stl",
              camera = "iso",
              material = { glow = 1.2, wireframe = false, opacity = 0.9 },
              field = {
                name = "thermal",
                unit = "C",
                range = {20, 180},
                colormap = "inferno",
                source = { kind = "point_heat", at = {0, 0, 0}, power = 160 },
              },
              transition_ms = 260,
              fly_in = false,
            }
            "#,
        )
        .expect("scene evaluates");

        let field = scene.field.expect("field declared");
        assert_eq!(field.name, "thermal");
        assert_eq!(field.colormap, crate::Colormap::Inferno);
        // Defaults fill in: ambient 20C, falloff 1.
        assert!((field.sample([0.0, 0.0, 0.0]) - 180.0).abs() < 1e-3);
        assert_eq!(field.normalize(180.0), 1.0);
    }

    #[test]
    fn a_scene_without_a_field_paints_nothing() {
        let scene = evaluate(
            r#"
            return {
              scene_id = "plain", asset = "a.stl", camera = "iso",
              material = { glow = 1.0, wireframe = false, opacity = 1.0 },
              transition_ms = 0, fly_in = false,
            }
            "#,
        )
        .expect("scene evaluates");
        assert!(scene.field.is_none(), "painting must be opt-in");
    }

    #[test]
    fn lua_has_no_process_or_filesystem_surface() {
        assert!(evaluate("return os.execute('id')").is_err());
    }
}
