//! Canvas adapter for the retained globe renderer owned by `fpl-gfx`.
//!
//! Canvas owns spatial semantics and camera planning. `fpl-gfx` owns WGPU
//! pipelines, geometry, and retained GPU resources. This crate only translates
//! between those two stable interfaces.

use canvas::map::{
    globe::GlobeFramePlan,
    spatial::{
        GeoPosition, SpatialDisplayPoint, SpatialFeature, SpatialFeatureSet, SpatialGeometry,
        SpatialPalette, SpatialPrimitive, base_display_points, temporal_display_points_at,
    },
};
use fpl_gfx::{
    GlobeFrame, GlobeLineSegment, GlobeMarkerKind, GlobeMarkerStyle, GlobePoint, GlobeRenderError,
    GlobeRendererConfig, GlobeStyle,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    pub earth_triangles: u32,
    pub point_instances: u32,
}

#[derive(Debug)]
pub enum RenderError {
    Gpu(GlobeRenderError),
    RouteBudget,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gpu(error) => error.fmt(formatter),
            Self::RouteBudget => {
                formatter.write_str("reference routes exceed the 200000 segment budget")
            }
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gpu(error) => Some(error),
            Self::RouteBudget => None,
        }
    }
}

impl From<GlobeRenderError> for RenderError {
    fn from(error: GlobeRenderError) -> Self {
        Self::Gpu(error)
    }
}

pub struct GlobeRenderer {
    renderer: fpl_gfx::GlobeRenderer,
    base_points: Vec<SpatialDisplayPoint>,
    base_markers: Vec<GlobeMarkerStyle>,
    base_features: Option<SpatialFeatureSet>,
    cluster_zoom: Option<u8>,
    reference_features: Option<SpatialFeatureSet>,
    reference_palette: Option<SpatialPalette>,
    temporal_features: Option<SpatialFeatureSet>,
    resolved_points: Vec<GlobePoint>,
    dynamic_start: usize,
    last_palette: Option<SpatialPalette>,
    last_timestamp_unix_ms: Option<u64>,
}

