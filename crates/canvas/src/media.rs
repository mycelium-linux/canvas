//! Renderer-neutral sizing and cache policy for pixel-backed Canvas surfaces.
//!
//! Planning is intentionally separate from decoding and GPU upload. Callers can
//! perform those expensive operations away from the render thread and hand the
//! resulting texture to any Canvas renderer.

use serde::{Deserialize, Serialize};
use std::{error, fmt};

/// A size expressed in Canvas logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

/// An integral size expressed in physical display pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

/// Hard limits applied before a decode or PDF rasterization is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaLimits {
    pub max_width_px: u32,
    pub max_height_px: u32,
    pub max_allocation_bytes: u64,
    pub bytes_per_pixel: u8,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_width_px: 8_192,
            max_height_px: 8_192,
            max_allocation_bytes: 256 * 1024 * 1024,
            bytes_per_pixel: 4,
        }
    }
}

/// A deterministic key for decoded or rasterized media.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCacheKey {
    pub source_id: String,
    pub variant: MediaVariant,
    pub pixel_size: PixelSize,
}

impl MediaCacheKey {
    /// A collision-safe stable representation suitable for disk-cache paths.
    pub fn stable_string(&self) -> String {
        let variant = match self.variant {
            MediaVariant::Raster => "raster".to_owned(),
            MediaVariant::PdfPage { page_index } => format!("pdf-page-{page_index}"),
        };
        format!(
            "canvas-media-v1-{}-{}-{}-{}x{}",
            self.source_id.len(),
            self.source_id,
            variant,
            self.pixel_size.width,
            self.pixel_size.height
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaVariant {
    Raster,
    PdfPage { page_index: u32 },
}

/// The source's intrinsic dimensions in its native coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceDimensions {
    RasterPixels(PixelSize),
    PdfPageUnits(LogicalSize),
}

/// Work that a decoder and renderer can execute without re-deriving policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaPlan {
    /// The layout space that requested this plan.
    pub viewport_logical: LogicalSize,
    /// Intrinsic source dimensions, retained for diagnostics and replanning.
    pub source_dimensions: SourceDimensions,
    /// Decode or rasterize to this physical-pixel size.
    pub target_px: PixelSize,
    /// Draw at this logical size to preserve a one-texel-to-one-physical-pixel
    /// upper bound after hard limits are applied.
    pub display_logical: LogicalSize,
    /// The scale factor used to make this plan.
    pub scale_factor: f64,
    pub cache_key: MediaCacheKey,
}

/// Controls when an existing plan is replaced after a resize or scale change.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeInvalidation {
    /// Ignore dimension jitter below this physical-pixel distance.
    pub min_pixel_delta: u32,
    /// Still invalidate small absolute changes when they are a meaningful
    /// fraction of the previous plan.
    pub min_relative_delta: f64,
}

impl Default for ResizeInvalidation {
    fn default() -> Self {
        Self {
            min_pixel_delta: 32,
            min_relative_delta: 0.05,
        }
    }
}

