//! Iced/wGPU paint adapter for renderer-neutral gfx signal geometry.
use super::gfx_theme;
use canvas_core::VisualTheme;
use fpl_gfx::{PrimitiveKind, Rect, Scene, signals::SignalFrame};
use iced::widget::{Canvas, canvas};
use iced::{Element, Length, Point, Rectangle, Renderer, Theme, mouse};

pub(super) fn view<'a, Message: 'a>(
    raw: &serde_json::Value,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let frame: SignalFrame = match serde_json::from_value(raw.clone()) {
        Ok(frame) => frame,
        Err(error) => return iced::widget::text(format!("Signal error: {error}")).into(),
    };
    let mut theme = fpl_gfx::Theme::studio_dark();
    if let Err(error) = gfx_theme::apply(visual, &mut theme) {
        return iced::widget::text(format!("Signal theme error: {error}")).into();
    }
    Canvas::new(Program { frame, theme })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

struct Program {
    frame: SignalFrame,
    theme: fpl_gfx::Theme,
}
impl<Message> canvas::Program<Message> for Program {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut scene = Scene::with_capacity(1200);
        if self
            .frame
            .append(&mut scene, Rect::new(0.0, 0.0, bounds.width, bounds.height))
            .is_err()
        {
            return vec![];
        }
        let mut target = canvas::Frame::new(renderer, bounds.size());
        for p in scene.iter() {
            let c = self.theme.color(p.fill);
            let color = iced::Color::from_linear_rgba(c.r, c.g, c.b, c.a);
            match p.kind {
                PrimitiveKind::RoundedRect => target.fill(
                    &canvas::Path::rounded_rectangle(
                        Point::new(p.rect.x, p.rect.y),
                        iced::Size::new(p.rect.width, p.rect.height),
                        p.radius.into(),
                    ),
                    color,
                ),
                PrimitiveKind::Line => {
                    let center = p.rect.center();
                    let dx = p.rotation.cos() * p.value;
                    let dy = p.rotation.sin() * p.value;
                    target.stroke(
                        &canvas::Path::line(
                            Point::new(center.x - dx, center.y - dy),
                            Point::new(center.x + dx, center.y + dy),
                        ),
                        canvas::Stroke::default()
                            .with_width(p.radius * 2.0)
                            .with_line_cap(canvas::LineCap::Round)
                            .with_color(color),
                    );
                }
                _ => unreachable!("signal builders only emit rectangles and lines"),
            }
        }
        if let Ok(labels) = self
            .frame
            .labels(Rect::new(0.0, 0.0, bounds.width, bounds.height))
        {
            for label in labels {
                let c = self.theme.color(label.role);
                target.fill_text(canvas::Text {
                    content: label.text,
                    position: Point::new(label.bounds.x, label.bounds.y),
                    size: iced::Pixels((label.bounds.height * 0.8).clamp(10.0, 22.0)),
                    color: iced::Color::from_linear_rgba(c.r, c.g, c.b, c.a),
                    ..Default::default()
                });
            }
        }
        vec![target.into_geometry()]
    }
}