impl GlobeRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let renderer = fpl_gfx::GlobeRenderer::new(device, GlobeRendererConfig::new(target_format))
            .expect("standard WGS84 globe tessellation is valid");
        Self {
            renderer,
            base_points: Vec::new(),
            base_markers: Vec::new(),
            base_features: None,
            cluster_zoom: None,
            reference_features: None,
            reference_palette: None,
            temporal_features: None,
            resolved_points: Vec::new(),
            dynamic_start: 0,
            last_palette: None,
            last_timestamp_unix_ms: None,
        }
    }

    pub fn set_raster_rgba8(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), RenderError> {
        self.renderer
            .set_raster_rgba8(device, queue, width, height, rgba)
            .map_err(Into::into)
    }

    pub fn set_features(
        &mut self,
        _device: &wgpu::Device,
        features: &SpatialFeatureSet,
    ) -> Result<(), RenderError> {
        self.base_features = Some(features.clone());
        (self.base_points, self.base_markers) =
            clustered_points(features, self.cluster_zoom.unwrap_or(18));
        let mut temporal_features = features.clone();
        temporal_features
            .features
            .retain(|feature| feature.temporal.is_some());
        self.temporal_features = Some(temporal_features);
        self.last_palette = None;
        self.last_timestamp_unix_ms = None;
        Ok(())
    }

    /// Chooses presentation LOD using Galileo's shared hysteretic XYZ selection.
    pub fn set_view_resolution(&mut self, latitude: f64, meters_per_pixel: f64) {
        let zoom = galileo::tile_lod::zoom_for_resolution_hysteretic(
            latitude,
            meters_per_pixel / 8.0,
            galileo::tile_lod::TilePolicy {
                previous_zoom: self.cluster_zoom,
                ..Default::default()
            },
        );
        if self.cluster_zoom != Some(zoom) {
            self.cluster_zoom = Some(zoom);
            if let Some(features) = &self.base_features {
                (self.base_points, self.base_markers) = clustered_points(features, zoom);
                self.last_palette = None;
            }
        }
    }

    /// Binds owner-produced reference routes independently of live observations.
    pub fn set_reference_features(&mut self, features: &SpatialFeatureSet) {
        self.reference_features = Some(features.clone());
        self.reference_palette = None;
    }

    pub fn clear_reference_features(&mut self) {
        self.reference_features = None;
        self.reference_palette = None;
    }

    pub fn set_mercator_detail_rgba8(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        bounds: [f64; 4],
        rgba: &[u8],
    ) -> Result<(), RenderError> {
        self.renderer
            .set_mercator_detail_rgba8(device, queue, width, height, bounds, rgba)
            .map_err(Into::into)
    }

    pub fn render_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        plan: &GlobeFramePlan,
        timestamp_unix_ms: u64,
    ) -> Result<RenderStats, RenderError> {
        if self.reference_palette.as_ref() != Some(&plan.palette) {
            let lines = self
                .reference_features
                .as_ref()
                .map(|features| resolve_lines(features, plan.palette))
                .transpose()?
                .unwrap_or_default();
            self.renderer.set_lines(device, &lines)?;
            self.reference_palette = Some(plan.palette);
        }
        if self.last_palette.as_ref() != Some(&plan.palette) {
            self.resolved_points = resolve_points(&self.base_points, plan.palette);
            for (point, marker) in self.resolved_points.iter_mut().zip(&self.base_markers) {
                if matches!(
                    marker.kind,
                    GlobeMarkerKind::Vessel | GlobeMarkerKind::Aircraft
                ) && marker.tangent_ecef == [0.0; 3]
                {
                    point.color[3] *= 0.55;
                }
            }
            self.dynamic_start = self.resolved_points.len();
            let dynamic = self
                .temporal_features
                .as_ref()
                .map_or_else(Vec::new, |features| {
                    temporal_display_points_at(features, timestamp_unix_ms)
                });
            self.resolved_points
                .extend(resolve_points(&dynamic, plan.palette));
            self.renderer.set_points(device, &self.resolved_points)?;
            let mut markers = self.base_markers.clone();
            markers.resize(self.resolved_points.len(), GlobeMarkerStyle::default());
            self.renderer.set_marker_styles(&markers)?;
            self.last_palette = Some(plan.palette.clone());
            self.last_timestamp_unix_ms = Some(timestamp_unix_ms);
        } else if self.last_timestamp_unix_ms != Some(timestamp_unix_ms) {
            let dynamic = self
                .temporal_features
                .as_ref()
                .map_or_else(Vec::new, |features| {
                    temporal_display_points_at(features, timestamp_unix_ms)
                });
            let dynamic = resolve_points(&dynamic, plan.palette);
            self.renderer
                .update_points(queue, self.dynamic_start, &dynamic)?;
            self.resolved_points[self.dynamic_start..].copy_from_slice(&dynamic);
            self.last_timestamp_unix_ms = Some(timestamp_unix_ms);
        }

        let stats = self.renderer.render_into(
            device,
            queue,
            encoder,
            target,
            &GlobeFrame {
                world_origin_ecef_m: plan.world_origin_ecef_m,
                view_projection: plan.view_projection,
                viewport_px: plan.viewport_px,
                style: globe_style(&plan.palette),
            },
        )?;
        Ok(RenderStats {
            earth_triangles: stats.earth_triangles,
            point_instances: stats.point_instances,
        })
    }
}

