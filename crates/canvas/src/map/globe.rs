//! Continuous WGS84 globe camera and renderer-independent frame planning.
//!
//! This module deliberately owns no window, swapchain, or GPU texture. A WGPU
//! encoder consumes [`GlobeFramePlan`] and draws into a caller-owned target,
//! including a texture imported from the compositor's external-buffer path.

use super::{CameraResolution, MapSurface, spatial::SpatialPalette};
use crate::backend::{
    BackendCapabilities, BackendError, FrameOutcome, FrameRequest, MountContext, SurfaceMetrics,
    VisualBackend,
};
use canvas_core::ThemePalette;
use std::{f64::consts::PI, time::Duration};

pub const WGS84_SEMI_MAJOR_M: f64 = 6_378_137.0;
pub const WGS84_SEMI_MINOR_M: f64 = 6_356_752.314_245;
const MIN_DISTANCE_M: f64 = 2.0;
const MAX_DISTANCE_M: f64 = WGS84_SEMI_MAJOR_M * 5.0;
const INERTIA_CUTOFF_DEGREES_PER_SECOND: f64 = 0.000_001;
const INERTIA_DAMPING_PER_SECOND: f64 = 7.5;
const FIELD_OF_VIEW_Y_RADIANS: f64 = PI / 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobeCamera {
    pub latitude: f64,
    pub longitude: f64,
    pub distance_m: f64,
    pub bearing_degrees: f64,
    pub pitch_degrees: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobeFramePlan {
    /// ECEF origin subtracted on the CPU before values become `f32`.
    pub world_origin_ecef_m: [f64; 3],
    /// Column-major camera matrix for camera-relative coordinates.
    pub view_projection: [f32; 16],
    pub viewport_px: [u32; 2],
    pub palette: SpatialPalette,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlobeInput {
    PointerPressed { x: f64, y: f64 },
    PointerMoved { x: f64, y: f64 },
    PointerReleased,
    ZoomLines { delta: f64 },
    CancelGesture,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlobeEvent {
    CameraChanged(GlobeCamera),
}

pub struct GlobeBackend {
    camera: GlobeCamera,
    metrics: Option<SurfaceMetrics>,
    palette: SpatialPalette,
    reduced_motion: bool,
    pointer: Option<[f64; 2]>,
    velocity_degrees_per_second: [f64; 2],
    events: Vec<GlobeEvent>,
    last_frame_time: Option<Duration>,
    dirty: bool,
    mounted: bool,
}

impl GlobeBackend {
    pub fn new(theme: &ThemePalette) -> Self {
        Self {
            camera: GlobeCamera::default(),
            metrics: None,
            palette: SpatialPalette::from_theme(theme),
            reduced_motion: false,
            pointer: None,
            velocity_degrees_per_second: [0.0; 2],
            events: Vec::new(),
            last_frame_time: None,
            dirty: true,
            mounted: false,
        }
    }

    pub fn set_theme(&mut self, theme: &ThemePalette) {
        let palette = SpatialPalette::from_theme(theme);
        if palette != self.palette {
            self.palette = palette;
            self.dirty = true;
        }
    }

    pub fn camera(&self) -> GlobeCamera {
        self.camera
    }

    fn apply_model(&mut self, model: &MapSurface) -> Result<(), BackendError> {
        model
            .validate()
            .map_err(|error| BackendError::Invalid(error.0))?;
        let metrics = self.metrics.ok_or_else(|| {
            BackendError::Lifecycle("globe needs surface metrics before a model".into())
        })?;
        let resolved = model
            .camera
            .resolve(metrics)
            .map_err(|error| BackendError::Invalid(error.0))?;
        let CameraResolution::Resolved(camera) = resolved else {
            return Err(BackendError::Adapter(
                "globe camera target requires unresolved binding data".into(),
            ));
        };
        self.camera = GlobeCamera {
            latitude: camera.center.lat,
            longitude: camera.center.lon,
            distance_m: distance_for_zoom(camera.zoom),
            bearing_degrees: wrap_degrees(camera.bearing_degrees),
            pitch_degrees: camera.pitch_degrees,
        };
        self.velocity_degrees_per_second = [0.0; 2];
        self.dirty = true;
        Ok(())
    }

    fn orbit(&mut self, delta_x: f64, delta_y: f64) {
        let radians_per_pixel =
            (self.camera.distance_m / WGS84_SEMI_MAJOR_M).clamp(0.000_000_01, 3.0) * 0.002;
        let longitude_delta = -delta_x * radians_per_pixel.to_degrees();
        let latitude_delta = delta_y * radians_per_pixel.to_degrees();
        self.camera.longitude = wrap_longitude(self.camera.longitude + longitude_delta);
        self.camera.latitude =
            (self.camera.latitude + latitude_delta).clamp(-89.999_999, 89.999_999);
        self.velocity_degrees_per_second = [longitude_delta * 60.0, latitude_delta * 60.0];
        self.changed();
    }

    fn zoom(&mut self, delta: f64) {
        if delta.is_finite() {
            self.camera.distance_m = (self.camera.distance_m * (-delta * 0.18).exp())
                .clamp(MIN_DISTANCE_M, MAX_DISTANCE_M);
            self.velocity_degrees_per_second = [0.0; 2];
            self.changed();
        }
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.events.push(GlobeEvent::CameraChanged(self.camera));
    }

    fn advance_inertia(&mut self, now: Duration) {
        let Some(previous) = self.last_frame_time.replace(now) else {
            return;
        };
        if self.pointer.is_some() || self.reduced_motion {
            return;
        }
        let seconds = now.saturating_sub(previous).as_secs_f64().min(0.1);
        let speed = self.velocity_degrees_per_second[0]
            .abs()
            .max(self.velocity_degrees_per_second[1].abs());
        if speed <= INERTIA_CUTOFF_DEGREES_PER_SECOND {
            self.velocity_degrees_per_second = [0.0; 2];
            return;
        }
        self.camera.longitude =
            wrap_longitude(self.camera.longitude + self.velocity_degrees_per_second[0] * seconds);
        self.camera.latitude = (self.camera.latitude
            + self.velocity_degrees_per_second[1] * seconds)
            .clamp(-89.999_999, 89.999_999);
        let damping = (-INERTIA_DAMPING_PER_SECOND * seconds).exp();
        self.velocity_degrees_per_second[0] *= damping;
        self.velocity_degrees_per_second[1] *= damping;
        self.dirty = true;
    }

    fn frame_plan(&self) -> Result<GlobeFramePlan, BackendError> {
        let metrics = self
            .metrics
            .ok_or_else(|| BackendError::Lifecycle("globe is not mounted".into()))?;
        self.camera.frame_plan(metrics, self.palette)
    }
}

impl Default for GlobeCamera {
    fn default() -> Self {
        Self {
            latitude: 20.0,
            longitude: 0.0,
            distance_m: distance_for_zoom(0.0),
            bearing_degrees: 0.0,
            pitch_degrees: 0.0,
        }
    }
}

impl GlobeCamera {
    pub fn target_ecef_m(self) -> [f64; 3] {
        geodetic_to_ecef(self.latitude, self.longitude, 0.0)
    }

    pub fn frame_plan(
        self,
        metrics: SurfaceMetrics,
        palette: SpatialPalette,
    ) -> Result<GlobeFramePlan, BackendError> {
        metrics.validate()?;
        self.validate()?;

        let origin = self.target_ecef_m();
        let latitude = self.latitude.to_radians();
        let longitude = self.longitude.to_radians();
        let up = normalize3([
            latitude.cos() * longitude.cos(),
            latitude.cos() * longitude.sin(),
            latitude.sin(),
        ]);
        let east = normalize3([-longitude.sin(), longitude.cos(), 0.0]);
        let north = normalize3(cross3(up, east));
        let bearing = self.bearing_degrees.to_radians();
        let pitch = self.pitch_degrees.to_radians();
        let horizontal = add3(scale3(north, bearing.cos()), scale3(east, bearing.sin()));
        let eye_relative = add3(
            scale3(up, self.distance_m * pitch.cos()),
            scale3(horizontal, -self.distance_m * pitch.sin()),
        );
        let view = look_at(eye_relative, [0.0; 3], up);
        let physical_width = (metrics.logical_width * metrics.scale_factor)
            .round()
            .max(1.0);
        let physical_height = (metrics.logical_height * metrics.scale_factor)
            .round()
            .max(1.0);
        let aspect = physical_width / physical_height;
        let near = (self.distance_m * 0.000_001).clamp(0.05, 10_000.0);
        let far = self.distance_m + WGS84_SEMI_MAJOR_M * 3.0;
        let projection = perspective(FIELD_OF_VIEW_Y_RADIANS, aspect, near, far);

        Ok(GlobeFramePlan {
            world_origin_ecef_m: origin,
            view_projection: mat4_to_f32(mul4(projection, view)),
            viewport_px: [physical_width as u32, physical_height as u32],
            palette,
        })
    }

    fn validate(self) -> Result<(), BackendError> {
        if !self.latitude.is_finite()
            || !(-90.0..=90.0).contains(&self.latitude)
            || !self.longitude.is_finite()
            || !(-180.0..=180.0).contains(&self.longitude)
            || !self.distance_m.is_finite()
            || !(MIN_DISTANCE_M..=MAX_DISTANCE_M).contains(&self.distance_m)
            || !self.bearing_degrees.is_finite()
            || !self.pitch_degrees.is_finite()
            || !(0.0..=85.0).contains(&self.pitch_degrees)
        {
            Err(BackendError::Invalid("invalid globe camera".into()))
        } else {
            Ok(())
        }
    }
}

impl VisualBackend for GlobeBackend {
    type Model = MapSurface;
    type Frame = GlobeFramePlan;
    type Input = GlobeInput;
    type Event = GlobeEvent;

    fn id(&self) -> &'static str {
        "canvas-map-wgpu"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            interactive: true,
            offscreen_texture: true,
            continuous_updates: true,
        }
    }

    fn mount(&mut self, context: &MountContext, model: &Self::Model) -> Result<(), BackendError> {
        context.metrics.validate()?;
        self.metrics = Some(context.metrics);
        self.reduced_motion = context.reduced_motion;
        self.mounted = true;
        self.apply_model(model)
    }

    fn update(&mut self, _revision: u64, model: &Self::Model) -> Result<(), BackendError> {
        self.apply_model(model)
    }

    fn resize(&mut self, metrics: SurfaceMetrics) -> Result<(), BackendError> {
        metrics.validate()?;
        self.metrics = Some(metrics);
        self.dirty = true;
        Ok(())
    }

    fn input(&mut self, input: Self::Input) -> Result<(), BackendError> {
        if !self.mounted {
            return Err(BackendError::Lifecycle("globe is not mounted".into()));
        }
        match input {
            GlobeInput::PointerPressed { x, y } => {
                self.pointer = Some([x, y]);
                self.velocity_degrees_per_second = [0.0; 2];
            }
            GlobeInput::PointerMoved { x, y } => {
                if let Some(previous) = self.pointer {
                    self.pointer = Some([x, y]);
                    self.orbit(x - previous[0], y - previous[1]);
                }
            }
            GlobeInput::PointerReleased => {
                self.pointer = None;
                if self.reduced_motion {
                    self.velocity_degrees_per_second = [0.0; 2];
                }
            }
            GlobeInput::ZoomLines { delta } => self.zoom(delta),
            GlobeInput::CancelGesture => {
                self.pointer = None;
                self.velocity_degrees_per_second = [0.0; 2];
            }
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Result<Vec<Self::Event>, BackendError> {
        Ok(std::mem::take(&mut self.events))
    }

    fn render(&mut self, request: FrameRequest) -> Result<FrameOutcome<Self::Frame>, BackendError> {
        self.advance_inertia(request.monotonic_time);
        if self.dirty {
            self.dirty = false;
            Ok(FrameOutcome::Present(self.frame_plan()?))
        } else {
            Ok(FrameOutcome::Idle)
        }
    }

    fn suspend(&mut self) -> Result<(), BackendError> {
        self.pointer = None;
        self.velocity_degrees_per_second = [0.0; 2];
        Ok(())
    }

    fn resume(&mut self) -> Result<(), BackendError> {
        self.dirty = true;
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), BackendError> {
        self.mounted = false;
        self.metrics = None;
        self.events.clear();
        self.pointer = None;
        self.velocity_degrees_per_second = [0.0; 2];
        Ok(())
    }
}

/// @implements url:https://epsg.org/guidance-notes.html Guidance Note 7-2 §2.2.1 -- geographic to geocentric conversion on the WGS84 ellipsoid
pub fn geodetic_to_ecef(
    latitude_degrees: f64,
    longitude_degrees: f64,
    altitude_m: f64,
) -> [f64; 3] {
    let latitude = latitude_degrees.to_radians();
    let longitude = longitude_degrees.to_radians();
    let eccentricity_squared =
        1.0 - (WGS84_SEMI_MINOR_M * WGS84_SEMI_MINOR_M) / (WGS84_SEMI_MAJOR_M * WGS84_SEMI_MAJOR_M);
    let prime_vertical =
        WGS84_SEMI_MAJOR_M / (1.0 - eccentricity_squared * latitude.sin().powi(2)).sqrt();
    [
        (prime_vertical + altitude_m) * latitude.cos() * longitude.cos(),
        (prime_vertical + altitude_m) * latitude.cos() * longitude.sin(),
        (prime_vertical * (1.0 - eccentricity_squared) + altitude_m) * latitude.sin(),
    ]
}

fn distance_for_zoom(zoom: f64) -> f64 {
    (WGS84_SEMI_MAJOR_M * 2.5 / 2.0_f64.powf(zoom)).clamp(MIN_DISTANCE_M, MAX_DISTANCE_M)
}

fn wrap_degrees(value: f64) -> f64 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

fn wrap_longitude(value: f64) -> f64 {
    wrap_degrees(value)
}

type Mat4 = [[f64; 4]; 4];

fn perspective(fov_y: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, (near * far) / (near - far), 0.0],
    ]
}

fn look_at(eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> Mat4 {
    let forward = normalize3(sub3(target, eye));
    let right = normalize3(cross3(forward, up));
    let camera_up = cross3(right, forward);
    [
        [right[0], camera_up[0], -forward[0], 0.0],
        [right[1], camera_up[1], -forward[1], 0.0],
        [right[2], camera_up[2], -forward[2], 0.0],
        [
            -dot3(right, eye),
            -dot3(camera_up, eye),
            dot3(forward, eye),
            1.0,
        ],
    ]
}

fn mul4(left: Mat4, right: Mat4) -> Mat4 {
    let mut result = [[0.0; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            result[column][row] = (0..4)
                .map(|index| left[index][row] * right[column][index])
                .sum();
        }
    }
    result
}

fn mat4_to_f32(matrix: Mat4) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            result[column * 4 + row] = matrix[column][row] as f32;
        }
    }
    result
}

