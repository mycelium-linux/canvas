use crate::TransientNotification;
use canvas_core::notification::Level;
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Length, Vector, alignment};
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct NotificationPalette {
    pub panel: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
}

pub(crate) fn view<'a, Message: 'a>(
    items: &'a [TransientNotification],
    lifetime: Duration,
    palette: NotificationPalette,
) -> Element<'a, Message> {
    let mut list = column![].spacing(10).width(Length::Fixed(430.0));
    for item in items.iter().rev() {
        let alpha = opacity(item.shown_at.elapsed(), lifetime);
        let accent = accent(item.notification.level, alpha);
        let title = row![
            container(
                space()
                    .width(Length::Fixed(4.0))
                    .height(Length::Fixed(42.0)),
            )
            .style(move |_| {
                container::Style {
                    background: Some(accent.into()),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }),
            column![
                text(item.notification.title.as_str())
                    .size(17)
                    .color(with_alpha(palette.text, alpha)),
                text(item.notification.message.as_str())
                    .size(13)
                    .color(with_alpha(palette.muted, alpha)),
            ]
            .spacing(4)
            .width(Length::Fill),
        ]
        .spacing(12);

        list = list.push(
            container(title)
                .width(Length::Fill)
                .padding([14, 16])
                .style(move |_| container::Style {
                    background: Some(with_alpha(palette.panel, 0.94 * alpha).into()),
                    border: iced::Border {
                        color: with_alpha(palette.border, alpha),
                        width: 1.0,
                        radius: 9.0.into(),
                    },
                    shadow: iced::Shadow {
                        color: Color::from_rgba8(0, 0, 0, 0.28 * alpha),
                        offset: Vector::new(0.0, 7.0),
                        blur_radius: 20.0,
                    },
                    ..Default::default()
                }),
        );
    }

    container(list)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([56, 24])
        .align_x(alignment::Horizontal::Right)
        .align_y(alignment::Vertical::Top)
        .into()
}

fn opacity(elapsed: Duration, lifetime: Duration) -> f32 {
    const FADE: Duration = Duration::from_secs(2);
    if elapsed >= lifetime {
        return 0.0;
    }
    let remaining = lifetime - elapsed;
    if remaining >= FADE {
        1.0
    } else {
        (remaining.as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0)
    }
}

fn accent(level: Level, alpha: f32) -> Color {
    let color = match level {
        Level::Neutral => Color::from_rgb8(109, 191, 255),
        Level::Good => Color::from_rgb8(78, 220, 151),
        Level::Warning => Color::from_rgb8(255, 190, 72),
        Level::Critical => Color::from_rgb8(255, 86, 104),
    };
    with_alpha(color, alpha)
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha,
        ..color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_holds_then_fades_and_expires() {
        let lifetime = Duration::from_secs(9);
        assert_eq!(opacity(Duration::from_secs(1), lifetime), 1.0);
        assert!((opacity(Duration::from_secs(8), lifetime) - 0.5).abs() < f32::EPSILON);
        assert_eq!(opacity(lifetime, lifetime), 0.0);
    }
}