fn clustered_points(
    features: &SpatialFeatureSet,
    zoom: u8,
) -> (Vec<SpatialDisplayPoint>, Vec<GlobeMarkerStyle>) {
    let source_points = base_display_points(features);
    let source_features: Vec<_> = features
        .features
        .iter()
        .filter(|feature| matches!(feature.geometry, SpatialGeometry::Point(_)))
        .collect();
    let mut groups = std::collections::BTreeMap::new();
    let mut points = Vec::new();
    let mut markers = Vec::new();
    for (index, (point, feature)) in source_points.iter().zip(&source_features).enumerate() {
        if matches!(feature.identity.domain.as_str(), "ais" | "adsb") {
            let tile =
                galileo::tile_lod::tile_at(point.position.latitude, point.position.longitude, zoom);
            groups
                .entry((
                    feature.identity.domain.clone(),
                    feature.style_role.clone(),
                    tile,
                ))
                .or_insert_with(Vec::new)
                .push(index);
        } else {
            points.push(point.clone());
            markers.push(GlobeMarkerStyle::default());
        }
    }
    for indices in groups.into_values() {
        let first = indices[0];
        let mut point = source_points[first].clone();
        let mut marker = marker_style(source_features[first]);
        let mut count = 0u32;
        let (mut latitude, mut longitude_sin, mut longitude_cos, mut altitude) =
            (0.0, 0.0, 0.0, 0.0);
        for index in &indices {
            let weight = source_features[*index]
                .measures
                .get("count")
                .copied()
                .unwrap_or(1.0)
                .round()
                .max(1.0) as u32;
            count = count.saturating_add(weight);
            let position = source_points[*index].position;
            let weight = f64::from(weight);
            latitude += position.latitude * weight;
            longitude_sin += position.longitude.to_radians().sin() * weight;
            longitude_cos += position.longitude.to_radians().cos() * weight;
            altitude += position.altitude_m.unwrap_or(0.0) * weight;
        }
        if count > 1 {
            point.position = GeoPosition {
                latitude: latitude / f64::from(count),
                longitude: longitude_sin.atan2(longitude_cos).to_degrees(),
                altitude_m: Some(altitude / f64::from(count)),
            };
            point.size_px = 15.0;
            marker = GlobeMarkerStyle {
                kind: GlobeMarkerKind::Cluster(count),
                ..Default::default()
            };
        } else {
            point.size_px = 12.0;
        }
        points.push(point);
        markers.push(marker);
    }
    (points, markers)
}

