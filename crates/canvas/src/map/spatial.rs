//! Spatial feature bindings consumed by map backends.
//!
//! The public model is source-neutral. Wire adapters stop at this boundary so
//! a renderer never learns Panopticon credentials, transport, or service state.

use super::{MapError, MapResourceRef};
use canvas_core::ThemePalette;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const PANOPTICON_SNAPSHOT_V1: &str = "panopticon.spatial.snapshot.v1";
const MAX_FEATURES: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialFeatureSet {
    pub resource: MapResourceRef,
    pub revision: u64,
    pub generated_at_unix_ms: u64,
    pub max_age_secs: u64,
    pub features: Vec<SpatialFeature>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialFeature {
    pub identity: SpatialIdentity,
    pub geometry: SpatialGeometry,
    pub style_role: String,
    pub temporal: Option<TemporalSeries>,
    pub measures: BTreeMap<String, f64>,
    pub summary: BTreeMap<String, Value>,
    pub provenance: Vec<SpatialProvenance>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TemporalSeries {
    Scalar {
        role: String,
        interpolation: TemporalInterpolation,
        samples: Vec<TemporalScalarSample>,
    },
    Vector {
        role: String,
        interpolation: TemporalInterpolation,
        samples: Vec<TemporalVectorSample>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalInterpolation {
    Step,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalScalarSample {
    pub timestamp_unix_ms: u64,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalVectorSample {
    pub timestamp_unix_ms: u64,
    pub magnitude: f64,
    pub direction_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvaluatedVector {
    pub magnitude: f64,
    pub direction_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvaluatedScalar {
    pub value: f64,
    pub normalized: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialDisplayPoint {
    pub position: GeoPosition,
    pub style_role: String,
    pub size_px: f32,
}

const CURRENT_PARTICLES_PER_STATION: usize = 24;
const CURRENT_TRAIL_LENGTH_M: f64 = 8_000.0;
const CURRENT_VISUAL_TIME_SCALE: f64 = 120.0;
const TIDE_RING_POINTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpatialIdentity {
    pub domain: String,
    pub entity_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpatialGeometry {
    Point(GeoPosition),
    LineString(Vec<GeoPosition>),
    Polygon(Vec<Vec<GeoPosition>>),
    Multi(Vec<SpatialPrimitive>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpatialPrimitive {
    Point(GeoPosition),
    LineString(Vec<GeoPosition>),
    Polygon(Vec<Vec<GeoPosition>>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpatialProvenance {
    pub source_id: String,
    pub record_id: Option<String>,
    pub uri: Option<String>,
}

/// GPU-ready semantic colors derived from the compositor's canonical palette.
///
/// Backends receive this value alongside geometry. They do not own a second
/// theme or hard-code a dark-map palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialPalette {
    pub atmosphere: [f32; 4],
    pub earth: [f32; 4],
    pub land: [f32; 4],
    pub boundary: [f32; 4],
    pub label: [f32; 4],
    pub label_muted: [f32; 4],
    pub selection: [f32; 4],
    semantic: [[f32; 4]; 5],
}

impl SpatialPalette {
    pub fn from_theme(theme: &ThemePalette) -> Self {
        Self {
            atmosphere: rgba(theme.info, 0.22),
            earth: rgba(theme.background_strong, 1.0),
            land: rgba(theme.surface, 1.0),
            boundary: rgba(theme.border, 0.72),
            label: rgba(theme.text, 1.0),
            label_muted: rgba(theme.text_muted, 1.0),
            selection: rgba(theme.primary, 1.0),
            semantic: [
                rgba(theme.primary, 1.0),
                rgba(theme.info, 1.0),
                rgba(theme.success, 1.0),
                rgba(theme.warning, 1.0),
                rgba(theme.danger, 1.0),
            ],
        }
    }

    /// Resolve an open-ended source role without adding palette configuration.
    /// Known semantic suffixes are stable; other roles use a stable FNV-1a
    /// bucket so a domain keeps its color across native and WASM hosts.
    pub fn color_for_role(&self, role: &str) -> [f32; 4] {
        if role.ends_with(".selected") || role == "selection" {
            return self.selection;
        }
        let semantic_index = if role.ends_with(".info") {
            Some(1)
        } else if role.ends_with(".success") {
            Some(2)
        } else if role.ends_with(".warning") {
            Some(3)
        } else if role.ends_with(".danger") {
            Some(4)
        } else {
            None
        };
        self.semantic[semantic_index.unwrap_or_else(|| stable_bucket(role, self.semantic.len()))]
    }
}

pub fn decode_panopticon_snapshot(bytes: &[u8]) -> Result<SpatialFeatureSet, MapError> {
    let wire: SnapshotWire = serde_json::from_slice(bytes)
        .map_err(|error| MapError(format!("invalid Panopticon spatial snapshot: {error}")))?;
    wire.try_into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotWire {
    contract: String,
    version: u8,
    source: SourceWire,
    revision: u64,
    generated_at_unix_ms: u64,
    freshness: FreshnessWire,
    features: Vec<FeatureWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceWire {
    owner: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreshnessWire {
    max_age_secs: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureWire {
    domain: String,
    entity_id: String,
    geometry: GeometryWire,
    style_role: String,
    // The owner also supplies richer render semantics. Point consumers use
    // geometry/style_role; semantic payloads belong to their typed adapters.
    #[serde(default, rename = "semantic")]
    _semantic: Option<Value>,
    #[serde(default)]
    temporal: Option<TemporalWire>,
    #[serde(default)]
    measures: BTreeMap<String, f64>,
    #[serde(default)]
    summary: BTreeMap<String, Value>,
    #[serde(default)]
    provenance: Vec<ProvenanceWire>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TemporalWire {
    Scalar {
        role: String,
        interpolation: TemporalInterpolation,
        samples: Vec<TemporalScalarSample>,
    },
    Vector {
        role: String,
        interpolation: TemporalInterpolation,
        #[serde(rename = "direction_convention")]
        _direction_convention: DirectionConventionWire,
        samples: Vec<TemporalVectorSample>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum DirectionConventionWire {
    DegreesClockwiseFromTrueNorth,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum GeometryWire {
    Point { position: PositionWire },
    LineString { positions: Vec<PositionWire> },
    Polygon { rings: Vec<Vec<PositionWire>> },
    Multi { geometries: Vec<PrimitiveWire> },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PrimitiveWire {
    Point { position: PositionWire },
    LineString { positions: Vec<PositionWire> },
    Polygon { rings: Vec<Vec<PositionWire>> },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionWire {
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    altitude_m: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceWire {
    source_id: String,
    #[serde(default)]
    record_id: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

impl TryFrom<SnapshotWire> for SpatialFeatureSet {
    type Error = MapError;

    fn try_from(wire: SnapshotWire) -> Result<Self, Self::Error> {
        if wire.contract != PANOPTICON_SNAPSHOT_V1 || wire.version != 1 {
            return Err(MapError("unsupported Panopticon spatial contract".into()));
        }
        if !identifier(&wire.source.owner, 128) || !identifier(&wire.source.id, 128) {
            return Err(MapError("invalid Panopticon spatial source".into()));
        }
        if wire.revision == 0 || wire.freshness.max_age_secs == 0 {
            return Err(MapError(
                "spatial revision and freshness must be greater than zero".into(),
            ));
        }
        if wire.features.len() > MAX_FEATURES {
            return Err(MapError(format!(
                "spatial snapshot exceeds {MAX_FEATURES} features"
            )));
        }

        let mut identities = BTreeSet::new();
        let mut features = Vec::with_capacity(wire.features.len());
        for feature in wire.features {
            let identity = SpatialIdentity {
                domain: feature.domain,
                entity_id: feature.entity_id,
            };
            if !identifier(&identity.domain, 64)
                || !identifier(&identity.entity_id, 256)
                || !identities.insert(identity.clone())
            {
                return Err(MapError("invalid or duplicate spatial identity".into()));
            }
            if !identifier(&feature.style_role, 64) {
                return Err(MapError("invalid spatial style role".into()));
            }
            if feature.measures.values().any(|value| !value.is_finite()) {
                return Err(MapError("spatial measures must be finite".into()));
            }
            let geometry = convert_geometry(feature.geometry)?;
            let provenance = feature
                .provenance
                .into_iter()
                .map(|item| SpatialProvenance {
                    source_id: item.source_id,
                    record_id: item.record_id,
                    uri: item.uri,
                })
                .collect();
            let temporal = feature.temporal.map(TemporalSeries::try_from).transpose()?;
            features.push(SpatialFeature {
                identity,
                geometry,
                style_role: feature.style_role,
                temporal,
                measures: feature.measures,
                summary: feature.summary,
                provenance,
            });
        }

        Ok(Self {
            resource: MapResourceRef {
                owner: wire.source.owner,
                id: wire.source.id,
                contract: wire.contract,
            },
            revision: wire.revision,
            generated_at_unix_ms: wire.generated_at_unix_ms,
            max_age_secs: wire.freshness.max_age_secs,
            features,
        })
    }
}

impl TryFrom<TemporalWire> for TemporalSeries {
    type Error = MapError;

    fn try_from(wire: TemporalWire) -> Result<Self, Self::Error> {
        let series = match wire {
            TemporalWire::Scalar {
                role,
                interpolation,
                samples,
            } => Self::Scalar {
                role,
                interpolation,
                samples,
            },
            TemporalWire::Vector {
                role,
                interpolation,
                _direction_convention: _,
                samples,
            } => Self::Vector {
                role,
                interpolation,
                samples,
            },
        };
        let (role, timestamps_valid, values_valid, count) = match &series {
            Self::Scalar { role, samples, .. } => (
                role,
                samples
                    .windows(2)
                    .all(|pair| pair[0].timestamp_unix_ms < pair[1].timestamp_unix_ms),
                samples.iter().all(|sample| sample.value.is_finite()),
                samples.len(),
            ),
            Self::Vector { role, samples, .. } => (
                role,
                samples
                    .windows(2)
                    .all(|pair| pair[0].timestamp_unix_ms < pair[1].timestamp_unix_ms),
                samples.iter().all(|sample| {
                    sample.magnitude.is_finite()
                        && sample.magnitude >= 0.0
                        && sample.direction_deg.is_finite()
                        && (0.0..360.0).contains(&sample.direction_deg)
                }),
                samples.len(),
            ),
        };
        if !identifier(role, 128)
            || count == 0
            || count > 1_000
            || !timestamps_valid
            || !values_valid
        {
            return Err(MapError("invalid Panopticon temporal series".into()));
        }
        Ok(series)
    }
}

impl TemporalSeries {
    pub fn scalar_at(&self, timestamp_unix_ms: u64) -> Option<EvaluatedScalar> {
        let Self::Scalar {
            interpolation,
            samples,
            ..
        } = self
        else {
            return None;
        };
        let value = evaluate_scalar(samples, *interpolation, timestamp_unix_ms)?;
        let (minimum, maximum) = samples
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |range, sample| {
                (range.0.min(sample.value), range.1.max(sample.value))
            });
        let normalized = if maximum > minimum {
            ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        Some(EvaluatedScalar { value, normalized })
    }

    pub fn vector_at(&self, timestamp_unix_ms: u64) -> Option<EvaluatedVector> {
        let Self::Vector {
            interpolation,
            samples,
            ..
        } = self
        else {
            return None;
        };
        evaluate_vector(samples, *interpolation, timestamp_unix_ms)
    }
}

fn evaluate_scalar(
    samples: &[TemporalScalarSample],
    interpolation: TemporalInterpolation,
    timestamp_unix_ms: u64,
) -> Option<f64> {
    let first = *samples.first()?;
    if timestamp_unix_ms <= first.timestamp_unix_ms {
        return Some(first.value);
    }
    let last = *samples.last()?;
    if timestamp_unix_ms >= last.timestamp_unix_ms {
        return Some(last.value);
    }
    let upper = samples.partition_point(|sample| sample.timestamp_unix_ms <= timestamp_unix_ms);
    let before = samples[upper - 1];
    let after = samples[upper];
    if interpolation == TemporalInterpolation::Step {
        return Some(before.value);
    }
    let span = (after.timestamp_unix_ms - before.timestamp_unix_ms) as f64;
    let mix = (timestamp_unix_ms - before.timestamp_unix_ms) as f64 / span;
    Some(before.value + (after.value - before.value) * mix)
}

/// Evaluates renderer-neutral features into retained display points.
///
/// Current particles are deterministic functions of the explicit presentation
/// timestamp, so realtime Canvas and fixed-step video renders produce the same
/// scene without retaining simulation state.
pub fn display_points_at(
    features: &SpatialFeatureSet,
    timestamp_unix_ms: u64,
) -> Vec<SpatialDisplayPoint> {
    let mut points = base_display_points(features);
    points.extend(temporal_display_points_at(features, timestamp_unix_ms));
    points
}

pub fn base_display_points(features: &SpatialFeatureSet) -> Vec<SpatialDisplayPoint> {
    features
        .features
        .iter()
        .filter_map(|feature| {
            let SpatialGeometry::Point(position) = feature.geometry else {
                return None;
            };
            let count = feature.measures.get("count").copied().unwrap_or(1.0);
            Some(SpatialDisplayPoint {
                position,
                style_role: feature.style_role.clone(),
                size_px: (5.0 + count.max(0.0).sqrt() as f32).clamp(5.0, 24.0),
            })
        })
        .collect()
}

pub fn temporal_display_points_at(
    features: &SpatialFeatureSet,
    timestamp_unix_ms: u64,
) -> Vec<SpatialDisplayPoint> {
    let mut points = Vec::new();
    for feature in &features.features {
        let SpatialGeometry::Point(position) = feature.geometry else {
            continue;
        };
        let Some(series) = &feature.temporal else {
            continue;
        };
        match series {
            TemporalSeries::Vector { role, .. } => {
                let Some(vector) = series.vector_at(timestamp_unix_ms) else {
                    continue;
                };
                let elapsed_s = timestamp_unix_ms as f64 / 1_000.0;
                let advected_m = elapsed_s * vector.magnitude * CURRENT_VISUAL_TIME_SCALE;
                for index in 0..CURRENT_PARTICLES_PER_STATION {
                    let spacing_m = CURRENT_TRAIL_LENGTH_M / CURRENT_PARTICLES_PER_STATION as f64;
                    let along_m = (advected_m + index as f64 * spacing_m)
                        .rem_euclid(CURRENT_TRAIL_LENGTH_M)
                        - CURRENT_TRAIL_LENGTH_M / 2.0;
                    points.push(SpatialDisplayPoint {
                        position: offset_position(position, vector.direction_deg, along_m),
                        style_role: role.clone(),
                        size_px: 2.0 + 2.0 * ((index % 6) as f32 / 5.0),
                    });
                }
            }
            TemporalSeries::Scalar { role, .. } => {
                let Some(tide) = series.scalar_at(timestamp_unix_ms) else {
                    continue;
                };
                let radius_m = 500.0 + tide.normalized * 1_500.0;
                for index in 0..TIDE_RING_POINTS {
                    points.push(SpatialDisplayPoint {
                        position: offset_position(
                            position,
                            index as f64 * 360.0 / TIDE_RING_POINTS as f64,
                            radius_m,
                        ),
                        style_role: role.clone(),
                        size_px: 2.5,
                    });
                }
            }
        }
    }
    points
}

fn offset_position(origin: GeoPosition, direction_deg: f64, distance_m: f64) -> GeoPosition {
    const EARTH_RADIUS_M: f64 = 6_371_008.8;
    let angular = distance_m / EARTH_RADIUS_M;
    let bearing = direction_deg.to_radians();
    let latitude = origin.latitude.to_radians();
    let longitude = origin.longitude.to_radians();
    let destination_latitude =
        (latitude.sin() * angular.cos() + latitude.cos() * angular.sin() * bearing.cos()).asin();
    let destination_longitude = longitude
        + (bearing.sin() * angular.sin() * latitude.cos())
            .atan2(angular.cos() - latitude.sin() * destination_latitude.sin());
    GeoPosition {
        latitude: destination_latitude.to_degrees(),
        longitude: (destination_longitude.to_degrees() + 180.0).rem_euclid(360.0) - 180.0,
        altitude_m: origin.altitude_m,
    }
}

fn evaluate_vector(
    samples: &[TemporalVectorSample],
    interpolation: TemporalInterpolation,
    timestamp_unix_ms: u64,
) -> Option<EvaluatedVector> {
    let first = *samples.first()?;
    if timestamp_unix_ms <= first.timestamp_unix_ms {
        return Some(EvaluatedVector {
            magnitude: first.magnitude,
            direction_deg: first.direction_deg,
        });
    }
    let last = *samples.last()?;
    if timestamp_unix_ms >= last.timestamp_unix_ms {
        return Some(EvaluatedVector {
            magnitude: last.magnitude,
            direction_deg: last.direction_deg,
        });
    }
    let upper = samples.partition_point(|sample| sample.timestamp_unix_ms <= timestamp_unix_ms);
    let before = samples[upper - 1];
    let after = samples[upper];
    if interpolation == TemporalInterpolation::Step {
        return Some(EvaluatedVector {
            magnitude: before.magnitude,
            direction_deg: before.direction_deg,
        });
    }
    let span = (after.timestamp_unix_ms - before.timestamp_unix_ms) as f64;
    let mix = (timestamp_unix_ms - before.timestamp_unix_ms) as f64 / span;
    let direction_delta =
        (after.direction_deg - before.direction_deg + 540.0).rem_euclid(360.0) - 180.0;
    Some(EvaluatedVector {
        magnitude: before.magnitude + (after.magnitude - before.magnitude) * mix,
        direction_deg: (before.direction_deg + direction_delta * mix).rem_euclid(360.0),
    })
}

fn convert_geometry(wire: GeometryWire) -> Result<SpatialGeometry, MapError> {
    Ok(match wire {
        GeometryWire::Point { position } => SpatialGeometry::Point(convert_position(position)?),
        GeometryWire::LineString { positions } => {
            SpatialGeometry::LineString(convert_positions(positions)?)
        }
        GeometryWire::Polygon { rings } => SpatialGeometry::Polygon(convert_rings(rings)?),
        GeometryWire::Multi { geometries } => SpatialGeometry::Multi(
            geometries
                .into_iter()
                .map(|geometry| match geometry {
                    PrimitiveWire::Point { position } => {
                        convert_position(position).map(SpatialPrimitive::Point)
                    }
                    PrimitiveWire::LineString { positions } => {
                        convert_positions(positions).map(SpatialPrimitive::LineString)
                    }
                    PrimitiveWire::Polygon { rings } => {
                        convert_rings(rings).map(SpatialPrimitive::Polygon)
                    }
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

fn convert_positions(wire: Vec<PositionWire>) -> Result<Vec<GeoPosition>, MapError> {
    wire.into_iter().map(convert_position).collect()
}

fn convert_rings(wire: Vec<Vec<PositionWire>>) -> Result<Vec<Vec<GeoPosition>>, MapError> {
    wire.into_iter().map(convert_positions).collect()
}

fn convert_position(wire: PositionWire) -> Result<GeoPosition, MapError> {
    if !wire.latitude.is_finite()
        || !(-90.0..=90.0).contains(&wire.latitude)
        || !wire.longitude.is_finite()
        || !(-180.0..=180.0).contains(&wire.longitude)
        || wire
            .altitude_m
            .is_some_and(|altitude| !altitude.is_finite())
    {
        return Err(MapError("invalid spatial coordinate".into()));
    }
    Ok(GeoPosition {
        latitude: wire.latitude,
        longitude: wire.longitude,
        altitude_m: wire.altitude_m,
    })
}

fn identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

fn rgba(rgb: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
        alpha,
    ]
}

fn stable_bucket(value: &str, buckets: usize) -> usize {
    let hash = value.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    hash as usize % buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_panopticon_contract_fixture() {
        let fixture = include_bytes!("../../tests/fixtures/spatial_snapshot_v1.json");
        let set = decode_panopticon_snapshot(fixture).expect("valid shared fixture");

        assert_eq!(set.resource.owner, "panopticon");
        assert_eq!(set.resource.id, "live-aviation");
        assert_eq!(set.resource.contract, PANOPTICON_SNAPSHOT_V1);
        assert_eq!(set.revision, 7);
        assert_eq!(set.max_age_secs, 30);
        assert_eq!(set.features.len(), 1);
        let feature = &set.features[0];
        assert_eq!(feature.identity.domain, "adsb");
        assert_eq!(feature.identity.entity_id, "icao-a1b2c3");
        assert_eq!(feature.style_role, "domain.adsb");
        assert_eq!(feature.measures["speed_knots"], 420.0);
        assert_eq!(feature.summary["callsign"], "N12345");
        assert_eq!(feature.provenance[0].source_id, "adsb-exchange");
        assert!(matches!(feature.geometry, SpatialGeometry::Point(_)));
    }

    #[test]
    fn rejects_renderer_fields_at_the_source_boundary() {
        let mut value: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/spatial_snapshot_v1.json"
        ))
        .unwrap();
        value["renderer"] = Value::String("wgpu".into());
        let error = decode_panopticon_snapshot(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(error.0.contains("unknown field"));
    }

    #[test]
    fn derives_spatial_roles_from_the_compositor_palette() {
        let theme: ThemePalette = serde_json::from_str(
            r##"{
                "background":"#101014","background_strong":"#08090c",
                "surface":"#20242c","surface_muted":"#171a20",
                "text":"#f2f4f8","text_muted":"#9299a6","primary":"#66ccff",
                "success":"#57d38c","warning":"#f0bd5a","danger":"#f06a73",
                "info":"#7aa7ff","border":"#343a46","border_strong":"#525b6b"
            }"##,
        )
        .unwrap();
        let palette = SpatialPalette::from_theme(&theme);

        assert_eq!(palette.earth, rgba([0x08, 0x09, 0x0c], 1.0));
        assert_eq!(palette.selection, rgba([0x66, 0xcc, 0xff], 1.0));
        assert_eq!(
            palette.color_for_role("alert.danger"),
            rgba([0xf0, 0x6a, 0x73], 1.0)
        );
        assert_eq!(
            palette.color_for_role("domain.adsb"),
            palette.color_for_role("domain.adsb")
        );
    }

    #[test]
    fn vector_interpolation_takes_the_short_path_across_north() {
        let series = TemporalSeries::Vector {
            role: "environment.water_current".into(),
            interpolation: TemporalInterpolation::Linear,
            samples: vec![
                TemporalVectorSample {
                    timestamp_unix_ms: 1_000,
                    magnitude: 1.0,
                    direction_deg: 350.0,
                },
                TemporalVectorSample {
                    timestamp_unix_ms: 2_000,
                    magnitude: 3.0,
                    direction_deg: 10.0,
                },
            ],
        };
        let evaluated = series.vector_at(1_500).unwrap();
        assert_eq!(evaluated.magnitude, 2.0);
        assert!(evaluated.direction_deg < 0.001 || evaluated.direction_deg > 359.999);
    }

    #[test]
    fn scalar_interpolation_reports_daily_range_position() {
        let series = TemporalSeries::Scalar {
            role: "environment.tide_height".into(),
            interpolation: TemporalInterpolation::Linear,
            samples: vec![
                TemporalScalarSample {
                    timestamp_unix_ms: 1_000,
                    value: -1.0,
                },
                TemporalScalarSample {
                    timestamp_unix_ms: 2_000,
                    value: 3.0,
                },
            ],
        };
        let evaluated = series.scalar_at(1_500).unwrap();
        assert_eq!(evaluated.value, 1.0);
        assert_eq!(evaluated.normalized, 0.5);
    }

    #[test]
    fn current_particles_are_deterministic_and_time_driven() {
        let set = SpatialFeatureSet {
            resource: MapResourceRef {
                owner: "panopticon".into(),
                id: "marine".into(),
                contract: PANOPTICON_SNAPSHOT_V1.into(),
            },
            revision: 1,
            generated_at_unix_ms: 1_000,
            max_age_secs: 60,
            features: vec![SpatialFeature {
                identity: SpatialIdentity {
                    domain: "noaa_coops".into(),
                    entity_id: "current:test".into(),
                },
                geometry: SpatialGeometry::Point(GeoPosition {
                    latitude: 48.0,
                    longitude: -123.0,
                    altitude_m: None,
                }),
                style_role: "domain.noaa_coops".into(),
                temporal: Some(TemporalSeries::Vector {
                    role: "environment.water_current".into(),
                    interpolation: TemporalInterpolation::Linear,
                    samples: vec![TemporalVectorSample {
                        timestamp_unix_ms: 1_000,
                        magnitude: 1.0,
                        direction_deg: 90.0,
                    }],
                }),
                measures: BTreeMap::new(),
                summary: BTreeMap::new(),
                provenance: Vec::new(),
            }],
        };
        let first = display_points_at(&set, 1_000);
        let repeated = display_points_at(&set, 1_000);
        let later = display_points_at(&set, 2_000);
        assert_eq!(first, repeated);
        assert_eq!(first.len(), 1 + CURRENT_PARTICLES_PER_STATION);
        assert_ne!(first[1].position, later[1].position);
    }
}
