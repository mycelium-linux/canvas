//! Shared drawing treatments for native Canvas instruments.
//!
//! Keep the small details which make animated geometry read cleanly here,
//! rather than letting every widget rediscover cap, join, and edge-softening
//! choices independently.

use iced::Color;
use iced::widget::canvas::{self, Path};

/// Stroke a path with rounded geometry and a subtle wider underlay.
///
/// The underlay softens sub-pixel movement without rasterizing at a fixed
/// resolution, so it remains crisp across display scale factors. It is useful
/// for clock hands, traces, needles, and other continuously moving geometry.
pub(super) fn soft_round_stroke(frame: &mut canvas::Frame, path: &Path, width: f32, color: Color) {
    frame.stroke(
        path,
        canvas::Stroke::default()
            .with_width(width + 1.5)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round)
            .with_color(Color { a: 0.18, ..color }),
    );
    frame.stroke(
        path,
        canvas::Stroke::default()
            .with_width(width)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round)
            .with_color(color),
    );
}
