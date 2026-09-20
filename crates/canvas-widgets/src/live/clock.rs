use super::{CacheState, Key};
use iced::mouse;
use iced::widget::canvas::{self, Geometry, Path};
use iced::{Color, Point, Rectangle, Renderer, Theme};

pub(super) struct AnalogClockProgram {
    pub(super) hour: u64,
    pub(super) minute: u64,
    pub(super) second: f32,
    pub(super) face: Color,
    pub(super) marks: Color,
    pub(super) hands: Color,
    pub(super) accent: Color,
}

impl<Message> canvas::Program<Message> for AnalogClockProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = Key::new()
            .add_u64(self.hour)
            .add_u64(self.minute)
            .add_f32(self.second)
            .add_color(self.face)
            .add_color(self.marks)
            .add_color(self.hands)
            .add_color(self.accent)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
            let radius = bounds.width.min(bounds.height) * 0.39;
            frame.fill(&Path::circle(center, radius), self.face);
            frame.stroke(
                &Path::circle(center, radius),
                canvas::Stroke::default()
                    .with_width(2.5)
                    .with_line_cap(canvas::LineCap::Round)
                    .with_color(self.marks),
            );
            for tick in 0..60 {
                let angle =
                    tick as f32 * std::f32::consts::TAU / 60.0 - std::f32::consts::FRAC_PI_2;
                let major = tick % 5 == 0;
                let outer = Point::new(
                    center.x + angle.cos() * radius * 0.9,
                    center.y + angle.sin() * radius * 0.9,
                );
                let inner_scale = if major { 0.76 } else { 0.84 };
                let inner = Point::new(
                    center.x + angle.cos() * radius * inner_scale,
                    center.y + angle.sin() * radius * inner_scale,
                );
                frame.stroke(
                    &Path::line(inner, outer),
                    canvas::Stroke::default()
                        .with_width(if major { 2.8 } else { 1.35 })
                        .with_line_cap(canvas::LineCap::Round)
                        .with_color(self.marks),
                );
            }
            let minute_fraction = self.minute as f32 + self.second / 60.0;
            let hour_fraction = (self.hour % 12) as f32 + minute_fraction / 60.0;
            for (fraction, divisions, length, width, color) in [
                (hour_fraction, 12.0, 0.48, 4.5, self.hands),
                (minute_fraction, 60.0, 0.69, 3.0, self.hands),
                (self.second, 60.0, 0.76, 1.8, self.accent),
            ] {
                let angle =
                    fraction * std::f32::consts::TAU / divisions - std::f32::consts::FRAC_PI_2;
                let end = Point::new(
                    center.x + angle.cos() * radius * length,
                    center.y + angle.sin() * radius * length,
                );
                frame.stroke(
                    &Path::line(center, end),
                    canvas::Stroke::default()
                        .with_width(width + 1.5)
                        .with_line_cap(canvas::LineCap::Round)
                        .with_color(Color { a: 0.18, ..color }),
                );
                frame.stroke(
                    &Path::line(center, end),
                    canvas::Stroke::default()
                        .with_width(width)
                        .with_line_cap(canvas::LineCap::Round)
                        .with_color(color),
                );
            }
            frame.fill(&Path::circle(center, 4.0), self.accent);
        })
    }
}
