//! Canvas adapter for the retained globe renderer owned by `fpl-gfx`.
//!
//! Canvas owns spatial semantics and camera planning. `fpl-gfx` owns WGPU
//! pipelines, geometry, and retained GPU resources. This crate only translates
//! between those two stable interfaces.

use canvas::map::{
    globe::GlobeFramePlan,
    spatial::{
        SpatialDisplayPoint, SpatialFeatureSet, SpatialPalette, base_display_points,
        temporal_display_points_at,
    },
};
use fpl_gfx::{GlobeFrame, GlobePoint, GlobeRenderError, GlobeRendererConfig, GlobeStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    pub earth_triangles: u32,
    pub point_instances: u32,
}

#[derive(Debug)]
pub struct RenderError(GlobeRenderError);

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<GlobeRenderError> for RenderError {
    fn from(error: GlobeRenderError) -> Self {
        Self(error)
    }
}

pub struct GlobeRenderer {
    renderer: fpl_gfx::GlobeRenderer,
    base_points: Vec<SpatialDisplayPoint>,
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
        self.base_points = base_display_points(features);
        let mut temporal_features = features.clone();
        temporal_features
            .features
            .retain(|feature| feature.temporal.is_some());
        self.temporal_features = Some(temporal_features);
        self.last_palette = None;
        self.last_timestamp_unix_ms = None;
        Ok(())
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
        if self.last_palette.as_ref() != Some(&plan.palette) {
            self.resolved_points = resolve_points(&self.base_points, plan.palette);
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

fn resolve_points(points: &[SpatialDisplayPoint], palette: SpatialPalette) -> Vec<GlobePoint> {
    points
        .iter()
        .map(|point| GlobePoint {
            ecef_m: canvas::map::globe::geodetic_to_ecef(
                point.position.latitude,
                point.position.longitude,
                point.position.altitude_m.unwrap_or(0.0),
            ),
            color: palette.color_for_role(&point.style_role),
            size_px: point.size_px,
        })
        .collect()
}

fn globe_style(palette: &SpatialPalette) -> GlobeStyle {
    GlobeStyle {
        earth: palette.earth,
        land: palette.land,
        boundary: palette.boundary,
        atmosphere: palette.atmosphere,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::ThemePalette;

    #[test]
    fn palette_translation_is_lossless() {
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
        assert_eq!(style.earth, palette.earth);
        assert_eq!(style.land, palette.land);
        assert_eq!(style.boundary, palette.boundary);
        assert_eq!(style.atmosphere, palette.atmosphere);
    }
}
