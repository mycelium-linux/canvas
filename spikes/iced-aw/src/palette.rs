//! The bridge from `canvas_core::VisualTheme` to both widget sets.
//!
//! The point of this module is the comparison it makes possible: the plain-iced
//! closures and the iced_aw closures are the same shape and the same length.
//! Both widget sets take `impl Fn(&Theme, Status) -> Style`, so an externally
//! supplied palette CAN reach an iced_aw mark. The difference is only what the
//! *defaults* do when you don't write the closure.

use canvas_core::VisualTheme;
use iced::widget::{button, container, pick_list, text_input};
use iced::{Background, Border, Color, Shadow, Theme, border};

pub fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

fn alpha(rgb: [u8; 3], a: f32) -> Color {
    let c = color(rgb);
    Color { a, ..c }
}

/// The app ground.
pub fn screen(v: VisualTheme) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color(v.palette.background))),
        text_color: Some(color(v.palette.text)),
        ..Default::default()
    }
}

/// A raised panel: `surface_raised` + `border`, radius from `metrics`.
pub fn card(v: VisualTheme) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color(v.surface_raised))),
        text_color: Some(color(v.palette.text)),
        border: Border {
            color: color(v.palette.border),
            width: 1.0,
            radius: border::radius(v.metrics.radius_md),
        },
        shadow: Shadow {
            color: Color {
                a: v.shadow_opacity,
                ..Color::BLACK
            },
            offset: iced::Vector::new(0.0, v.shadow_offset_y),
            blur_radius: v.shadow_blur,
        },
        ..Default::default()
    }
}

