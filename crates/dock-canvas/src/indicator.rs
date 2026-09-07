use dock_canvas::control::{IndicatorState, StatusIndicator};
use iced::mouse;
use iced::widget::{canvas, column, container, text};
use iced::{Color, Element, Length, Point, Radians, Rectangle, Renderer, Theme, Vector, alignment};
use std::f32::consts::{PI, TAU};
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct IndicatorPalette {
    pub panel: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
}

pub(crate) fn view<'a, Message: 'a>(
    indicator: &'a StatusIndicator,
    elapsed: Duration,
    palette: IndicatorPalette,
) -> Element<'a, Message> {
    let caption = indicator
        .caption
        .as_deref()
        .unwrap_or(indicator.state.label());
    let reactor = canvas(Reactor {
        state: indicator.state,
        elapsed,
    })
    .width(Length::Fixed(176.0))
    .height(Length::Fixed(176.0));

    let mut content = column![
        reactor,
        text(indicator.title.as_str()).size(23).color(palette.text),
        text(caption).size(15).color(palette.muted),
    ]
    .spacing(8)
    .align_x(iced::Alignment::Center);
    if let Some(detail) = indicator.detail.as_deref() {
        content = content.push(
            text(detail)
                .size(14)
                .color(palette.text)
                .width(Length::Fill),
        );
    }
    if let Some(diagnostic) = indicator.diagnostic.as_deref() {
        content = content.push(
            text(diagnostic)
                .font(iced::Font::MONOSPACE)
                .size(11)
                .color(palette.muted)
                .width(Length::Fill),
        );
    }

    let panel = container(content)
        .width(Length::Fill)
        .max_width(440)
        .padding([22, 28])
        .align_x(alignment::Horizontal::Center)
        .style(move |_| container::Style {
            background: Some(palette.panel.into()),
            border: iced::Border {
                color: palette.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.32),
                offset: Vector::new(0.0, 10.0),
                blur_radius: 28.0,
            },
            ..Default::default()
        });

    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .center(Length::Fill)
        .into()
}

#[derive(Debug, Clone, Copy)]
struct Reactor {
    state: IndicatorState,
    elapsed: Duration,
}

impl<Message> canvas::Program<Message> for Reactor {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = frame.center();
        let radius = bounds.width.min(bounds.height) * 0.39;
        let motion = self.state.motion();
        let elapsed = self.elapsed.as_secs_f32();
        let phase = elapsed * motion.speed;
        let pulse = ((elapsed * motion.pulse_hz * TAU).sin() + 1.0) * 0.5;
        let accent = self.state.accent();

        for (scale, alpha) in [(1.18, 0.035), (1.06, 0.065), (0.92, 0.1)] {
            frame.fill(
                &canvas::Path::circle(center, radius * scale * (1.0 + pulse * 0.025)),
                with_alpha(accent, alpha),
            );
        }

        frame.stroke(
            &canvas::Path::circle(center, radius),
            canvas::Stroke::default()
                .with_color(with_alpha(accent, 0.25))
                .with_width(1.5),
        );
        frame.stroke(
            &canvas::Path::circle(center, radius * 0.72),
            canvas::Stroke::default()
                .with_color(with_alpha(accent, 0.48))
                .with_width(2.0),
        );

        for index in 0..24 {
            let angle = index as f32 * TAU / 24.0 - phase * 0.16;
            let emphasis = if index % 3 == 0 { 0.68 } else { 0.26 };
            let inner = polar(center, radius * 0.82, angle);
            let outer = polar(
                center,
                radius * if index % 3 == 0 { 0.98 } else { 0.93 },
                angle,
            );
            frame.stroke(
                &canvas::Path::line(inner, outer),
                canvas::Stroke::default()
                    .with_color(with_alpha(accent, emphasis))
                    .with_width(if index % 3 == 0 { 2.2 } else { 1.0 })
                    .with_line_cap(canvas::LineCap::Round),
            );
        }

        for (ring, direction, offset) in [(1.0, 1.0, 0.0), (0.78, -1.0, PI * 0.6)] {
            let start = phase * direction + offset;
            for segment in 0..3 {
                let segment_start = start + segment as f32 * TAU / 3.0;
                let arc = canvas::Path::new(|path| {
                    path.arc(canvas::path::Arc {
                        center,
                        radius: radius * ring,
                        start_angle: Radians(segment_start),
                        end_angle: Radians(segment_start + PI * 0.31),
                    });
                });
                frame.stroke(
                    &arc,
                    canvas::Stroke::default()
                        .with_color(with_alpha(accent, if ring == 1.0 { 0.92 } else { 0.62 }))
                        .with_width(if ring == 1.0 { 3.2 } else { 2.0 })
                        .with_line_cap(canvas::LineCap::Round),
                );
            }
        }

        let core_radius = radius * (0.4 + pulse * motion.pulse_scale);
        frame.fill(
            &canvas::Path::circle(center, core_radius),
            with_alpha(accent, 0.13 + pulse * 0.08),
        );
        frame.stroke(
            &canvas::Path::circle(center, core_radius),
            canvas::Stroke::default()
                .with_color(with_alpha(accent, 0.9))
                .with_width(2.5),
        );
        frame.fill(
            &canvas::Path::circle(center, radius * 0.1),
            with_alpha(Color::WHITE, 0.72 + pulse * 0.2),
        );

        vec![frame.into_geometry()]
    }
}

#[derive(Debug, Clone, Copy)]
struct Motion {
    speed: f32,
    pulse_hz: f32,
    pulse_scale: f32,
}

trait IndicatorStateVisuals {
    fn accent(self) -> Color;
    fn motion(self) -> Motion;
    fn label(self) -> &'static str;
}

impl IndicatorStateVisuals for IndicatorState {
    fn accent(self) -> Color {
        match self {
            Self::Ready => Color::from_rgb8(79, 209, 197),
            Self::Transcribing => Color::from_rgb8(109, 174, 255),
            Self::Listening => Color::from_rgb8(77, 218, 255),
            Self::Thinking => Color::from_rgb8(255, 190, 84),
            Self::Speaking => Color::from_rgb8(126, 231, 135),
        }
    }

    fn motion(self) -> Motion {
        match self {
            Self::Ready => Motion {
                speed: 0.35,
                pulse_hz: 0.35,
                pulse_scale: 0.015,
            },
            Self::Transcribing => Motion {
                speed: 1.15,
                pulse_hz: 1.1,
                pulse_scale: 0.035,
            },
            Self::Listening => Motion {
                speed: 0.8,
                pulse_hz: 1.65,
                pulse_scale: 0.05,
            },
            Self::Thinking => Motion {
                speed: 1.8,
                pulse_hz: 0.8,
                pulse_scale: 0.025,
            },
            Self::Speaking => Motion {
                speed: 1.05,
                pulse_hz: 2.4,
                pulse_scale: 0.075,
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Transcribing => "Transcribing",
            Self::Listening => "Listening",
            Self::Thinking => "Thinking",
            Self::Speaking => "Speaking",
        }
    }
}

fn polar(center: Point, radius: f32, angle: f32) -> Point {
    Point::new(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}
