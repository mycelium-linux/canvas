//! Bounded, change-time Lua scene construction.
//!
//! Scripts return declarative desired state. They receive no filesystem,
//! process, clock, network, or GPU handles and are never retained in the frame
//! loop; the validated [`SceneState`](crate::SceneState) is.

use crate::SceneState;
use anyhow::{Result, anyhow, ensure};
use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_INTERRUPTS: usize = 20_000;

pub fn evaluate(source: &str) -> Result<SceneState> {
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

    #[test]
    fn lua_has_no_process_or_filesystem_surface() {
        assert!(evaluate("return os.execute('id')").is_err());
    }
}
