//! Shared native chrome for widgets that behave like small applications.
//!
//! The viewer owns semantics and messages; this module owns the consistent
//! Coherence-derived control strip used by document and engineering viewers.

use canvas_core::VisualTheme;
use iced::widget::{button, container, row, space, text};
use iced::{Color, Element, Length};

/// Resolution-independent symbols; no font glyphs, file paths, or SVG parsing.
pub fn shell_icon<Message: 'static>(
    icon: crate::ShellIcon,
    size: f32,
    ink: Color,
) -> Element<'static, Message> {
    use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
    struct Symbol(crate::ShellIcon, Color);
    impl<Message> canvas::Program<Message> for Symbol {
        type State = ();
        fn draw(
            &self,
            _: &(),
            renderer: &iced::Renderer,
            _: &iced::Theme,
            bounds: iced::Rectangle,
            _: iced::mouse::Cursor,
        ) -> Vec<Geometry> {
            let mut frame = Frame::new(renderer, bounds.size());
            let s = bounds.width.min(bounds.height) / 24.0;
            frame.scale(s);
            let stroke = Stroke::default().with_color(self.1).with_width(1.6);
            let line = |frame: &mut Frame, a: [f32; 2], b: [f32; 2]| {
                frame.stroke(&Path::line(a.into(), b.into()), stroke);
            };
            use crate::ShellIcon;
            match self.0 {
                ShellIcon::Apps => {
                    for x in [5.0, 12.0, 19.0] {
                        for y in [5.0, 12.0, 19.0] {
                            frame.fill(&Path::circle(iced::Point::new(x, y), 1.5), self.1);
                        }
                    }
                }
                ShellIcon::Window | ShellIcon::Browser => {
                    frame.stroke(
                        &Path::rectangle(iced::Point::new(3.0, 4.0), iced::Size::new(18.0, 16.0)),
                        stroke,
                    );
                    line(&mut frame, [3.0, 9.0], [21.0, 9.0]);
                    if self.0 == ShellIcon::Browser {
                        for x in [6.0, 9.0, 12.0] {
                            frame.fill(&Path::circle(iced::Point::new(x, 6.5), 0.65), self.1);
                        }
                    }
                }
                ShellIcon::Terminal => {
                    line(&mut frame, [4.0, 7.0], [9.0, 12.0]);
                    line(&mut frame, [9.0, 12.0], [4.0, 17.0]);
                    line(&mut frame, [12.0, 17.0], [20.0, 17.0]);
                }
                ShellIcon::Document => {
                    frame.stroke(
                        &Path::rectangle(iced::Point::new(6.0, 3.0), iced::Size::new(12.0, 18.0)),
                        stroke,
                    );
                    for y in [8.0, 12.0, 16.0] {
                        line(&mut frame, [9.0, y], [15.0, y]);
                    }
                }
                ShellIcon::Chart => {
                    for (x, y) in [(5.0, 13.0), (12.0, 7.0), (19.0, 10.0)] {
                        line(&mut frame, [x, 20.0], [x, y]);
                    }
                }
            }
            vec![frame.into_geometry()]
        }
    }
    canvas::Canvas::new(Symbol(icon, ink))
        .width(size)
        .height(size)
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Persistent tools: browsers, timelines, inspectors and transport bars.
    Compact,
    /// Reading and presentation surfaces where breathing room aids scanning.
    Comfortable,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Insets {
    pub gap: f32,
    pub control_y: f32,
    pub control_x: f32,
    pub panel: f32,
}

