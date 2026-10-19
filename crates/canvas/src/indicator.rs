use ::canvas::control::{IndicatorState, StatusIndicator};
use fpl_gfx::{
    reactor::{self, Activity, Mark, Paint},
    voice_popup as popup,
};
use iced::mouse;
use iced::widget::canvas::{Frame, Geometry, Path, Stroke};
use iced::widget::{canvas, container};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, alignment};
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
    reveal: f32,
    palette: IndicatorPalette,
) -> Element<'a, Message> {
    // Kept in the owner API for source compatibility; voice chrome is shared
    // with the native popup and intentionally independent of workspace theme.
    let _ = (palette.panel, palette.border, palette.text, palette.muted);
    let pill = canvas(Reactor {
        state: indicator.state,
        elapsed,
        caption: indicator.caption.as_deref(),
        reveal,
    })
    .width(Length::Fixed(popup::SIZE[0]))
    .height(Length::Fixed(popup::SIZE[1] + popup::SLIDE));
    // Global, noninteractive presentation: workspace selection remains intact.
    container(pill)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Bottom)
        .into()
}

#[derive(Debug, Clone, Copy)]
struct Reactor<'a> {
    state: IndicatorState,
    elapsed: Duration,
    caption: Option<&'a str>,
    reveal: f32,
}
impl<Message> canvas::Program<Message> for Reactor<'_> {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let opacity = popup::opacity(self.reveal);
        frame.translate(iced::Vector::new(0.0, popup::SLIDE * (1.0 - opacity)));
        let ink = |mut color: Color| {
            color.a *= opacity;
            color
        };
        let capsule = Path::rounded_rectangle(
            Point::ORIGIN,
            Size::new(popup::SIZE[0], popup::SIZE[1]),
            popup::RADIUS.into(),
        );
        frame.fill(&capsule, ink(rgba(popup::SURFACE)));
        frame.stroke(
            &capsule,
            Stroke::default()
                .with_width(1.0)
                .with_color(ink(rgba(popup::BORDER))),
        );
        let activity = match self.state {
            IndicatorState::Thinking => Activity::Thinking,
            IndicatorState::Speaking => Activity::Speaking,
            IndicatorState::Listening | IndicatorState::Transcribing => Activity::Listening,
            IndicatorState::Ready => Activity::Resting,
        };
        frame.fill_text(canvas::Text {
            content: popup::label(activity, self.caption),
            position: Point::new(popup::TEXT_RECT[0], popup::SIZE[1] * 0.5),
            color: ink(rgba(popup::TEXT)),
            size: popup::FONT_SIZE.into(),
            align_y: alignment::Vertical::Center,
            ..Default::default()
        });
        let scale = popup::REACTOR_WIDTH / reactor::SIZE[0];
        frame.translate(iced::Vector::new(
            popup::REACTOR_CENTER[0] - reactor::CENTER[0] * scale,
            popup::REACTOR_CENTER[1] - reactor::CENTER[1] * scale,
        ));
        frame.scale(scale);
        let center = Point::new(reactor::CENTER[0], reactor::CENTER[1]);
        let [r, g, b] = popup::accent(activity);
        let base = Color::from_rgb(r, g, b);
        let [ir, ig, ib] = popup::INK;
        reactor::visit(activity, self.elapsed.as_secs_f32(), |mark| match mark {
            Mark::Disc {
                radius,
                paint,
                alpha,
            } => {
                let color = match paint {
                    Paint::Surface => rgba(popup::SURFACE),
                    Paint::Accent => base,
                    Paint::Ink => Color::from_rgb(ir, ig, ib),
                };
                frame.fill(
                    &Path::circle(center, radius),
                    ink(Color { a: alpha, ..color }),
                );
            }
            Mark::Ring {
                radius,
                width,
                alpha,
            } => frame.stroke(
                &Path::circle(center, radius),
                Stroke::default()
                    .with_width(width)
                    .with_color(ink(Color { a: alpha, ..base })),
            ),
            Mark::Arc {
                radius,
                start,
                end,
                width,
                alpha,
            } => {
                let path = Path::new(|builder| {
                    builder.arc(canvas::path::Arc {
                        center,
                        radius,
                        start_angle: iced::Radians(start),
                        end_angle: iced::Radians(end),
                    })
                });
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_width(width)
                        .with_color(ink(Color { a: alpha, ..base })),
                );
            }
            Mark::Bar {
                x,
                height,
                width,
                alpha,
            } => frame.fill_rectangle(
                Point::new(center.x + x, center.y - height / 2.0),
                Size::new(width, height),
                ink(Color::from_rgba(ir, ig, ib, alpha)),
            ),
        });
        vec![frame.into_geometry()]
    }
}

fn rgba([r, g, b, a]: [f32; 4]) -> Color {
    Color::from_rgba(r, g, b, a)
}
