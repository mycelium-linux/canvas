//! Domain-neutral, data-reactive scene primitives.
//!
//! These functions know about geometry, time, and semantic color roles. They
//! deliberately know nothing about MEV, networks, cameras, or model inference.

use canvas_core::VisualTheme;
use iced::widget::canvas;
use iced::{Color, Point, Size};

#[derive(Clone, Copy)]
pub(super) struct SignalPalette {
    pub source: Color,
    pub transform: Color,
    pub positive: Color,
    pub negative: Color,
    pub text: Color,
    pub muted: Color,
    pub grid: Color,
}

impl SignalPalette {
    pub fn from_theme(theme: &VisualTheme) -> Self {
        let p = theme.palette;
        Self {
            source: rgb(p.info),
            transform: rgb(p.warning),
            positive: rgb(p.success),
            negative: rgb(p.danger),
            text: rgb(p.text),
            muted: rgb(p.text_muted),
            grid: alpha(rgb(p.border), theme.grid_opacity * 0.45),
        }
    }
}

pub(super) fn phase(unix_ms: u128, period_ms: u128) -> f32 {
    if period_ms == 0 {
        0.0
    } else {
        (unix_ms % period_ms) as f32 / period_ms as f32
    }
}

pub(super) fn grid(frame: &mut canvas::Frame, origin: Point, size: Size, step: f32, color: Color) {
    let mut x = origin.x;
    while x <= origin.x + size.width {
        frame.stroke(
            &canvas::Path::line(
                Point::new(x, origin.y),
                Point::new(x, origin.y + size.height),
            ),
            canvas::Stroke::default().with_width(1.0).with_color(color),
        );
        x += step;
    }
    let mut y = origin.y;
    while y <= origin.y + size.height {
        frame.stroke(
            &canvas::Path::line(
                Point::new(origin.x, y),
                Point::new(origin.x + size.width, y),
            ),
            canvas::Stroke::default().with_width(1.0).with_color(color),
        );
        y += step;
    }
}

pub(super) fn scan_line(
    frame: &mut canvas::Frame,
    origin: Point,
    size: Size,
    phase: f32,
    color: Color,
) {
    let x = origin.x + phase.clamp(0.0, 1.0) * size.width;
    frame.fill(
        &canvas::Path::rectangle(Point::new(x, origin.y), Size::new(2.0, size.height)),
        alpha(color, 0.18),
    );
}

pub(super) fn pulse_node(frame: &mut canvas::Frame, center: Point, phase: f32, color: Color) {
    let pulse = (phase * core::f32::consts::TAU).sin().abs();
    for ring in (1..=3).rev() {
        frame.stroke(
            &canvas::Path::circle(center, 25.0 + ring as f32 * 10.0 + pulse * 5.0),
            canvas::Stroke::default()
                .with_width(1.0)
                .with_color(alpha(color, 0.08 * (4 - ring) as f32)),
        );
    }
    frame.fill(&canvas::Path::circle(center, 9.0), color);
}

pub(super) fn flow(frame: &mut canvas::Frame, start: Point, end: Point, phase: f32, color: Color) {
    frame.stroke(
        &canvas::Path::line(start, end),
        canvas::Stroke::default()
            .with_width(2.0)
            .with_color(alpha(color, 0.55)),
    );
    for particle in 0..5 {
        let t = (phase * 1.7 + particle as f32 / 5.0) % 1.0;
        let point = Point::new(
            start.x + (end.x - start.x) * t,
            start.y + (end.y - start.y) * t,
        );
        frame.fill(&canvas::Path::circle(point, 2.5 + 2.0 * (1.0 - t)), color);
    }
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}
fn alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_is_bounded_and_repeats() {
        assert_eq!(phase(0, 4_000), 0.0);
        assert_eq!(phase(4_000, 4_000), 0.0);
        assert!((phase(1_000, 4_000) - 0.25).abs() < f32::EPSILON);
    }
}