impl Density {
    pub(super) fn insets(self, visual: VisualTheme) -> Insets {
        let metrics = visual.metrics;
        match self {
            Self::Compact => Insets {
                gap: metrics.space_1,
                control_y: (metrics.space_1 * 0.5).max(2.0),
                control_x: metrics.space_1,
                panel: metrics.space_1,
            },
            Self::Comfortable => Insets {
                gap: metrics.space_2,
                control_y: metrics.space_1,
                control_x: metrics.space_2,
                panel: metrics.space_3,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum Item<Message> {
    Action {
        label: String,
        message: Option<Message>,
        active: bool,
        faded: bool,
    },
    Status(String),
    Divider,
}

impl<Message> Item<Message> {
    pub fn action(label: impl Into<String>, message: Message) -> Self {
        Self::Action {
            label: label.into(),
            message: Some(message),
            active: false,
            faded: false,
        }
    }

    pub fn active(label: impl Into<String>, message: Message) -> Self {
        Self::Action {
            label: label.into(),
            message: Some(message),
            active: true,
            faded: false,
        }
    }

    pub fn disabled(label: impl Into<String>) -> Self {
        Self::Action {
            label: label.into(),
            message: None,
            active: false,
            faded: false,
        }
    }

    pub fn faded(label: impl Into<String>, message: Message) -> Self {
        Self::Action {
            label: label.into(),
            message: Some(message),
            active: false,
            faded: true,
        }
    }
}

pub fn bar<'a, Message: Clone + 'a>(
    visual: VisualTheme,
    items: impl IntoIterator<Item = Item<Message>>,
) -> Element<'a, Message> {
    bar_with_density(visual, Density::Compact, items)
}

pub fn bar_with_density<'a, Message: Clone + 'a>(
    visual: VisualTheme,
    density: Density,
    items: impl IntoIterator<Item = Item<Message>>,
) -> Element<'a, Message> {
    let palette = visual.palette;
    let metrics = visual.metrics;
    let insets = density.insets(visual);
    let mut controls = row![].spacing(insets.gap).align_y(iced::Alignment::Center);

    for item in items {
        let element: Element<'a, Message> = match item {
            Item::Action {
                label,
                message,
                active,
                faded,
            } => button(text(label).size(metrics.small).center())
                .padding([insets.control_y, insets.control_x])
                .on_press_maybe(message)
                .style(move |_, status| action_style(visual, active, faded, status))
                .into(),
            Item::Status(label) => container(
                text(label)
                    .size(metrics.small)
                    .color(rgb(palette.text_muted)),
            )
            .padding([insets.control_y, insets.control_x])
            .into(),
            Item::Divider => container(space::horizontal())
                .width(Length::Fixed(1.0))
                .height(Length::Fixed(metrics.body))
                .style(move |_| container::Style {
                    background: Some(rgb(palette.border).into()),
                    ..Default::default()
                })
                .into(),
        };
        controls = controls.push(element);
    }

    container(controls)
        .width(Length::Fill)
        .padding(insets.panel)
        .align_x(iced::alignment::Horizontal::Center)
        .style(move |_| panel_style_with_density(visual, density))
        .into()
}

pub(super) fn panel_style(visual: VisualTheme) -> container::Style {
    panel_style_with_density(visual, Density::Comfortable)
}

fn panel_style_with_density(visual: VisualTheme, density: Density) -> container::Style {
    container::Style {
        background: Some(
            Color {
                a: visual.panel_opacity,
                ..rgb(visual.palette.surface)
            }
            .into(),
        ),
        border: iced::Border {
            color: rgb(visual.palette.border),
            width: 1.0,
            radius: if density == Density::Compact {
                0.0
            } else {
                visual.metrics.radius_sm
            }
            .into(),
        },
        ..Default::default()
    }
}

pub(super) fn work_area_style(visual: VisualTheme) -> container::Style {
    container::Style {
        background: Some(rgb(visual.palette.background).into()),
        ..Default::default()
    }
}

pub(super) fn list_item_style(
    visual: VisualTheme,
    selected: bool,
    status: button::Status,
) -> button::Style {
    let palette = visual.palette;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: (selected || hovered).then(|| {
            Color {
                a: if selected { 0.72 } else { 0.42 },
                ..rgb(if selected {
                    visual.surface_accent
                } else {
                    palette.surface_muted
                })
            }
            .into()
        }),
        text_color: rgb(palette.text),
        border: iced::Border {
            color: if selected {
                rgb(visual.focus)
            } else {
                Color::TRANSPARENT
            },
            width: if selected { 1.0 } else { 0.0 },
            radius: visual.metrics.radius_xs.into(),
        },
        ..Default::default()
    }
}

pub(super) fn action_style(
    visual: VisualTheme,
    active: bool,
    faded: bool,
    status: button::Status,
) -> button::Style {
    let palette = visual.palette;
    let disabled = status == button::Status::Disabled;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = if active {
        rgb(visual.surface_accent_strong)
    } else if hovered {
        rgb(visual.surface_raised)
    } else {
        rgb(palette.surface_muted)
    };
    let foreground = if disabled || (faded && !hovered) {
        Color {
            a: visual.muted_opacity,
            ..rgb(palette.text_muted)
        }
    } else if active {
        rgb(palette.primary)
    } else {
        rgb(palette.text)
    };
    button::Style {
        background: Some(
            Color {
                a: if disabled {
                    0.35
                } else if faded && !hovered {
                    0.28
                } else {
                    0.88
                },
                ..background
            }
            .into(),
        ),
        text_color: foreground,
        border: iced::Border {
            color: if active {
                rgb(visual.focus)
            } else {
                rgb(palette.border)
            },
            width: if active { 1.5 } else { 1.0 },
            radius: visual.metrics.radius_xs.into(),
        },
        ..Default::default()
    }
}

pub fn cube_button_style(
    visual: VisualTheme,
    active: bool,
    status: button::Status,
) -> button::Style {
    shared_button_style(
        visual,
        active,
        status,
        fpl_gfx::chrome::ControlAppearance::Standard,
    )
}