/// A status pill — what `iced_aw::badge` is for, in six lines of plain iced.
pub fn pill(v: VisualTheme, tint: [u8; 3]) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color(tint))),
        text_color: Some(color(v.palette.background)),
        border: Border {
            radius: border::radius(v.metrics.radius_xs),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A filled action button. Hover/press states come off `surface_accent_strong`
/// and `focus`, both of which exist in VisualTheme and neither of which survives
/// the projection down to `iced::theme::Palette`.
pub fn solid(v: VisualTheme, tint: [u8; 3]) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let base = color(tint);
        let bg = match status {
            button::Status::Hovered => Color { a: 0.86, ..base },
            button::Status::Pressed => Color { a: 0.72, ..base },
            button::Status::Disabled => alpha(tint, 0.35),
            button::Status::Active => base,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: color(v.palette.background),
            border: Border {
                radius: border::radius(v.metrics.radius_sm),
                width: if matches!(status, button::Status::Hovered) {
                    1.0
                } else {
                    0.0
                },
                color: color(v.focus),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// A quiet, list-row style button — the one Canvas needs for RSS items and
/// project tiles.
pub fn ghost(v: VisualTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered => color(v.surface_accent),
            button::Status::Pressed => color(v.surface_accent_strong),
            _ => Color::TRANSPARENT,
        })),
        text_color: match status {
            button::Status::Disabled => alpha(v.palette.text_muted, 0.5),
            button::Status::Active => color(v.palette.text_muted),
            _ => color(v.palette.text),
        },
        border: Border {
            radius: border::radius(v.metrics.radius_sm),
            width: 1.0,
            color: match status {
                button::Status::Hovered => color(v.palette.border_strong),
                _ => Color::TRANSPARENT,
            },
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn input(v: VisualTheme) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| text_input::Style {
        background: Background::Color(color(v.palette.surface)),
        border: Border {
            color: match status {
                text_input::Status::Focused { .. } => color(v.focus),
                text_input::Status::Hovered => color(v.palette.border_strong),
                _ => color(v.palette.border),
            },
            width: 1.0,
            radius: border::radius(v.metrics.radius_sm),
        },
        icon: color(v.palette.text_muted),
        placeholder: alpha(v.palette.text_muted, 0.7),
        value: color(v.palette.text),
        selection: alpha(v.focus, 0.35),
    }
}

pub fn picklist(v: VisualTheme) -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style {
    move |_, status| pick_list::Style {
        text_color: color(v.palette.text),
        placeholder_color: color(v.palette.text_muted),
        handle_color: color(v.palette.text_muted),
        background: Background::Color(color(v.palette.surface)),
        border: Border {
            color: match status {
                pick_list::Status::Hovered | pick_list::Status::Opened { .. } => {
                    color(v.palette.border_strong)
                }
                _ => color(v.palette.border),
            },
            width: 1.0,
            radius: border::radius(v.metrics.radius_sm),
        },
    }
}

// ---------------------------------------------------------------- iced_aw
//
// Same closures, iced_aw's Style structs. `iced_aw::style::StyleFn` is
// `Box<dyn Fn(&Theme, Status) -> Style>` where `Theme` is `iced_core::Theme`,
// i.e. the exact type Canvas already builds with `Theme::custom`. So there is
// no second theme model to fight — but there is a second `Status` enum, and a
// per-widget `Style` struct whose fields don't line up with iced's.

pub fn aw_card(
    v: VisualTheme,
) -> impl Fn(&Theme, iced_aw::style::Status) -> iced_aw::style::card::Style {
    move |_, _status| iced_aw::style::card::Style {
        background: Background::Color(color(v.surface_raised)),
        border_radius: v.metrics.radius_md,
        border_width: 1.0,
        border_color: color(v.palette.border),
        head_background: Background::Color(color(v.surface_accent)),
        head_text_color: color(v.palette.text),
        body_background: Background::Color(Color::TRANSPARENT),
        body_text_color: color(v.palette.text_muted),
        foot_background: Background::Color(Color::TRANSPARENT),
        foot_text_color: color(v.palette.text),
        close_color: color(v.palette.text_muted),
    }
}

pub fn aw_menu(
    v: VisualTheme,
) -> impl Fn(&Theme, iced_aw::style::Status) -> iced_aw::style::menu_bar::Style {
    move |_, _status| {
        let panel = Border {
            color: color(v.palette.border),
            width: 1.0,
            radius: border::radius(v.metrics.radius_md),
        };
        iced_aw::style::menu_bar::Style {
            bar_background: Background::Color(Color::TRANSPARENT),
            bar_border: Border::default(),
            bar_shadow: Shadow::default(),
            menu_background: Background::Color(color(v.surface_raised)),
            menu_border: panel,
            menu_shadow: Shadow {
                color: Color {
                    a: v.shadow_opacity,
                    ..Color::BLACK
                },
                offset: iced::Vector::new(0.0, v.shadow_offset_y),
                blur_radius: v.shadow_blur,
            },
            path: Background::Color(color(v.surface_accent)),
            path_border: Border {
                radius: border::radius(v.metrics.radius_sm),
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Accent, ThemeMode, ThemeSelection};

    fn visual() -> VisualTheme {
        ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).visual_theme()
    }

    /// The theming question, made falsifiable: an externally supplied palette
    /// role must be observable in the iced_aw `Style` the widget will draw.
    #[test]
    fn an_external_palette_reaches_an_iced_aw_mark() {
        let v = visual();
        let style = aw_card(v)(&Theme::Dark, iced_aw::style::Status::Active);
        assert_eq!(
            style.border_color,
            color(v.palette.border),
            "iced_aw card border must come from the Canvas palette"
        );
        assert_eq!(style.border_radius, v.metrics.radius_md);
        assert_eq!(
            style.head_background,
            Background::Color(color(v.surface_accent))
        );
    }

    /// And the negative result: iced_aw's OWN default styling does not. It is a
    /// hybrid — background/text follow the theme, accents are hardcoded CSS.
    #[test]
    fn iced_aw_default_card_style_ignores_the_canvas_accent() {
        let v = visual();
        let theme = Theme::custom(
            "canvas",
            iced::theme::Palette {
                background: color(v.palette.background),
                text: color(v.palette.text),
                primary: color(v.palette.primary),
                success: color(v.palette.success),
                danger: color(v.palette.danger),
                warning: color(v.palette.warning),
            },
        );
        let style =
            iced_aw::style::card::primary(&theme, iced_aw::style::Status::Active);

        // `iced_aw::style::colors::PRIMARY` is DODGER_BLUE, a `const` in the
        // crate. It is used verbatim, so the head of a `card` styled
        // `card::primary` is dodger blue no matter what Canvas's accent is.
        assert_eq!(
            style.head_background,
            Background::Color(Color::from_rgb(0.118, 0.565, 1.0)),
            "iced_aw's card::primary hardcodes dodger blue"
        );
        assert_ne!(
            style.head_background,
            Background::Color(color(v.palette.primary)),
            "…which is NOT the Canvas primary"
        );

        // But the surface and text DO follow the supplied theme, so it is a
        // partial regression, not a total one.
        assert_eq!(
            style.body_text_color,
            color(v.palette.text),
            "body text does follow the supplied palette"
        );
    }

    /// iced_aw's default `Style` (used whenever a field is left to
    /// `..Style::default()`) is a light-mode Bootstrap card. Canvas in dark mode
    /// would get white-on-white unless every field is overridden.
    #[test]
    fn iced_aw_style_default_is_light_mode_bootstrap() {
        let d = iced_aw::style::card::Style::default();
        assert_eq!(d.background, Background::Color(Color::WHITE));
        assert_eq!(d.body_text_color, Color::BLACK);
        assert_eq!(d.border_radius, 10.0, "not any Canvas metric");
    }

    /// The metrics half of the theme never reaches iced_aw's defaults at all:
    /// `iced::theme::Palette` has no radius or spacing, so a hand-written
    /// closure is the ONLY way to get Canvas's radii onto an iced_aw widget.
    #[test]
    fn iced_theme_palette_carries_no_metrics() {
        let v = visual();
        assert_ne!(
            v.metrics.radius_md, 10.0,
            "Canvas radius differs from iced_aw's hardcoded default, so the \
             mismatch is visible rather than accidental parity"
        );
    }
}
