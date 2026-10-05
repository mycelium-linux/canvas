//! Coherence mapping for gfx's existing versioned semantic GPU theme.

use crate::{InvalidMetrics, Tokens};
use fpl_gfx::{Color, MaterialTheme, Theme, ThemeRole};

/// Apply Coherence appearance to a caller-owned gfx theme.
///
/// Authored sRGB is converted to linear light by gfx, exactly once. The flat
/// product profile disables global gloss/grain/emission and shadows; explicit
/// per-primitive material overrides remain host-owned. Host-owned motion
/// and outline width are preserved: Coherence currently exports no GPU timing
/// or outline-width tokens. Resolve/update on appearance changes, not per frame.
pub fn apply(tokens: &Tokens, theme: &mut Theme) -> Result<(), InvalidMetrics> {
    tokens.validate()?;
    let c = tokens.colors;
    for (role, rgb) in [
        (ThemeRole::Background, c.base.background),
        (ThemeRole::Surface, c.base.surface),
        (ThemeRole::SurfaceRaised, c.surface_raised),
        (ThemeRole::SurfaceRecessed, c.surface_muted),
        (ThemeRole::Text, c.base.text),
        (ThemeRole::TextMuted, c.base.muted),
        (ThemeRole::Primary, c.base.primary),
        (ThemeRole::Secondary, c.info),
        (ThemeRole::Success, c.base.success),
        (ThemeRole::Warning, c.base.warning),
        (ThemeRole::Danger, c.base.danger),
        (ThemeRole::MeterLow, c.base.success),
        (ThemeRole::MeterHigh, c.base.danger),
        (ThemeRole::Outline, c.border_strong),
        (ThemeRole::Highlight, c.surface_raised),
    ] {
        theme.set_color(role, Color::from_srgb8(rgb[0], rgb[1], rgb[2], 255));
    }
    theme.set_color(ThemeRole::Shadow, Color::TRANSPARENT);
    theme.shapes.radius_small = tokens.metrics.radius_small;
    theme.shapes.radius_large = tokens.metrics.radius_large;
    theme.materials = MaterialTheme {
        gloss: 0.0,
        grain: 0.0,
        emission: 0.0,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Density, Mode};

    #[test]
    fn every_mode_maps_linear_colors_and_keeps_host_motion() {
        for mode in [Mode::Light, Mode::Dark, Mode::Sepia] {
            let tokens = Tokens::new(mode, Density::Compact);
            let mut theme = Theme::studio_dark();
            theme.motion.control_smoothing_ms = 17.0;
            theme.shapes.outline_width = 2.0;
            let timing = theme.motion;
            apply(&tokens, &mut theme).unwrap();
            theme.validate().unwrap();
            let [r, g, b] = mode.palette().primary;
            assert_eq!(
                theme.color(ThemeRole::Primary),
                Color::from_srgb8(r, g, b, 255)
            );
            assert_eq!(theme.color(ThemeRole::Shadow), Color::TRANSPARENT);
            assert_eq!(theme.shapes.radius_small, tokens.metrics.radius_small);
            assert_eq!(theme.shapes.outline_width, 2.0);
            assert_eq!(theme.motion, timing);
            assert_eq!(theme.materials.gloss, 0.0);
        }
    }

    #[test]
    fn adapter_does_not_mistake_srgb_for_linear_or_leave_old_palette_slots() {
        let tokens = Tokens::new(Mode::Dark, Density::Comfortable);
        let mut theme = Theme::studio_dark();
        theme.colors.fill(Color::linear(1.0, 0.0, 1.0, 1.0));
        apply(&tokens, &mut theme).unwrap();
        assert!(!theme.colors.contains(&Color::linear(1.0, 0.0, 1.0, 1.0)));
        let c = theme.color(ThemeRole::Background);
        assert!(c.r < f32::from(tokens.colors.base.background[0]) / 255.0);
    }
}
