//! Renderer-neutral map presentation intent.
//!
//! Canvas owns camera behavior and presentation. A data owner such as
//! Panopticon resolves names and supplies authorized geographic projections;
//! a backend such as MapLibre renders them. This module depends on neither.

use crate::backend::{BackendError, SurfaceMetrics, VisualBackend};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error, f64::consts::PI, fmt};

pub mod globe;
pub mod spatial;

const MAX_ZOOM: f64 = 22.0;
const MAX_PITCH: f64 = 85.0;
const MAX_LAYERS: usize = 64;
const MAX_TRANSITION_MS: u32 = 30_000;
const WEB_MERCATOR_MAX_LAT: f64 = 85.051_128_78;
const MAPLIBRE_TILE_SIZE: f64 = 512.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapSurface {
    /// Opaque authorized resource. It is an identity, never an endpoint or key.
    pub resource: MapResourceRef,
    pub camera: CameraIntent,
    #[serde(default)]
    pub layers: Vec<MapLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapResourceRef {
    pub owner: String,
    pub id: String,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapLayer {
    pub id: String,
    /// Workspace input binding carrying this layer's data projection.
    pub binding_id: String,
    /// Semantic role resolved through the active project theme/map style.
    pub style_role: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub min_zoom: f64,
    #[serde(default = "default_max_zoom")]
    pub max_zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraIntent {
    pub target: CameraTarget,
    #[serde(default)]
    pub bearing_degrees: f64,
    #[serde(default)]
    pub pitch_degrees: f64,
    #[serde(default)]
    pub padding: MapPadding,
    #[serde(default)]
    pub transition: CameraTransition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum CameraTarget {
    Extent {
        extent: GeoExtent,
    },
    CenterZoom {
        center: GeoPoint,
        zoom: f64,
    },
    FollowEntity {
        binding_id: String,
        entity_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zoom: Option<f64>,
    },
    FitBinding {
        binding_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_zoom: Option<f64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct GeoExtent {
    pub bounds: GeoBounds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<GeoSourceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct GeoSourceRef {
    pub owner: String,
    pub id: String,
    pub revision: String,
}

/// WGS84 bounds. `west > east` deliberately represents an antimeridian span.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct GeoBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapPadding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Default for MapPadding {
    fn default() -> Self {
        Self {
            top: 32.0,
            right: 32.0,
            bottom: 32.0,
            left: 32.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum CameraTransition {
    #[default]
    Jump,
    Ease {
        duration_ms: u32,
    },
    Fly {
        duration_ms: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedCamera {
    pub center: GeoPoint,
    pub zoom: f64,
    pub bearing_degrees: f64,
    pub pitch_degrees: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CameraResolution {
    Resolved(ResolvedCamera),
    NeedsEntity {
        binding_id: String,
        entity_id: String,
        zoom: Option<f64>,
    },
    NeedsBindingExtent {
        binding_id: String,
        max_zoom: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveTransition {
    Jump,
    Ease { duration_ms: u32 },
    Fly { duration_ms: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError(pub String);

impl fmt::Display for MapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl error::Error for MapError {}

impl From<BackendError> for MapError {
    fn from(value: BackendError) -> Self {
        Self(value.to_string())
    }
}

/// Marker trait for interchangeable map engines hosted by Canvas.
pub trait MapBackend: VisualBackend<Model = MapSurface> {}

impl<T> MapBackend for T where T: VisualBackend<Model = MapSurface> {}

impl MapSurface {
    pub fn validate(&self) -> Result<(), MapError> {
        self.resource.validate()?;
        self.camera.validate()?;
        if self.layers.len() > MAX_LAYERS {
            return Err(MapError("a map may contain at most 64 layers".into()));
        }
        let mut layer_ids = BTreeSet::new();
        for layer in &self.layers {
            if !identifier(&layer.id) || !layer_ids.insert(layer.id.as_str()) {
                return Err(MapError(format!(
                    "invalid or duplicate map layer id: {}",
                    layer.id
                )));
            }
            if !identifier(&layer.binding_id) || !contract(&layer.style_role) {
                return Err(MapError(format!("invalid map layer: {}", layer.id)));
            }
            if !valid_zoom(layer.min_zoom)
                || !valid_zoom(layer.max_zoom)
                || layer.min_zoom > layer.max_zoom
            {
                return Err(MapError(format!(
                    "invalid zoom range for map layer: {}",
                    layer.id
                )));
            }
        }
        Ok(())
    }
}

impl MapResourceRef {
    fn validate(&self) -> Result<(), MapError> {
        if identifier(&self.owner) && identifier(&self.id) && contract(&self.contract) {
            Ok(())
        } else {
            Err(MapError("invalid map resource reference".into()))
        }
    }
}

impl CameraIntent {
    pub fn validate(&self) -> Result<(), MapError> {
        self.target.validate()?;
        if !self.bearing_degrees.is_finite() || !(-360.0..=360.0).contains(&self.bearing_degrees) {
            return Err(MapError(
                "camera bearing must be between -360 and 360".into(),
            ));
        }
        if !self.pitch_degrees.is_finite() || !(0.0..=MAX_PITCH).contains(&self.pitch_degrees) {
            return Err(MapError("camera pitch must be between 0 and 85".into()));
        }
        self.padding.validate()?;
        self.transition.validate()
    }

    pub fn resolve(&self, metrics: SurfaceMetrics) -> Result<CameraResolution, MapError> {
        self.validate()?;
        metrics.validate()?;
        match &self.target {
            CameraTarget::Extent { extent } => Ok(CameraResolution::Resolved(fit_extent(
                extent.bounds,
                metrics,
                self.padding,
                MAX_ZOOM,
                self.bearing_degrees,
                self.pitch_degrees,
            )?)),
            CameraTarget::CenterZoom { center, zoom } => {
                Ok(CameraResolution::Resolved(ResolvedCamera {
                    center: *center,
                    zoom: *zoom,
                    bearing_degrees: self.bearing_degrees,
                    pitch_degrees: self.pitch_degrees,
                }))
            }
            CameraTarget::FollowEntity {
                binding_id,
                entity_id,
                zoom,
            } => Ok(CameraResolution::NeedsEntity {
                binding_id: binding_id.clone(),
                entity_id: entity_id.clone(),
                zoom: *zoom,
            }),
            CameraTarget::FitBinding {
                binding_id,
                max_zoom,
            } => Ok(CameraResolution::NeedsBindingExtent {
                binding_id: binding_id.clone(),
                max_zoom: *max_zoom,
            }),
        }
    }

    pub fn effective_transition(&self, reduced_motion: bool) -> EffectiveTransition {
        if reduced_motion {
            return EffectiveTransition::Jump;
        }
        match self.transition {
            CameraTransition::Jump => EffectiveTransition::Jump,
            CameraTransition::Ease { duration_ms } => EffectiveTransition::Ease { duration_ms },
            CameraTransition::Fly { duration_ms } => EffectiveTransition::Fly { duration_ms },
        }
    }
}

impl CameraTarget {
    fn validate(&self) -> Result<(), MapError> {
        match self {
            Self::Extent { extent } => extent.validate(),
            Self::CenterZoom { center, zoom } => {
                center.validate()?;
                if valid_zoom(*zoom) {
                    Ok(())
                } else {
                    Err(MapError("camera zoom must be between 0 and 22".into()))
                }
            }
            Self::FollowEntity {
                binding_id,
                entity_id,
                zoom,
            } => {
                if !identifier(binding_id) || !visible(entity_id, 256) {
                    return Err(MapError("invalid follow-entity target".into()));
                }
                if zoom.is_some_and(|value| !valid_zoom(value)) {
                    return Err(MapError(
                        "follow-entity zoom must be between 0 and 22".into(),
                    ));
                }
                Ok(())
            }
            Self::FitBinding {
                binding_id,
                max_zoom,
            } => {
                if !identifier(binding_id) {
                    return Err(MapError("invalid fit-binding target".into()));
                }
                if max_zoom.is_some_and(|value| !valid_zoom(value)) {
                    return Err(MapError("fit-binding zoom must be between 0 and 22".into()));
                }
                Ok(())
            }
        }
    }
}

impl GeoExtent {
    fn validate(&self) -> Result<(), MapError> {
        self.bounds.validate()?;
        if self
            .label
            .as_ref()
            .is_some_and(|label| !visible(label, 256))
        {
            return Err(MapError("geographic extent label is invalid".into()));
        }
        if let Some(source) = &self.source
            && (!identifier(&source.owner)
                || !visible(&source.id, 256)
                || !visible(&source.revision, 128))
        {
            return Err(MapError("geographic extent source is invalid".into()));
        }
        Ok(())
    }
}

impl GeoBounds {
    pub fn validate(self) -> Result<(), MapError> {
        let longitude_span = self.longitude_span();
        if [self.west, self.south, self.east, self.north]
            .iter()
            .all(|value| value.is_finite())
            && (-180.0..=180.0).contains(&self.west)
            && (-180.0..=180.0).contains(&self.east)
            && (-90.0..=90.0).contains(&self.south)
            && (-90.0..=90.0).contains(&self.north)
            && self.north > self.south
            && longitude_span.is_finite()
            && longitude_span > 0.0
            && longitude_span <= 360.0
        {
            Ok(())
        } else {
            Err(MapError(
                "geographic bounds must form a non-empty WGS84 extent".into(),
            ))
        }
    }

    fn longitude_span(self) -> f64 {
        if self.east < self.west {
            self.east + 360.0 - self.west
        } else {
            self.east - self.west
        }
    }
}

impl GeoPoint {
    fn validate(self) -> Result<(), MapError> {
        if self.lat.is_finite()
            && (-90.0..=90.0).contains(&self.lat)
            && self.lon.is_finite()
            && (-180.0..=180.0).contains(&self.lon)
        {
            Ok(())
        } else {
            Err(MapError("geographic point must be valid WGS84".into()))
        }
    }
}

impl MapPadding {
    fn validate(self) -> Result<(), MapError> {
        if [self.top, self.right, self.bottom, self.left]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0 && *value <= 16_384.0)
        {
            Ok(())
        } else {
            Err(MapError(
                "map padding must be finite, non-negative, and bounded".into(),
            ))
        }
    }
}

impl CameraTransition {
    fn validate(self) -> Result<(), MapError> {
        let duration = match self {
            Self::Jump => return Ok(()),
            Self::Ease { duration_ms } | Self::Fly { duration_ms } => duration_ms,
        };
        if (16..=MAX_TRANSITION_MS).contains(&duration) {
            Ok(())
        } else {
            Err(MapError(format!(
                "camera transition duration must be between 16 and {MAX_TRANSITION_MS} ms"
            )))
        }
    }
}

fn fit_extent(
    bounds: GeoBounds,
    metrics: SurfaceMetrics,
    padding: MapPadding,
    max_zoom: f64,
    bearing_degrees: f64,
    pitch_degrees: f64,
) -> Result<ResolvedCamera, MapError> {
    bounds.validate()?;
    let width = metrics.logical_width - padding.left - padding.right;
    let height = metrics.logical_height - padding.top - padding.bottom;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(MapError("map padding leaves no visible viewport".into()));
    }

    let longitude_span = bounds.longitude_span();
    let center_lon = normalize_longitude(bounds.west + longitude_span / 2.0);
    let north_y = mercator_y(bounds.north);
    let south_y = mercator_y(bounds.south);
    let latitude_span = (south_y - north_y).abs();
    let center_y = (north_y + south_y) / 2.0;

    let horizontal_zoom = (width / (MAPLIBRE_TILE_SIZE * longitude_span / 360.0)).log2();
    let vertical_zoom = if latitude_span > f64::EPSILON {
        (height / (MAPLIBRE_TILE_SIZE * latitude_span)).log2()
    } else {
        max_zoom
    };
    let zoom = horizontal_zoom.min(vertical_zoom).clamp(0.0, max_zoom);

    Ok(ResolvedCamera {
        center: GeoPoint {
            lat: inverse_mercator_y(center_y),
            lon: center_lon,
        },
        zoom,
        bearing_degrees,
        pitch_degrees,
    })
}

fn mercator_y(latitude: f64) -> f64 {
    let latitude = latitude.clamp(-WEB_MERCATOR_MAX_LAT, WEB_MERCATOR_MAX_LAT);
    let sine = latitude.to_radians().sin();
    0.5 - ((1.0 + sine) / (1.0 - sine)).ln() / (4.0 * PI)
}

fn inverse_mercator_y(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

fn normalize_longitude(longitude: f64) -> f64 {
    let normalized = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if normalized == -180.0 && longitude > 0.0 {
        180.0
    } else {
        normalized
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

fn visible(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn valid_zoom(zoom: f64) -> bool {
    zoom.is_finite() && (0.0..=MAX_ZOOM).contains(&zoom)
}

const fn default_true() -> bool {
    true
}

const fn default_max_zoom() -> f64 {
    MAX_ZOOM
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> SurfaceMetrics {
        SurfaceMetrics {
            logical_width: 1200.0,
            logical_height: 800.0,
            scale_factor: 2.0,
        }
    }

    fn intent(target: CameraTarget) -> CameraIntent {
        CameraIntent {
            target,
            bearing_degrees: 0.0,
            pitch_degrees: 0.0,
            padding: MapPadding::default(),
            transition: CameraTransition::Fly { duration_ms: 1_200 },
        }
    }

    #[test]
    fn extent_camera_is_derived_from_actual_surface_size() {
        let camera = intent(CameraTarget::Extent {
            extent: GeoExtent {
                bounds: GeoBounds {
                    west: -74.2591,
                    south: 40.4774,
                    east: -73.7004,
                    north: 40.9176,
                },
                label: Some("New York City".into()),
                source: Some(GeoSourceRef {
                    owner: "panopticon".into(),
                    id: "nyc-five-boroughs".into(),
                    revision: "tiger-2026".into(),
                }),
            },
        });
        let CameraResolution::Resolved(resolved) = camera.resolve(metrics()).unwrap() else {
            panic!("extent should resolve without external state");
        };
        assert!((40.69..=40.71).contains(&resolved.center.lat));
        assert!((-74.0..=-73.9).contains(&resolved.center.lon));
        assert!((8.0..=11.0).contains(&resolved.zoom));

        let wider = camera
            .resolve(SurfaceMetrics {
                logical_width: 2400.0,
                ..metrics()
            })
            .unwrap();
        let CameraResolution::Resolved(wider) = wider else {
            unreachable!();
        };
        assert!(wider.zoom >= resolved.zoom);
    }

    #[test]
    fn antimeridian_extent_takes_the_short_path() {
        let camera = intent(CameraTarget::Extent {
            extent: GeoExtent {
                bounds: GeoBounds {
                    west: 170.0,
                    south: -20.0,
                    east: -170.0,
                    north: 20.0,
                },
                label: Some("Dateline".into()),
                source: None,
            },
        });
        let CameraResolution::Resolved(resolved) = camera.resolve(metrics()).unwrap() else {
            unreachable!();
        };
        assert!(resolved.center.lon.abs() >= 179.9);
        assert!(resolved.zoom > 2.0);

        assert!(
            GeoBounds {
                west: 180.0,
                south: -10.0,
                east: -180.0,
                north: 10.0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn follow_and_fit_targets_defer_to_bound_data() {
        let follow = intent(CameraTarget::FollowEntity {
            binding_id: "live-aircraft".into(),
            entity_id: "icao-a1b2c3".into(),
            zoom: Some(10.0),
        });
        assert!(matches!(
            follow.resolve(metrics()).unwrap(),
            CameraResolution::NeedsEntity { .. }
        ));
        let fit = intent(CameraTarget::FitBinding {
            binding_id: "query-results".into(),
            max_zoom: Some(12.0),
        });
        assert!(matches!(
            fit.resolve(metrics()).unwrap(),
            CameraResolution::NeedsBindingExtent { .. }
        ));
    }

    #[test]
    fn reduced_motion_turns_every_animation_into_a_jump() {
        let camera = intent(CameraTarget::CenterZoom {
            center: GeoPoint {
                lat: 47.6062,
                lon: -122.3321,
            },
            zoom: 11.0,
        });
        assert_eq!(
            camera.effective_transition(false),
            EffectiveTransition::Fly { duration_ms: 1_200 }
        );
        assert_eq!(camera.effective_transition(true), EffectiveTransition::Jump);
    }

    #[test]
    fn malformed_camera_and_layer_state_fails_loudly() {
        let surface = MapSurface {
            resource: MapResourceRef {
                owner: "panopticon".into(),
                id: "intelligence-map".into(),
                contract: "panopticon.map.v1".into(),
            },
            camera: CameraIntent {
                pitch_degrees: 90.0,
                ..intent(CameraTarget::CenterZoom {
                    center: GeoPoint { lat: 0.0, lon: 0.0 },
                    zoom: 4.0,
                })
            },
            layers: vec![],
        };
        assert!(surface.validate().is_err());

        let mut surface = MapSurface {
            camera: CameraIntent {
                pitch_degrees: 0.0,
                ..surface.camera
            },
            layers: vec![MapLayer {
                id: "aircraft".into(),
                binding_id: "live-aircraft".into(),
                style_role: "intelligence.air".into(),
                visible: true,
                min_zoom: 12.0,
                max_zoom: 4.0,
            }],
            ..surface
        };
        assert!(surface.validate().is_err());
        surface.layers[0].max_zoom = 15.0;
        surface.validate().unwrap();
    }

    #[test]
    fn map_contract_rejects_unknown_fields() {
        let json = r#"{
          "resource":{"owner":"panopticon","id":"map","contract":"panopticon.map.v1"},
          "camera":{
            "target":{"kind":"center_zoom","center":{"lat":47.6,"lon":-122.3},"zoom":9},
            "surprise":true
          },
          "layers":[]
        }"#;
        assert!(serde_json::from_str::<MapSurface>(json).is_err());
    }
}