impl ResizeInvalidation {
    pub fn should_replace(self, cached: PixelSize, requested: PixelSize) -> bool {
        if cached == requested {
            return false;
        }
        if !self.min_relative_delta.is_finite() || self.min_relative_delta < 0.0 {
            return true;
        }

        dimension_changed(
            cached.width,
            requested.width,
            self.min_pixel_delta,
            self.min_relative_delta,
        ) || dimension_changed(
            cached.height,
            requested.height,
            self.min_pixel_delta,
            self.min_relative_delta,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPlanError(pub &'static str);

impl fmt::Display for MediaPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl error::Error for MediaPlanError {}

/// Plan a raster decode. The returned display size never asks the renderer to
/// enlarge a source texel beyond one physical display pixel.
pub fn plan_raster(
    source_id: impl Into<String>,
    source_px: PixelSize,
    viewport: LogicalSize,
    scale_factor: f64,
    limits: MediaLimits,
) -> Result<MediaPlan, MediaPlanError> {
    validate(source_px, viewport, scale_factor, limits)?;

    let viewport_px = physical_viewport(viewport, scale_factor)?;
    let target = contain(source_px, viewport_px, false);
    let target = constrain(target, limits);
    Ok(plan(
        source_id.into(),
        MediaVariant::Raster,
        SourceDimensions::RasterPixels(source_px),
        viewport,
        target,
        scale_factor,
    ))
}

/// Plan a PDF page rasterization at the display's physical-pixel density.
/// `page_units` may be points or any other positive unit; only its aspect ratio
/// affects the target.
pub fn plan_pdf_page(
    source_id: impl Into<String>,
    page_index: u32,
    page_units: LogicalSize,
    viewport: LogicalSize,
    scale_factor: f64,
    limits: MediaLimits,
) -> Result<MediaPlan, MediaPlanError> {
    validate_logical(
        page_units,
        "PDF page dimensions must be finite and positive",
    )?;
    validate(
        PixelSize {
            width: 1,
            height: 1,
        },
        viewport,
        scale_factor,
        limits,
    )?;

    let viewport_px = physical_viewport(viewport, scale_factor)?;
    let (width, height) = ratio_basis(page_units.width, page_units.height);
    let aspect_source = PixelSize { width, height };
    let target = constrain(contain(aspect_source, viewport_px, true), limits);
    Ok(plan(
        source_id.into(),
        MediaVariant::PdfPage { page_index },
        SourceDimensions::PdfPageUnits(page_units),
        viewport,
        target,
        scale_factor,
    ))
}

fn plan(
    source_id: String,
    variant: MediaVariant,
    source_dimensions: SourceDimensions,
    viewport_logical: LogicalSize,
    target_px: PixelSize,
    scale_factor: f64,
) -> MediaPlan {
    MediaPlan {
        viewport_logical,
        source_dimensions,
        target_px,
        display_logical: LogicalSize {
            width: f64::from(target_px.width) / scale_factor,
            height: f64::from(target_px.height) / scale_factor,
        },
        scale_factor,
        cache_key: MediaCacheKey {
            source_id,
            variant,
            pixel_size: target_px,
        },
    }
}

fn validate(
    source: PixelSize,
    viewport: LogicalSize,
    scale_factor: f64,
    limits: MediaLimits,
) -> Result<(), MediaPlanError> {
    if source.width == 0 || source.height == 0 {
        return Err(MediaPlanError("source dimensions must be positive"));
    }
    validate_logical(viewport, "viewport dimensions must be finite and positive")?;
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err(MediaPlanError("scale factor must be finite and positive"));
    }
    if limits.max_width_px == 0
        || limits.max_height_px == 0
        || limits.max_allocation_bytes == 0
        || limits.bytes_per_pixel == 0
        || limits.max_allocation_bytes < u64::from(limits.bytes_per_pixel)
    {
        return Err(MediaPlanError("media limits must be positive"));
    }
    Ok(())
}

fn validate_logical(size: LogicalSize, message: &'static str) -> Result<(), MediaPlanError> {
    if size.width.is_finite() && size.width > 0.0 && size.height.is_finite() && size.height > 0.0 {
        Ok(())
    } else {
        Err(MediaPlanError(message))
    }
}

fn physical_viewport(
    viewport: LogicalSize,
    scale_factor: f64,
) -> Result<PixelSize, MediaPlanError> {
    let width = (viewport.width * scale_factor).ceil();
    let height = (viewport.height * scale_factor).ceil();
    if width > f64::from(u32::MAX) || height > f64::from(u32::MAX) {
        return Err(MediaPlanError(
            "physical viewport exceeds supported dimensions",
        ));
    }
    Ok(PixelSize {
        width: width as u32,
        height: height as u32,
    })
}

fn contain(source: PixelSize, bounds: PixelSize, allow_upscale: bool) -> PixelSize {
    let width_scale = f64::from(bounds.width) / f64::from(source.width);
    let height_scale = f64::from(bounds.height) / f64::from(source.height);
    let scale = width_scale
        .min(height_scale)
        .min(if allow_upscale { f64::MAX } else { 1.0 });
    PixelSize {
        width: (f64::from(source.width) * scale).floor().max(1.0) as u32,
        height: (f64::from(source.height) * scale).floor().max(1.0) as u32,
    }
}

fn constrain(size: PixelSize, limits: MediaLimits) -> PixelSize {
    let dimension_scale = (f64::from(limits.max_width_px) / f64::from(size.width))
        .min(f64::from(limits.max_height_px) / f64::from(size.height))
        .min(1.0);
    let available_pixels = limits.max_allocation_bytes / u64::from(limits.bytes_per_pixel);
    let requested_pixels = u64::from(size.width) * u64::from(size.height);
    let allocation_scale = if requested_pixels > available_pixels {
        (available_pixels as f64 / requested_pixels as f64).sqrt()
    } else {
        1.0
    };
    let scale = dimension_scale.min(allocation_scale);
    PixelSize {
        width: (f64::from(size.width) * scale).floor().max(1.0) as u32,
        height: (f64::from(size.height) * scale).floor().max(1.0) as u32,
    }
}

fn ratio_basis(width: f64, height: f64) -> (u32, u32) {
    const BASIS: f64 = 1_000_000.0;
    if width >= height {
        (
            BASIS as u32,
            (height / width * BASIS).round().max(1.0) as u32,
        )
    } else {
        (
            (width / height * BASIS).round().max(1.0) as u32,
            BASIS as u32,
        )
    }
}

fn dimension_changed(cached: u32, requested: u32, min_pixels: u32, min_relative: f64) -> bool {
    let delta = cached.abs_diff(requested);
    let relative = f64::from(delta) / f64::from(cached.max(1));
    delta >= min_pixels || relative >= min_relative
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport(width: f64, height: f64) -> LogicalSize {
        LogicalSize { width, height }
    }

    #[test]
    fn raster_targets_physical_pixels_at_one_and_two_x() {
        let source = PixelSize {
            width: 4_000,
            height: 2_000,
        };
        let one = plan_raster(
            "image",
            source,
            viewport(800.0, 600.0),
            1.0,
            MediaLimits::default(),
        )
        .unwrap();
        let two = plan_raster(
            "image",
            source,
            viewport(800.0, 600.0),
            2.0,
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(
            one.target_px,
            PixelSize {
                width: 800,
                height: 400
            }
        );
        assert_eq!(
            two.target_px,
            PixelSize {
                width: 1_600,
                height: 800
            }
        );
        assert_eq!(two.viewport_logical, viewport(800.0, 600.0));
        assert_eq!(
            two.source_dimensions,
            SourceDimensions::RasterPixels(source)
        );
        assert_eq!(two.display_logical, viewport(800.0, 400.0));
    }

    #[test]
    fn raster_refuses_to_upscale() {
        let plan = plan_raster(
            "small",
            PixelSize {
                width: 640,
                height: 480,
            },
            viewport(1_920.0, 1_080.0),
            2.0,
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(
            plan.target_px,
            PixelSize {
                width: 640,
                height: 480
            }
        );
        assert_eq!(plan.display_logical, viewport(320.0, 240.0));
    }

    #[test]
    fn limits_bound_dimensions_and_allocation() {
        let limits = MediaLimits {
            max_width_px: 2_000,
            max_height_px: 2_000,
            max_allocation_bytes: 4_000_000,
            bytes_per_pixel: 4,
        };
        let plan = plan_raster(
            "large",
            PixelSize {
                width: 8_000,
                height: 4_000,
            },
            viewport(4_000.0, 4_000.0),
            2.0,
            limits,
        )
        .unwrap();
        assert!(plan.target_px.width <= limits.max_width_px);
        assert!(plan.target_px.height <= limits.max_height_px);
        assert!(
            u64::from(plan.target_px.width) * u64::from(plan.target_px.height) * 4 <= 4_000_000
        );
        assert_eq!(plan.target_px.width, plan.target_px.height * 2);
    }

    #[test]
    fn pdf_uses_physical_target_and_page_aspect() {
        let plan = plan_pdf_page(
            "manual",
            7,
            viewport(612.0, 792.0),
            viewport(600.0, 600.0),
            2.0,
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(
            plan.target_px,
            PixelSize {
                width: 927,
                height: 1_200
            }
        );
        assert_eq!(
            plan.cache_key.variant,
            MediaVariant::PdfPage { page_index: 7 }
        );
    }

    #[test]
    fn resize_invalidation_ignores_jitter_and_detects_scale_changes() {
        let policy = ResizeInvalidation::default();
        assert!(!policy.should_replace(
            PixelSize {
                width: 1_600,
                height: 1_200
            },
            PixelSize {
                width: 1_610,
                height: 1_205
            },
        ));
        assert!(policy.should_replace(
            PixelSize {
                width: 1_600,
                height: 1_200
            },
            PixelSize {
                width: 1_700,
                height: 1_275
            },
        ));
        assert!(policy.should_replace(
            PixelSize {
                width: 800,
                height: 600
            },
            PixelSize {
                width: 1_600,
                height: 1_200
            },
        ));
    }

    #[test]
    fn cache_keys_are_stable_and_include_media_variant() {
        let size = PixelSize {
            width: 1_200,
            height: 800,
        };
        let raster = MediaCacheKey {
            source_id: "sha256:abc".to_owned(),
            variant: MediaVariant::Raster,
            pixel_size: size,
        };
        let pdf = MediaCacheKey {
            source_id: "sha256:abc".to_owned(),
            variant: MediaVariant::PdfPage { page_index: 0 },
            pixel_size: size,
        };
        assert_eq!(
            raster.stable_string(),
            "canvas-media-v1-10-sha256:abc-raster-1200x800"
        );
        assert_ne!(raster, pdf);
        assert_ne!(raster.stable_string(), pdf.stable_string());
    }
}
