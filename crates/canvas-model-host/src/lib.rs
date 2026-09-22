//! Retained scene contract for Canvas's remote GPU model renderer.
//!
//! The renderer owns GPU resources and animation. Callers send bounded desired
//! state; they never submit shaders, paths, commands, or per-frame callbacks.

use anyhow::{Result, ensure};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod lua;
pub mod mesh;
pub mod renderer;

pub const MODEL_SCENE_CONTRACT: &str = "canvas.model.scene.v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraPreset {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    #[default]
    Iso,
    IsoRear,
}

impl CameraPreset {
    pub fn direction(self) -> Vec3 {
        match self {
            Self::Top => Vec3::Y,
            Self::Bottom => Vec3::NEG_Y,
            Self::Front => Vec3::Z,
            Self::Back => Vec3::NEG_Z,
            Self::Left => Vec3::NEG_X,
            Self::Right => Vec3::X,
            Self::Iso => Vec3::new(1.0, 0.8, 1.0).normalize(),
            Self::IsoRear => Vec3::new(-1.0, 0.8, -1.0).normalize(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orbit {
    #[serde(default = "default_orbit_axis")]
    pub axis: [f32; 3],
    #[serde(default = "default_orbit_speed")]
    pub degrees_per_second: f32,
}

fn default_orbit_axis() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}
fn default_orbit_speed() -> f32 {
    8.0
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            axis: default_orbit_axis(),
            degrees_per_second: default_orbit_speed(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    /// Absent means inherit the active Canvas theme's primary colour.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default = "default_glow")]
    pub glow: f32,
    #[serde(default)]
    pub wireframe: bool,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

fn default_glow() -> f32 {
    1.15
}
fn default_opacity() -> f32 {
    0.92
}

impl Default for Material {
    fn default() -> Self {
        Self {
            color: None,
            glow: default_glow(),
            wireframe: false,
            opacity: default_opacity(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneState {
    pub scene_id: String,
    /// Content-addressed asset id resolved beneath the configured asset root.
    pub asset: String,
    /// Asset-local Euler rotation in degrees (X, Y, Z). Coordinate conventions
    /// belong to the model/scene definition, never to the renderer globally.
    #[serde(default)]
    pub model_rotation_degrees: [f32; 3],
    #[serde(default)]
    pub camera: CameraPreset,
    #[serde(default)]
    pub orbit: Option<Orbit>,
    #[serde(default)]
    pub material: Material,
    #[serde(default = "default_fly_in_ms")]
    pub transition_ms: u32,
    #[serde(default = "default_true")]
    pub fly_in: bool,
}

fn default_fly_in_ms() -> u32 {
    850
}
fn default_true() -> bool {
    true
}

impl SceneState {
    pub fn validate(&self) -> Result<()> {
        ensure!(safe_id(&self.scene_id), "unsafe scene_id");
        ensure!(
            safe_asset(&self.asset),
            "asset must be a content id ending in .stl"
        );
        ensure!(self.transition_ms <= 5_000, "transition_ms exceeds 5000");
        ensure!(
            self.model_rotation_degrees
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 360.0),
            "model rotation must contain finite values within +/-360 degrees"
        );
        ensure!(
            (0.0..=4.0).contains(&self.material.glow),
            "glow must be 0..=4"
        );
        ensure!(
            (0.05..=1.0).contains(&self.material.opacity),
            "opacity must be 0.05..=1"
        );
        if let Some(orbit) = self.orbit {
            let axis = Vec3::from_array(orbit.axis);
            ensure!(
                axis.is_finite() && axis.length_squared() > 0.001,
                "invalid orbit axis"
            );
            ensure!(
                orbit.degrees_per_second.abs() <= 90.0,
                "orbit speed exceeds 90 deg/s"
            );
        }
        Ok(())
    }

    pub fn resolve_asset(&self, root: &std::path::Path) -> Result<PathBuf> {
        self.validate()?;
        Ok(root.join(&self.asset))
    }
}

pub fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn safe_asset(value: &str) -> bool {
    value.len() <= 128
        && value.ends_with(".stl")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// What a scene is told about the meshes on disk.
///
/// Scenes are handed no filesystem, so a scene that wants to follow a build
/// cannot go looking. The host enumerates; the scene decides. Which step to
/// show and how to frame it stays scene policy instead of becoming renderer
/// behaviour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneContext {
    /// Every renderable mesh beneath the asset root, in build order.
    pub steps: Vec<String>,
    /// The most recent one, so following a build is a single lookup.
    ///
    /// Omitted rather than null when there is nothing yet: mlua serializes
    /// `None` as a NULL sentinel, which Lua's `or` treats as *present*, so a
    /// scene's `assets.latest or fallback` would have picked the sentinel and
    /// failed to decode. An absent key is genuinely `nil`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
}

impl SceneContext {
    pub fn scan(root: &std::path::Path) -> Self {
        let mut steps: Vec<String> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| safe_asset(name))
            .collect();
        // `step-10` comes after `step-9`, which a lexical sort gets backwards —
        // a viewer would show the build lurching about instead of progressing.
        steps.sort_by(|left, right| {
            step_ordinal(left)
                .cmp(&step_ordinal(right))
                .then_with(|| left.cmp(right))
        });
        let latest = steps.last().cloned();
        Self { steps, latest }
    }
}

/// The trailing integer in a mesh filename, if it has one.
fn step_ordinal(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".stl")?;
    let digits: String = stem
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Critically damped camera transition with a deliberate overscan fly-in.
#[derive(Debug, Clone, Copy)]
pub struct CameraFlight {
    from: Vec3,
    to: Vec3,
    duration_seconds: f32,
    fly_in: bool,
}

impl CameraFlight {
    pub fn new(from: Vec3, preset: CameraPreset, duration_ms: u32, fly_in: bool) -> Self {
        Self {
            from: from.normalize_or_zero(),
            to: preset.direction(),
            duration_seconds: (duration_ms.max(1) as f32) / 1_000.0,
            fly_in,
        }
    }

    pub fn sample(self, elapsed_seconds: f32) -> (Vec3, f32) {
        let t = (elapsed_seconds / self.duration_seconds).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        let direction = self.from.lerp(self.to, smooth).normalize_or_zero();
        // Start beyond the resting distance, accelerate inward, and settle
        // without a discontinuity. Camera snaps use the same path with fly_in=false.
        let distance_scale = if self.fly_in {
            1.0 + (1.0 - smooth).powi(2) * 1.25
        } else {
            1.0
        };
        (direction, distance_scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_is_normalized_and_elevated() {
        let direction = CameraPreset::Iso.direction();
        assert!((direction.length() - 1.0).abs() < 1e-5);
        assert!(direction.y > 0.0 && direction.x > 0.0 && direction.z > 0.0);
    }

    #[test]
    fn fly_in_settles_exactly_on_target() {
        let flight = CameraFlight::new(Vec3::Z, CameraPreset::Top, 800, true);
        let (direction, scale) = flight.sample(0.8);
        assert!((direction - Vec3::Y).length() < 1e-5);
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn scene_rejects_paths_and_unbounded_animation() {
        let mut scene = SceneState {
            scene_id: "part".into(),
            asset: "../part.stl".into(),
            model_rotation_degrees: [0.0; 3],
            camera: CameraPreset::Iso,
            orbit: None,
            material: Material::default(),
            transition_ms: 850,
            fly_in: true,
        };
        assert!(scene.validate().is_err());
        scene.asset = "sha256-deadbeef.stl".into();
        scene.transition_ms = 9_000;
        assert!(scene.validate().is_err());
    }
}

#[cfg(test)]
mod scene_context_tests {
    use super::*;

    fn touch(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), b"solid\n").expect("write mesh");
    }

    /// A viewer following a build must see step 10 as the newest, not step 9.
    /// A lexical sort puts "step-10.stl" before "step-2.stl", which would show
    /// the part lurching backwards mid-build.
    #[test]
    fn steps_are_ordered_by_number_not_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["step-2.stl", "step-10.stl", "step-9.stl", "step-0.stl"] {
            touch(dir.path(), name);
        }
        let assets = SceneContext::scan(dir.path());
        assert_eq!(
            assets.steps,
            vec!["step-0.stl", "step-2.stl", "step-9.stl", "step-10.stl"]
        );
        assert_eq!(assets.latest.as_deref(), Some("step-10.stl"));
    }

    /// Only meshes the renderer will accept are offered, so a scene cannot be
    /// handed a name that fails the bake.
    #[test]
    fn only_renderable_meshes_are_offered() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "step-0.stl");
        touch(dir.path(), "status.json");
        touch(dir.path(), "notes.txt");
        let assets = SceneContext::scan(dir.path());
        assert_eq!(assets.steps, vec!["step-0.stl"]);
    }

    #[test]
    fn an_empty_build_offers_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let assets = SceneContext::scan(dir.path());
        assert!(assets.steps.is_empty());
        assert!(assets.latest.is_none());
    }

    /// The point of the whole change: the scene picks the step, and picks a new
    /// one as the build produces it, with nothing rewriting the scene file.
    #[test]
    fn the_scene_follows_the_build_without_being_edited() {
        let source = r#"
            return {
              scene_id = "transmog-build",
              asset = assets.latest or "fallback.stl",
              fly_in = false,
            }
        "#;
        let dir = tempfile::tempdir().expect("tempdir");

        let empty = crate::lua::evaluate_with(source, &SceneContext::scan(dir.path()))
            .expect("scene evaluates with no steps");
        assert_eq!(empty.asset, "fallback.stl");

        touch(dir.path(), "step-0.stl");
        let first = crate::lua::evaluate_with(source, &SceneContext::scan(dir.path()))
            .expect("scene evaluates");
        assert_eq!(first.asset, "step-0.stl");

        touch(dir.path(), "step-1.stl");
        let second = crate::lua::evaluate_with(source, &SceneContext::scan(dir.path()))
            .expect("scene evaluates");
        assert_eq!(second.asset, "step-1.stl", "same source, newer step");
    }

    /// The shipped scene must work under StdLib::NONE — no math, string, or
    /// table library is available to it.
    #[test]
    fn the_shipped_transmog_scene_evaluates_in_the_sandbox() {
        let source = include_str!("../../../config/model-scenes/transmog-build.lua");
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..3 {
            touch(dir.path(), &format!("step-{index}.stl"));
        }
        let scene = crate::lua::evaluate_with(source, &SceneContext::scan(dir.path()))
            .expect("shipped transmog scene evaluates");
        assert_eq!(scene.asset, "step-2.stl");
        assert!(!scene.fly_in, "each step is a cut, not a camera re-fly");
    }
}
