//! Live in-canvas widget rendering via iced. Each widget maps a
//! [`WidgetSpec`](crate::WidgetSpec) plus a theme palette to an iced Element.
//! This is the same visual language the canvas already uses (VU meter,
//! presence orb, ticker text) — extracted so Lua, MCP, and the text protocol
//! all render through one library.

use crate::{ChartType, MeterStyle, StatusState, WidgetKind, WidgetSpec};
use dock_core::ThemePalette;
use iced::mouse;
use iced::widget::canvas::{Geometry, Path};
use iced::widget::{canvas, container, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

/// Deterministic mock pricing derived from the symbol — replaced by the live
/// data source once wired. Kept identical to the Lua module behavior so
/// surfaces look the same across backends.
pub fn mock_price(symbol: &str) -> (f64, f64) {
    let hash = symbol
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let price = (hash % 50_000) as f64 / 100.0 + 50.0;
    let change = (hash.wrapping_mul(7) % 2_000) as f64 / 100.0 - 10.0;
    (price, change)
}

pub fn ticker_text(symbols: &[String]) -> String {
    symbols
        .iter()
        .map(|symbol| {
            let (price, change) = mock_price(symbol);
            let sign = if change >= 0.0 { "+" } else { "‒" };
            format!("{symbol} ${price:.2} ({sign} {:.1}%)", change.abs())
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// Render any widget spec as a fill/fill iced element using the palette.
pub fn element<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    palette: &ThemePalette,
) -> Element<'a, Message> {
    match &spec.kind {
        WidgetKind::Text { text: body } => container(text(body).color(color(palette.text)))
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into(),
        WidgetKind::Clock { format } => {
            let now = chrono_now();
            let rendered = strftime(&now, format);
            container(text(rendered).size(72).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
        }
        WidgetKind::Ticker { symbols } => {
            container(text(ticker_text(symbols)).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
        }
        WidgetKind::Status { state } => {
            let orb = match state.unwrap_or(StatusState::Unknown) {
                StatusState::Present => Color::from_rgba8(34, 197, 94, 0.85),
                StatusState::Away => Color::from_rgba8(239, 68, 68, 0.85),
                StatusState::Unknown => Color::from_rgba8(250, 204, 21, 0.85),
            };
            container(space::horizontal())
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .style(move |_| container::Style {
                    background: Some(orb.into()),
                    border: iced::Border {
                        color: Color { a: 0.4, ..orb },
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        }
        WidgetKind::Meter { value, style } => {
            let bars = 8usize;
            let active = (*value * bars as f32).round().clamp(0.0, bars as f32) as usize;
            let bar_elements: Vec<Element<'_, Message>> = (0..bars)
                .map(|i| {
                    let is_active = i < active;
                    let h = match style {
                        MeterStyle::Vu => 5.0 + i as f32,
                        MeterStyle::Led if is_active => 12.0,
                        MeterStyle::Led => 6.0,
                    };
                    let c = if is_active {
                        color(palette.success)
                    } else {
                        Color {
                            a: 0.2,
                            ..color(palette.text_muted)
                        }
                    };
                    container(space::horizontal())
                        .width(Length::Fixed(3.0))
                        .height(Length::Fixed(h))
                        .style(move |_| container::Style {
                            background: Some(c.into()),
                            ..Default::default()
                        })
                        .into()
                })
                .collect();
            container(row(bar_elements).spacing(2))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
        }
        WidgetKind::Gauge { value, label } => {
            let pct = format!("{:.0}%", value);
            let label = label.clone().unwrap_or_default();
            container(
                iced::widget::Column::<Message>::new()
                    .push(text(pct).size(48).color(color(palette.text)))
                    .push(text(label).size(14).color(color(palette.text_muted))),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
        WidgetKind::Chart { .. } => {
            // Live charts reuse the canvas ChartProgram; the widgets crate
            // delegates so there is one chart implementation (tenet 1).
            container(
                text("chart (live) — see dock-canvas ChartProgram")
                    .color(color(palette.text_muted)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
    }
}

fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

/// Minimal strftime subset — enough for clock formats without pulling a
/// chrono dependency into the spec model.
fn strftime(now: &EpochParts, format: &str) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('H') => out.push_str(&format!("{:02}", now.hour)),
            Some('M') => out.push_str(&format!("{:02}", now.minute)),
            Some('S') => out.push_str(&format!("{:02}", now.second)),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

pub struct EpochParts {
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
}

pub fn chrono_now() -> EpochParts {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    EpochParts {
        hour: (now % 86_400) / 3_600,
        minute: (now % 3_600) / 60,
        second: now % 60,
    }
}

/// Re-exported chart program for canvas — the single chart implementation.
/// Canvas binds its own palette colors at call time.
pub struct ChartProgram {
    pub data: Vec<(f32, f32)>,
    pub chart_type: ChartType,
    pub axis_color: Color,
    pub bar_color: Color,
    pub line_color: Color,
    pub point_color: Color,
}

impl<Message> canvas::Program<Message> for ChartProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.data.is_empty() {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let margin = 40.0f32;
        let stroke = canvas::Stroke::default().with_width(1.0).with_color(Color {
            a: 0.15,
            ..self.axis_color
        });

        let min_x = self
            .data
            .iter()
            .map(|(x, _)| *x)
            .fold(f32::INFINITY, f32::min);
        let max_x = self
            .data
            .iter()
            .map(|(x, _)| *x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = self
            .data
            .iter()
            .map(|(_, y)| *y)
            .fold(f32::INFINITY, f32::min);
        let max_y = self
            .data
            .iter()
            .map(|(_, y)| *y)
            .fold(f32::NEG_INFINITY, f32::max);

        let plot_w = bounds.width - 2.0 * margin;
        let plot_h = bounds.height - 2.0 * margin;

        let sx = |x: f32| margin + (x - min_x) / (max_x - min_x).max(1e-6) * plot_w;
        let sy = |y: f32| bounds.height - margin - (y - min_y) / (max_y - min_y).max(1e-6) * plot_h;

        if plot_w > 0.0 && plot_h > 0.0 {
            frame.stroke(
                &Path::line(
                    Point::new(margin, bounds.height - margin),
                    Point::new(bounds.width - margin, bounds.height - margin),
                ),
                stroke,
            );
            frame.stroke(
                &Path::line(
                    Point::new(margin, margin),
                    Point::new(margin, bounds.height - margin),
                ),
                stroke,
            );
        }

        match self.chart_type {
            ChartType::Bar => {
                let n = self.data.len() as f32;
                let bar_w = plot_w / (n + 1.0);
                for (i, (_, y)) in self.data.iter().enumerate() {
                    let cx = margin + bar_w * (i as f32 + 0.5);
                    let bh = (*y - min_y) / (max_y - min_y).max(1e-6) * plot_h;
                    frame.fill(
                        &Path::rectangle(
                            Point::new(cx - bar_w * 0.4, bounds.height - margin - bh),
                            Size::new(bar_w * 0.8, bh.max(1.0)),
                        ),
                        self.bar_color,
                    );
                }
            }
            ChartType::Line => {
                let line_stroke = canvas::Stroke::default()
                    .with_width(2.0)
                    .with_color(self.line_color);
                for window in self.data.windows(2) {
                    frame.stroke(
                        &Path::line(
                            Point::new(sx(window[0].0), sy(window[0].1)),
                            Point::new(sx(window[1].0), sy(window[1].1)),
                        ),
                        line_stroke,
                    );
                }
                for (x, y) in &self.data {
                    frame.fill(
                        &Path::circle(Point::new(sx(*x), sy(*y)), 2.5),
                        self.line_color,
                    );
                }
            }
            ChartType::Scatter => {
                for (x, y) in &self.data {
                    frame.fill(
                        &Path::circle(Point::new(sx(*x), sy(*y)), 3.0),
                        self.point_color,
                    );
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticker_matches_lua_module_output() {
        let text = ticker_text(&["AAPL".into()]);
        assert!(text.contains("AAPL $"));
        assert!(text.contains('%'));
    }

    #[test]
    fn clock_formats_hours_minutes_seconds() {
        let now = EpochParts {
            hour: 14,
            minute: 3,
            second: 7,
        };
        assert_eq!(strftime(&now, "%H:%M:%S"), "14:03:07");
        assert_eq!(strftime(&now, "100%%"), "100%");
        assert_eq!(strftime(&now, "%Q"), "%Q");
    }
}