fn resolve_lines(
    features: &SpatialFeatureSet,
    palette: SpatialPalette,
) -> Result<Vec<GlobeLineSegment>, RenderError> {
    let mut lines = Vec::new();
    for feature in &features.features {
        let mut color = linear_rgba(palette.color_for_role(&feature.style_role));
        color[3] *= 0.65;
        match &feature.geometry {
            SpatialGeometry::LineString(route) => append_route(route, color, &mut lines)?,
            SpatialGeometry::Multi(primitives) => {
                for primitive in primitives {
                    if let SpatialPrimitive::LineString(route) = primitive {
                        append_route(route, color, &mut lines)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(lines)
}

fn append_route(
    route: &[GeoPosition],
    color: [f32; 4],
    lines: &mut Vec<GlobeLineSegment>,
) -> Result<(), RenderError> {
    for pair in route.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let longitude_delta = (b.longitude - a.longitude + 180.0).rem_euclid(360.0) - 180.0;
        let latitude_delta = b.latitude - a.latitude;
        // At most 0.125 degrees per axis keeps chord sag below the 10 m
        // cartographic drape offset without inflating the global segment count.
        let steps = (longitude_delta.abs().max(latitude_delta.abs()) / 0.125)
            .ceil()
            .max(1.0) as usize;
        if steps > 200_000usize.saturating_sub(lines.len()) {
            return Err(RenderError::RouteBudget);
        }
        let point = |fraction: f64| {
            let altitude = a.altitude_m.unwrap_or(0.0)
                + (b.altitude_m.unwrap_or(0.0) - a.altitude_m.unwrap_or(0.0)) * fraction;
            // Display-only drape offset keeps a cartographic stroke above mesh
            // chords. Source geometry and unknown seabed depth stay intact.
            fpl_gfx::geodetic_to_ecef(
                a.latitude + latitude_delta * fraction,
                a.longitude + longitude_delta * fraction,
                altitude + 10.0,
            )
        };
        let mut start = point(0.0);
        for step in 1..=steps {
            let end = point(step as f64 / steps as f64);
            lines.push(GlobeLineSegment {
                start_ecef_m: start,
                end_ecef_m: end,
                color,
                width_px: 2.2,
            });
            start = end;
        }
    }
    Ok(())
}

fn marker_style(feature: &SpatialFeature) -> GlobeMarkerStyle {
    let kind = match feature.identity.domain.as_str() {
        "ais" => GlobeMarkerKind::Vessel,
        "adsb" => GlobeMarkerKind::Aircraft,
        _ => GlobeMarkerKind::Observation,
    };
    let mut style = GlobeMarkerStyle {
        kind,
        ..Default::default()
    };
    if feature
        .measures
        .get("count")
        .is_some_and(|count| *count > 1.0)
    {
        return GlobeMarkerStyle {
            kind: GlobeMarkerKind::Cluster(feature.measures["count"].round() as u32),
            ..Default::default()
        };
    }
    if let (SpatialGeometry::Point(position), Some(heading)) = (
        &feature.geometry,
        feature
            .measures
            .get("heading_deg")
            .filter(|value| (0.0..360.0).contains(*value)),
    ) {
        let (latitude_sin, latitude_cos) = position.latitude.to_radians().sin_cos();
        let (longitude_sin, longitude_cos) = position.longitude.to_radians().sin_cos();
        let (heading_sin, heading_cos) = heading.to_radians().sin_cos();
        let north = [
            -latitude_sin * longitude_cos,
            -latitude_sin * longitude_sin,
            latitude_cos,
        ];
        let east = [-longitude_sin, longitude_cos, 0.0];
        style.tangent_ecef =
            std::array::from_fn(|axis| north[axis] * heading_cos + east[axis] * heading_sin);
    }
    style
}

fn resolve_points(points: &[SpatialDisplayPoint], palette: SpatialPalette) -> Vec<GlobePoint> {
    points
        .iter()
        .map(|point| GlobePoint {
            ecef_m: canvas::map::globe::geodetic_to_ecef(
                point.position.latitude,
                point.position.longitude,
                point.position.altitude_m.unwrap_or(0.0),
            ),
            color: linear_rgba(palette.color_for_role(&point.style_role)),
            size_px: point.size_px,
        })
        .collect()
}

fn globe_style(palette: &SpatialPalette) -> GlobeStyle {
    GlobeStyle {
        earth: linear_rgba(palette.earth),
        land: linear_rgba(palette.land),
        boundary: linear_rgba(palette.boundary),
        atmosphere: linear_rgba(palette.atmosphere),
    }
}

// SpatialPalette carries authored sRGB values. WGPU's sRGB target expects
// linear shader colors; the shared GFX color type owns that conversion.
fn linear_rgba(rgba: [f32; 4]) -> [f32; 4] {
    let [r, g, b, _] = rgba.map(|channel| (channel * 255.0).round() as u8);
    let mut color = fpl_gfx::Color::from_srgb8(r, g, b, 255);
    color.a = rgba[3];
    color.to_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::ThemePalette;

    #[test]
    fn traffic_lod_merges_by_class_and_splits_at_close_zoom() {
        use canvas::map::{MapResourceRef, spatial::SpatialIdentity};
        let feature = |domain: &str, id: &str, longitude| SpatialFeature {
            identity: SpatialIdentity {
                domain: domain.into(),
                entity_id: id.into(),
            },
            geometry: SpatialGeometry::Point(GeoPosition {
                latitude: 0.0,
                longitude,
                altitude_m: None,
            }),
            style_role: format!("domain.{domain}"),
            temporal: None,
            measures: Default::default(),
            summary: Default::default(),
            provenance: Vec::new(),
        };
        let source = SpatialFeatureSet {
            resource: MapResourceRef {
                owner: "panopticon".into(),
                id: "test".into(),
                contract: "panopticon.spatial.snapshot.v1".into(),
            },
            revision: 1,
            generated_at_unix_ms: 1,
            max_age_secs: 60,
            features: vec![
                feature("ais", "a", 0.0),
                feature("ais", "b", 0.01),
                feature("adsb", "c", 0.0),
            ],
        };
        let (points, markers) = clustered_points(&source, 0);
        assert_eq!(points.len(), 2);
        assert!(
            markers
                .iter()
                .any(|marker| marker.kind == GlobeMarkerKind::Cluster(2))
        );
        assert!(
            markers
                .iter()
                .any(|marker| marker.kind == GlobeMarkerKind::Aircraft)
        );
        let (points, markers) = clustered_points(&source, 18);
        assert_eq!(points.len(), 3);
        assert_eq!(
            markers
                .iter()
                .filter(|marker| marker.kind == GlobeMarkerKind::Vessel)
                .count(),
            2
        );
    }

    #[test]
    fn routes_follow_the_surface_across_the_short_dateline_arc() {
        let route = [179.0, -179.0].map(|longitude| GeoPosition {
            latitude: 0.0,
            longitude,
            altitude_m: Some(0.0),
        });
        let mut lines = Vec::new();
        append_route(&route, [0.2, 0.4, 0.6, 0.65], &mut lines).unwrap();
        assert_eq!(lines.len(), 16);
        for line in &lines {
            for point in [line.start_ecef_m, line.end_ecef_m] {
                assert!(point[0] < 0.0, "route crossed the prime meridian");
                let height = point[0].hypot(point[1]).hypot(point[2]) - fpl_gfx::WGS84_SEMI_MAJOR_M;
                assert!(
                    (height - 10.0).abs() < 0.001,
                    "unexpected display altitude {height}"
                );
            }
        }
    }

    #[test]
    fn vessel_headings_are_world_tangents_and_clusters_are_not_individual_ships() {
        use canvas::map::spatial::{GeoPosition, SpatialIdentity};
        let mut feature = SpatialFeature {
            identity: SpatialIdentity {
                domain: "ais".into(),
                entity_id: "vessel".into(),
            },
            geometry: SpatialGeometry::Point(GeoPosition {
                latitude: 0.0,
                longitude: 0.0,
                altitude_m: None,
            }),
            style_role: "domain.ais".into(),
            temporal: None,
            measures: std::collections::BTreeMap::from([("heading_deg".into(), 90.0)]),
            summary: Default::default(),
            provenance: Vec::new(),
        };
        let style = marker_style(&feature);
        assert_eq!(style.kind, GlobeMarkerKind::Vessel);
        assert!(style.tangent_ecef[0].abs() < 1e-10);
        assert!((style.tangent_ecef[1] - 1.0).abs() < 1e-10);
        assert!(style.tangent_ecef[2].abs() < 1e-10);
        feature.measures.insert("count".into(), 20.0);
        assert_eq!(marker_style(&feature).kind, GlobeMarkerKind::Cluster(20));
        feature.measures.clear();
        feature.identity.domain = "adsb".into();
        assert_eq!(marker_style(&feature).kind, GlobeMarkerKind::Aircraft);
        assert_eq!(marker_style(&feature).tangent_ecef, [0.0; 3]);
    }

    #[test]
    fn palette_translation_uses_owner_linear_colors() {
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
        let style = globe_style(&palette);
        assert_eq!(style.earth, linear_rgba(palette.earth));
        assert_eq!(style.land, linear_rgba(palette.land));
        assert_eq!(style.boundary, linear_rgba(palette.boundary));
        assert_eq!(style.atmosphere, linear_rgba(palette.atmosphere));
        let gray = linear_rgba([128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 0.22]);
        assert!((gray[0] - 0.21586).abs() < 0.0001);
        assert_eq!(gray[3], 0.22);
    }
}