fn add3(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn sub3(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn scale3(value: [f64; 3], scale: f64) -> [f64; 3] {
    [value[0] * scale, value[1] * scale, value[2] * scale]
}

fn dot3(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn cross3(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn normalize3(value: [f64; 3]) -> [f64; 3] {
    scale3(value, 1.0 / dot3(value, value).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{
        CameraIntent, CameraTarget, CameraTransition, GeoPoint, MapPadding, MapResourceRef,
    };
    use serde_json::json;

    fn theme() -> ThemePalette {
        serde_json::from_value(json!({
            "background":"#101014","background_strong":"#08090c",
            "surface":"#20242c","surface_muted":"#171a20",
            "text":"#f2f4f8","text_muted":"#9299a6","primary":"#66ccff",
            "success":"#57d38c","warning":"#f0bd5a","danger":"#f06a73",
            "info":"#7aa7ff","border":"#343a46","border_strong":"#525b6b"
        }))
        .unwrap()
    }

    fn map(zoom: f64) -> MapSurface {
        MapSurface {
            resource: MapResourceRef {
                owner: "panopticon".into(),
                id: "live".into(),
                contract: "panopticon.spatial.snapshot.v1".into(),
            },
            camera: CameraIntent {
                target: CameraTarget::CenterZoom {
                    center: GeoPoint {
                        lat: 47.6,
                        lon: -122.3,
                    },
                    zoom,
                },
                bearing_degrees: 0.0,
                pitch_degrees: 0.0,
                padding: MapPadding::default(),
                transition: CameraTransition::Jump,
            },
            layers: vec![],
        }
    }

    fn context(reduced_motion: bool) -> MountContext {
        MountContext {
            surface_id: "atlas".into(),
            metrics: SurfaceMetrics {
                logical_width: 1200.0,
                logical_height: 800.0,
                scale_factor: 2.0,
            },
            reduced_motion,
        }
    }

    #[test]
    fn wgs84_reference_positions_are_correct() {
        let equator = geodetic_to_ecef(0.0, 0.0, 0.0);
        assert!((equator[0] - WGS84_SEMI_MAJOR_M).abs() < 0.000_001);
        assert!(equator[1].abs() < 0.000_001);
        assert!(equator[2].abs() < 0.000_001);

        let pole = geodetic_to_ecef(90.0, 0.0, 0.0);
        assert!((pole[2] - WGS84_SEMI_MINOR_M).abs() < 0.000_001);
    }

    #[test]
    fn zoom_is_one_continuous_exponential_camera() {
        let whole_earth = distance_for_zoom(0.0);
        let regional = distance_for_zoom(8.0);
        let street = distance_for_zoom(22.0);
        assert!(whole_earth > regional && regional > street);
        assert_eq!(
            distance_for_zoom(8.5),
            WGS84_SEMI_MAJOR_M * 2.5 / 2.0_f64.powf(8.5)
        );
        assert!(street >= MIN_DISTANCE_M);
    }

    #[test]
    fn frame_plan_keeps_f64_origin_and_finite_f32_camera_matrix() {
        let camera = GlobeCamera {
            latitude: 47.6,
            longitude: -122.3,
            distance_m: 2_000.0,
            bearing_degrees: 20.0,
            pitch_degrees: 60.0,
        };
        let plan = camera
            .frame_plan(context(false).metrics, SpatialPalette::from_theme(&theme()))
            .unwrap();
        assert!(
            plan.world_origin_ecef_m
                .iter()
                .all(|value| value.is_finite())
        );
        assert!(plan.view_projection.iter().all(|value| value.is_finite()));
        assert_eq!(plan.viewport_px, [2400, 1600]);
    }

    #[test]
    fn input_orbits_without_a_projection_switch_and_then_idles() {
        let mut backend = GlobeBackend::new(&theme());
        backend.mount(&context(true), &map(3.0)).unwrap();
        let initial_distance = backend.camera().distance_m;
        backend
            .input(GlobeInput::PointerPressed { x: 100.0, y: 100.0 })
            .unwrap();
        backend
            .input(GlobeInput::PointerMoved { x: 140.0, y: 120.0 })
            .unwrap();
        backend.input(GlobeInput::PointerReleased).unwrap();
        backend
            .input(GlobeInput::ZoomLines { delta: 1.25 })
            .unwrap();
        assert!(backend.camera().distance_m < initial_distance);

        let first = backend
            .render(FrameRequest {
                sequence: 1,
                monotonic_time: Duration::from_secs(1),
            })
            .unwrap();
        assert!(matches!(first, FrameOutcome::Present(_)));
        let second = backend
            .render(FrameRequest {
                sequence: 2,
                monotonic_time: Duration::from_secs(2),
            })
            .unwrap();
        assert_eq!(second, FrameOutcome::Idle);
    }

    #[test]
    fn theme_changes_invalidate_exactly_one_frame() {
        let mut backend = GlobeBackend::new(&theme());
        backend.mount(&context(true), &map(2.0)).unwrap();
        let request = FrameRequest {
            sequence: 1,
            monotonic_time: Duration::ZERO,
        };
        assert!(matches!(
            backend.render(request).unwrap(),
            FrameOutcome::Present(_)
        ));
        assert_eq!(backend.render(request).unwrap(), FrameOutcome::Idle);

        let mut changed = theme();
        changed.primary = [255, 0, 255];
        backend.set_theme(&changed);
        assert!(matches!(
            backend.render(request).unwrap(),
            FrameOutcome::Present(_)
        ));
        assert_eq!(backend.render(request).unwrap(), FrameOutcome::Idle);
    }

    #[test]
    fn pointer_motion_without_a_press_does_not_orbit() {
        let mut backend = GlobeBackend::new(&theme());
        backend.mount(&context(false), &map(4.0)).unwrap();
        let before = backend.camera();
        backend
            .input(GlobeInput::PointerMoved { x: 400.0, y: 300.0 })
            .unwrap();
        backend
            .input(GlobeInput::PointerMoved { x: 450.0, y: 350.0 })
            .unwrap();
        assert_eq!(backend.camera(), before);
    }
}
