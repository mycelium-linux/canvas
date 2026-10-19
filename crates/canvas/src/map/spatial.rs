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
    pub measures: BTreeMap<String, f64>,
    pub summary: BTreeMap<String, Value>,
    pub provenance: Vec<SpatialProvenance>,
}

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
    measures: BTreeMap<String, f64>,
    #[serde(default)]
    summary: BTreeMap<String, Value>,
    #[serde(default)]
    provenance: Vec<ProvenanceWire>,
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
            features.push(SpatialFeature {
                identity,
                geometry,
                style_role: feature.style_role,
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
}