fn shared_button_style(
    visual: VisualTheme,
    active: bool,
    status: button::Status,
    appearance: fpl_gfx::chrome::ControlAppearance,
) -> button::Style {
    // The same canonical face is visited by Android's raster adapter. Iced
    // retains labels and input, while GFX owns shape and state semantics.
    let shapes = fpl_gfx::ShapeTheme {
        radius_small: visual.metrics.radius_sm,
        radius_large: visual.metrics.radius_lg,
        outline_width: 1.0,
    };
    let control = fpl_gfx::chrome::control_visual(
        fpl_gfx::Rect::new(0.0, 0.0, 100.0, 32.0),
        shapes,
        fpl_gfx::chrome::ControlState {
            selected: active,
            focused: status == button::Status::Hovered,
            pressed: status == button::Status::Pressed,
            disabled: status == button::Status::Disabled,
        },
        appearance,
    )
    .expect("validated Canvas chrome metrics");
    let face = control.face;
    let ink = |role| match role {
        fpl_gfx::ThemeRole::SurfaceRaised => rgb(visual.palette.surface_muted),
        fpl_gfx::ThemeRole::Primary => rgb(visual.palette.primary),
        fpl_gfx::ThemeRole::Outline => rgb(visual.palette.border),
        fpl_gfx::ThemeRole::SurfaceRecessed => rgb(visual.palette.background_strong),
        fpl_gfx::ThemeRole::Text => rgb(visual.palette.text),
        fpl_gfx::ThemeRole::TextMuted => rgb(visual.palette.text_muted),
        _ => rgb(visual.palette.surface),
    };
    button::Style {
        background: (control.face_opacity > 0.0).then(|| {
            Color {
                a: control.face_opacity,
                ..ink(face.fill)
            }
            .into()
        }),
        text_color: ink(control.ink),
        border: iced::Border {
            color: ink(face.outline),
            width: face.outline_width,
            radius: face.radius.into(),
        },
        ..Default::default()
    }
}

/// Borderless shell control, sharing the same Coherence state colors as app
/// chrome. The host still owns native focus, disabled state and hit testing.
pub fn quiet_button_style(
    visual: VisualTheme,
    active: bool,
    status: button::Status,
) -> button::Style {
    shared_button_style(
        visual,
        active,
        status,
        fpl_gfx::chrome::ControlAppearance::Quiet,
    )
}

pub(super) fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_state_is_explicit_instead_of_inferred_from_the_label() {
        let Item::Action {
            message,
            active,
            faded,
            ..
        } = Item::active("ORBIT ON", 7)
        else {
            panic!("active action");
        };
        assert_eq!(message, Some(7));
        assert!(active);
        assert!(!faded);

        let Item::Action {
            message, active, ..
        } = Item::<u8>::disabled("PREVIOUS")
        else {
            panic!("disabled action");
        };
        assert_eq!(message, None);
        assert!(!active);
    }

    #[test]
    fn quiet_controls_keep_native_state_without_idle_box_borders() {
        let visual = canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .visual_theme();
        let idle = quiet_button_style(visual, false, button::Status::Active);
        let hovered = quiet_button_style(visual, false, button::Status::Hovered);
        let selected = quiet_button_style(visual, true, button::Status::Active);
        let disabled = quiet_button_style(visual, false, button::Status::Disabled);
        assert!(idle.background.is_none());
        assert!(hovered.background.is_some());
        assert!(selected.background.is_some());
        assert_eq!(idle.border.width, 0.0);
        assert_eq!(hovered.border.width, 2.0);
        assert_eq!(disabled.text_color, idle.text_color);
        assert_ne!(hovered.text_color, idle.text_color);
        let pressed = quiet_button_style(visual, false, button::Status::Pressed);
        assert_ne!(pressed.background, hovered.background);
    }

    #[test]
    fn shell_controls_respect_live_shape_tokens() {
        let mut visual = canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .visual_theme();
        visual.metrics.radius_sm = 9.0;
        let style = cube_button_style(visual, true, button::Status::Active);
        assert_eq!(style.border.radius, iced::border::Radius::from(9.0));
        assert_eq!(
            quiet_button_style(visual, false, button::Status::Hovered)
                .border
                .radius,
            style.border.radius
        );
    }

    #[test]
    fn compact_apps_use_less_space_than_reading_surfaces() {
        let visual = canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .visual_theme();
        let compact = Density::Compact.insets(visual);
        let comfortable = Density::Comfortable.insets(visual);
        assert!(compact.panel < comfortable.panel);
        assert!(compact.control_x < comfortable.control_x);
        assert!(compact.gap < comfortable.gap);
    }
}
