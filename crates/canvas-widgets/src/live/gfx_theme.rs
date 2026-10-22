//! Canvas appearance bridge. Coherence owns renderer mapping; Canvas owns
//! resolved appearance, including imported palettes and Lua overrides.

use canvas_core::{ThemeMode, VisualTheme};
use coherence_native::{Density, InvalidMetrics, Mode, Tokens};

/// Translate the current Canvas snapshot without replacing its palette.
/// Canonical defaults supply only metrics not represented by Canvas yet.
pub fn tokens(visual: &VisualTheme) -> Tokens {
    let mode = match visual.mode {
        ThemeMode::Light => Mode::Light,
        ThemeMode::Dark => Mode::Dark,
    };
    let mut tokens = Tokens::new(mode, Density::Comfortable);
    let p = visual.palette;
    let c = &mut tokens.colors;
    c.base.background = p.background;
    c.base.surface = p.surface;
    c.base.text = p.text;
    c.base.muted = p.text_muted;
    c.base.primary = p.primary;
    c.base.success = p.success;
    c.base.warning = p.warning;
    c.base.danger = p.danger;
    c.background_strong = p.background_strong;
    c.surface_muted = p.surface_muted;
    c.surface_raised = visual.surface_raised;
    c.surface_accent = visual.surface_accent;
    c.surface_accent_strong = visual.surface_accent_strong;
    c.border = p.border;
    c.border_strong = p.border_strong;
    c.info = p.info;
    c.focus = visual.focus;
    let m = visual.metrics;
    tokens.metrics.small = m.small;
    tokens.metrics.body = m.body;
    tokens.metrics.large = m.large;
    tokens.metrics.title = m.title;
    tokens.metrics.display = m.display;
    tokens.metrics.space[..5]
        .copy_from_slice(&[m.space_1, m.space_2, m.space_3, m.space_4, m.space_5]);
    tokens.metrics.radius_small = m.radius_sm;
    tokens.metrics.radius_large = m.radius_lg;
    tokens.metrics.shell_radius = m.radius;
    tokens
}

/// Apply appearance to a host-owned GPU theme. Motion and outline width remain
/// host-owned; invalid geometry leaves the destination unchanged.
pub fn apply(visual: &VisualTheme, theme: &mut fpl_gfx::Theme) -> Result<(), InvalidMetrics> {
    coherence_native::gfx::apply(&tokens(visual), theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Accent, ThemeSelection};
    use fpl_gfx::{Color, Theme, ThemeRole};

    #[test]
    fn preserves_live_palette_and_metrics_in_both_modes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let mut visual = ThemeSelection::coherence(mode, Accent::Brand).visual_theme();
            visual.palette.primary = [21, 123, 231];
            visual.focus = [10, 20, 30];
            visual.metrics.radius_lg = 19.0;
            let snapshot = tokens(&visual);
            assert_eq!(snapshot.colors.base.primary, visual.palette.primary);
            assert_eq!(snapshot.colors.focus, visual.focus);
            let mut theme = Theme::studio_dark();
            let motion = theme.motion;
            apply(&visual, &mut theme).unwrap();
            assert_eq!(
                theme.color(ThemeRole::Primary),
                Color::from_srgb8(21, 123, 231, 255)
            );
            assert_eq!(theme.shapes.radius_large, 19.0);
            assert_eq!(theme.motion, motion);
            visual.palette.primary = [231, 123, 21];
            apply(&visual, &mut theme).unwrap();
            assert_eq!(
                theme.color(ThemeRole::Primary),
                Color::from_srgb8(231, 123, 21, 255)
            );
            theme.validate().unwrap();
        }
    }

    #[test]
    fn rejects_invalid_override_without_partial_mutation() {
        let mut visual = ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).visual_theme();
        visual.metrics.radius_sm = f32::NAN;
        let mut theme = Theme::studio_dark();
        let before = theme.clone();
        assert!(apply(&visual, &mut theme).is_err());
        assert_eq!(theme, before);
    }
}
