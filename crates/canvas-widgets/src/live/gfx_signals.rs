//! Iced/wGPU paint adapter for renderer-neutral gfx signal geometry.
use super::{CacheState, Key, gfx_theme};
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
    let fingerprint = match cache_fingerprint(&frame, &theme) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            return iced::widget::text(format!("Signal fingerprint error: {error}")).into();
        }
    };
    Canvas::new(Program {
        frame,
        theme,
        fingerprint,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
fn cache_fingerprint(
    frame: &SignalFrame,
    theme: &fpl_gfx::Theme,
) -> Result<u64, serde_json::Error> {
    Ok(Key::new()
        .add_str(&serde_json::to_string(&(frame, theme))?)
        .build())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_geometry_invalidates_only_when_frame_or_resolved_theme_changes() {
        let frame: SignalFrame = serde_json::from_str(
            r#"{"view":"scope","range":{"minimum":0,"maximum":5},"samples":[0,3,0]}"#,
        )
        .unwrap();
        let theme = fpl_gfx::Theme::studio_dark();
        let key = cache_fingerprint(&frame, &theme).unwrap();
        assert_eq!(
            key,
            cache_fingerprint(&frame.clone(), &theme.clone()).unwrap()
        );
        let changed: SignalFrame = serde_json::from_str(
            r#"{"view":"scope","range":{"minimum":0,"maximum":5},"samples":[0,4,0]}"#,
        )
        .unwrap();
        assert_ne!(key, cache_fingerprint(&changed, &theme).unwrap());
        let mut recolored = theme;
        recolored.colors[0].r = 0.123;
        assert_ne!(key, cache_fingerprint(&frame, &recolored).unwrap());
    }
}

struct Program {
    frame: SignalFrame,
    theme: fpl_gfx::Theme,
    fingerprint: u64,
}
impl Program {
    fn paint_error(&self, target: &mut canvas::Frame, message: String) {
        let c = self.theme.color(fpl_gfx::ThemeRole::Danger);
        target.fill_text(canvas::Text {
            content: message,
            position: Point::new(8., 8.),
            size: iced::Pixels(16.),
            color: iced::Color::from_linear_rgba(c.r, c.g, c.b, c.a),
            ..Default::default()
        });
    }
}
impl<Message> canvas::Program<Message> for Program {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        state.draw_cached(renderer, bounds.size(), self.fingerprint, |target| {
            let mut scene = Scene::with_capacity(1200);
            if let Err(error) = self
                .frame
                .append(&mut scene, Rect::new(0.0, 0.0, bounds.width, bounds.height))
            {
                self.paint_error(target, format!("Signal geometry error: {error}"));
                return;
            }
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
            match self
                .frame
                .labels(Rect::new(0.0, 0.0, bounds.width, bounds.height))
            {
                Ok(labels) => {
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
                Err(error) => self.paint_error(target, format!("Signal labels error: {error}")),
            }
        })
    }
}
