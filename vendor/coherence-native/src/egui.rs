//! Coherence mapping for egui 0.33: colors, widget states and logical geometry.

use crate::{InvalidMetrics, Mode, Tokens};
use ::egui::{vec2, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Style, TextStyle};

fn color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Apply appearance without replacing the host's behavior or custom text styles.
///
/// Uses egui's registered proportional/monospace families; this does not install
/// Geist or interpret a CSS font stack. Font loading is explicitly host-owned.
/// `row_height` and `line_height` remain available in [`Tokens`] for custom rows
/// and prose: egui has no global equivalents. Call when appearance changes.
pub fn apply(tokens: &Tokens, style: &mut Style) -> Result<(), InvalidMetrics> {
    tokens.validate()?;
    let c = tokens.colors;
    let m = tokens.metrics;
    let v = &mut style.visuals;
    v.dark_mode = tokens.mode == Mode::Dark;
    v.override_text_color = Some(color(c.base.text));
    v.weak_text_color = Some(color(c.base.muted));
    v.hyperlink_color = color(c.base.primary);
    v.warn_fg_color = color(c.base.warning);
    v.error_fg_color = color(c.base.danger);
    v.panel_fill = color(c.base.background);
    v.window_fill = color(c.base.surface);
    v.faint_bg_color = color(c.surface_muted);
    v.extreme_bg_color = color(c.base.surface);
    v.text_edit_bg_color = Some(color(c.base.surface));
    v.code_bg_color = color(c.surface_muted);
    v.selection.bg_fill = color(c.surface_accent_strong);
    v.selection.stroke = Stroke::new(1.0_f32, color(c.base.text));
    v.window_stroke = Stroke::new(1.0_f32, color(c.border));
    v.window_corner_radius = CornerRadius::from(m.shell_radius);
    v.menu_corner_radius = CornerRadius::from(m.radius_large);
    v.window_shadow = ::egui::epaint::Shadow::NONE;
    v.popup_shadow = ::egui::epaint::Shadow::NONE;
    v.text_cursor.stroke.color = color(c.focus);
    for (widget, fill, outline) in [
        (&mut v.widgets.noninteractive, c.base.surface, c.border),
        (&mut v.widgets.inactive, c.base.surface, c.border_strong),
        (&mut v.widgets.hovered, c.surface_accent, c.focus),
        (&mut v.widgets.active, c.surface_accent_strong, c.focus),
        (&mut v.widgets.open, c.surface_accent, c.focus),
    ] {
        widget.bg_fill = color(fill);
        widget.weak_bg_fill = color(fill);
        widget.bg_stroke = Stroke::new(1.0_f32, color(outline));
        widget.fg_stroke = Stroke::new(1.0_f32, color(c.base.text));
        widget.corner_radius = CornerRadius::from(m.radius_small);
        widget.expansion = 0.0;
    }
    style.spacing.item_spacing = vec2(m.space[1], m.space[0]);
    style.spacing.window_margin = Margin::from(m.space[3]);
    style.spacing.menu_margin = Margin::from(m.space[1]);
    style.spacing.button_padding = vec2(m.space[2], m.space[0]);
    style.spacing.interact_size = vec2(m.control_height, m.control_height);
    style.spacing.indent = m.space[3];
    style.spacing.icon_width = m.body;
    style.spacing.icon_width_inner = m.small;
    style.text_styles.extend([
        (
            TextStyle::Small,
            FontId::new(m.small, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(m.body, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(m.body, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(m.title, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(m.body, FontFamily::Monospace),
        ),
    ]);
    Ok(())
}

/// Construct a stock egui style using the shared Coherence token view.
pub fn style(tokens: &Tokens) -> Result<Style, InvalidMetrics> {
    let mut style = Style::default();
    apply(tokens, &mut style)?;
    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Density;

    #[test]
    fn all_modes_map_states_and_colors_without_changing_behavior() {
        for mode in [Mode::Light, Mode::Dark, Mode::Sepia] {
            let tokens = Tokens::new(mode, Density::Compact);
            let mut s = Style {
                animation_time: 0.037,
                ..Style::default()
            };
            let custom = TextStyle::Name("host-owned".into());
            s.text_styles
                .insert(custom.clone(), FontId::proportional(23.0));
            apply(&tokens, &mut s).unwrap();
            assert_eq!(s.animation_time, 0.037);
            assert_eq!(s.text_styles[&custom].size, 23.0);
            assert_eq!(s.visuals.dark_mode, mode == Mode::Dark);
            assert_eq!(s.visuals.panel_fill, color(tokens.colors.base.background));
            assert_eq!(s.visuals.window_fill, color(tokens.colors.base.surface));
            assert_eq!(
                s.visuals.widgets.hovered.bg_fill,
                color(tokens.colors.surface_accent)
            );
            assert_eq!(
                s.visuals.widgets.active.bg_stroke.color,
                color(tokens.colors.focus)
            );
            assert_eq!(
                s.visuals.weak_text_color,
                Some(color(tokens.colors.base.muted))
            );
            assert_eq!(s.visuals.window_shadow, ::egui::epaint::Shadow::NONE);
            assert_eq!(s.text_styles[&TextStyle::Body].size, tokens.metrics.body);
        }
    }

    #[test]
    fn density_changes_minimum_targets_not_font_size_or_palette() {
        let normal = style(&Tokens::new(Mode::Dark, Density::Comfortable)).unwrap();
        let compact = style(&Tokens::new(Mode::Dark, Density::Compact)).unwrap();
        assert!(compact.spacing.interact_size.y < normal.spacing.interact_size.y);
        assert_eq!(compact.text_styles, normal.text_styles);
        assert_eq!(compact.visuals, normal.visuals);
    }
}
