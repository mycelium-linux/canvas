//! Coherence's presentation language, independent of a compositor or app.
//!
//! Resolve [`Tokens`] when appearance changes, then apply the optional renderer
//! adapters. No filesystem, GPU device, window, service, or app-state ownership.
//! Values come from the generated `coherence-tokens` artifact, never a copied
//! native palette. Dimensions are logical pixels, colors are authored sRGB.

pub use coherence_tokens::{Density, Mode};

#[cfg(feature = "egui")]
pub mod egui;
#[cfg(feature = "gfx")]
pub mod gfx;

/// The common resolved view consumed by all native adapters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Explicit appearance; sepia is never an inferred OS preference.
    pub mode: Mode,
    /// Control/row density, orthogonal to appearance.
    pub density: Density,
    /// Semantic sRGB colors.
    pub colors: Colors,
    /// Logical geometry and typography.
    pub metrics: Metrics,
}

/// Extended semantic colors, all resolved from the canonical product profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    /// Existing renderer-independent palette (ground, surface, text and status).
    pub base: coherence_tokens::Palette,
    /// Alternate ground.
    pub background_strong: [u8; 3],
    /// Quiet wells and striped rows.
    pub surface_muted: [u8; 3],
    /// Raised controls and panels.
    pub surface_raised: [u8; 3],
    /// Hovered/selected control background.
    pub surface_accent: [u8; 3],
    /// Active control background.
    pub surface_accent_strong: [u8; 3],
    /// Quiet separating hairlines.
    pub border: [u8; 3],
    /// Interactive outlines.
    pub border_strong: [u8; 3],
    /// Informational/secondary accent.
    pub info: [u8; 3],
    /// Keyboard focus outline.
    pub focus: [u8; 3],
}

/// Shared native layout metrics, not physical framebuffer dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// CSS fallback stack for the host's font loader; not an installed font.
    pub sans_family: &'static str,
    /// Small annotation font size.
    pub small: f32,
    /// Body/control font size.
    pub body: f32,
    /// Section font size.
    pub large: f32,
    /// Title font size.
    pub title: f32,
    /// Display font size.
    pub display: f32,
    /// Body line-height multiplier (egui does not have a global equivalent).
    pub line_height: f32,
    /// Canonical spacing scale, smallest first.
    pub space: [f32; 8],
    /// Small control radius.
    pub radius_small: f32,
    /// Larger panel/menu radius.
    pub radius_large: f32,
    /// Outer native shell radius.
    pub shell_radius: f32,
    /// Minimum control height for the selected density.
    pub control_height: f32,
    /// List/table row height for the selected density.
    pub row_height: f32,
}

impl Tokens {
    /// Validate host overrides before either renderer is mutated.
    ///
    /// Bounds match the shared logical-pixel contract, including egui's compact
    /// margins/radii. Generated themes pass; invalid overrides fail loudly.
    pub fn validate(&self) -> Result<(), InvalidMetrics> {
        let m = self.metrics;
        let bounded = |n: f32, low: f32, high: f32| n.is_finite() && (low..=high).contains(&n);
        if ![
            m.small,
            m.body,
            m.large,
            m.title,
            m.display,
            m.control_height,
            m.row_height,
        ]
        .into_iter()
        .all(|n| bounded(n, 1.0, 512.0))
            || !m.space.into_iter().all(|n| bounded(n, 0.0, 127.0))
            || ![m.radius_small, m.radius_large, m.shell_radius]
                .into_iter()
                .all(|n| bounded(n, 0.0, 255.0))
            || !bounded(m.line_height, 1.0, 3.0)
            || m.sans_family.is_empty()
            || m.sans_family.len() > 1024
            || m.sans_family.chars().any(char::is_control)
        {
            return Err(InvalidMetrics);
        }
        Ok(())
    }

    /// Resolve the canonical artifact without I/O or renderer dependencies.
    pub fn new(mode: Mode, density: Density) -> Self {
        // This maps names, not values. A generator change propagates to every
        // renderer without updating or copying an adapter palette.
        macro_rules! resolve {
            ($module:ident) => {{
                use coherence_tokens::$module as t;
                Self {
                    mode,
                    density,
                    colors: Colors {
                        base: mode.palette(),
                        background_strong: t::COLOR_BACKGROUND_STRONG,
                        surface_muted: t::COLOR_SURFACE_MUTED,
                        surface_raised: t::COLOR_SURFACE_RAISED,
                        surface_accent: t::COLOR_SURFACE_ACCENT,
                        surface_accent_strong: t::COLOR_SURFACE_ACCENT_STRONG,
                        border: t::COLOR_BORDER,
                        border_strong: t::COLOR_BORDER_STRONG,
                        info: t::COLOR_INFO,
                        focus: t::COLOR_FOCUS,
                    },
                    metrics: Metrics {
                        sans_family: t::FONT_SANS,
                        small: t::FONT_SIZE_SM,
                        body: t::FONT_SIZE_BASE,
                        large: t::FONT_SIZE_LG,
                        title: t::FONT_SIZE_XL,
                        display: t::FONT_SIZE_2XL,
                        line_height: t::LINE_HEIGHT_BASE,
                        space: [
                            t::SPACE_1,
                            t::SPACE_2,
                            t::SPACE_3,
                            t::SPACE_4,
                            t::SPACE_5,
                            t::SPACE_6,
                            t::SPACE_7,
                            t::SPACE_8,
                        ],
                        radius_small: t::RADIUS_SM,
                        radius_large: t::RADIUS_LG,
                        shell_radius: t::SHELL_RADIUS,
                        control_height: density.control_height(),
                        row_height: density.row_height(),
                    },
                }
            }};
        }
        match mode {
            Mode::Light => resolve!(light),
            Mode::Dark => resolve!(dark),
            Mode::Sepia => resolve!(sepia),
        }
    }
}

/// Rejected non-finite, negative, or out-of-contract host typography/geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMetrics;

impl std::fmt::Display for InvalidMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Coherence native metrics must be finite and within the logical-pixel contract")
    }
}
impl std::error::Error for InvalidMetrics {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_modes_and_densities_resolve_from_generated_tokens() {
        for mode in [Mode::Light, Mode::Dark, Mode::Sepia] {
            let normal = Tokens::new(mode, Density::Comfortable);
            let compact = Tokens::new(mode, Density::Compact);
            normal.validate().unwrap();
            compact.validate().unwrap();
            assert_eq!(normal.colors.base, mode.palette());
            assert_eq!(compact.colors, normal.colors);
            assert_eq!(compact.metrics.body, normal.metrics.body);
            assert!(compact.metrics.control_height < normal.metrics.control_height);
            assert!(compact.metrics.row_height < normal.metrics.row_height);
            assert!(normal.metrics.space.windows(2).all(|w| w[0] < w[1]));
        }
        assert_eq!(
            Tokens::new(Mode::Sepia, Density::Compact)
                .colors
                .surface_raised,
            coherence_tokens::sepia::COLOR_SURFACE_RAISED
        );
    }
}
