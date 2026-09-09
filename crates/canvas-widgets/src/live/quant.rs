use crate::{
    WidgetSpec,
    quant::{Candle, QuantWidget},
};
use canvas_core::VisualTheme;
use iced::mouse;
use iced::widget::{Canvas, canvas, column, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

pub(super) fn element<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    widget: &'a QuantWidget,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    match widget {
        QuantWidget::Candlestick {
            symbol,
            interval,
            candles,
            show_volume,
        } => {
            let p = visual.palette;
            let m = visual.metrics;
            let last = candles.last().expect("validated candles are non-empty");
            let delta = last.close - last.open;
            let header = row![
                text(symbol).size(m.title).color(rgb(p.text)),
                text(interval).size(m.small).color(rgb(p.text_muted)),
                space::horizontal(),
                text(format!("{:.2}", last.close))
                    .size(m.body)
                    .color(rgb(p.text)),
                text(format!("{delta:+.2}"))
                    .size(m.small)
                    .color(rgb(if delta >= 0.0 { p.success } else { p.danger })),
            ]
            .spacing(m.space_2)
            .align_y(iced::Alignment::Center);
            let chart = Canvas::new(CandlestickProgram {
                candles,
                rising: rgb(p.success),
                falling: rgb(p.danger),
                grid: alpha(rgb(p.border), visual.grid_opacity),
                label: rgb(p.text_muted),
                label_size: m.micro,
                volume: alpha(rgb(p.info), visual.muted_opacity),
                show_volume: *show_volume,
            })
            .width(Length::Fill)
            .height(Length::Fill);
            super::panel(
                spec,
                column![header, chart].spacing(m.space_3).into(),
                visual,
            )
        }
    }
}

struct CandlestickProgram<'a> {
    candles: &'a [Candle],
    rising: Color,
    falling: Color,
    grid: Color,
    label: Color,
    /// Resolved from the theme at construction, like the colours above: a
    /// canvas draws its own text and has no access to the type scale.
    label_size: f32,
    volume: Color,
    show_volume: bool,
}

impl<Message> canvas::Program<Message> for CandlestickProgram<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if bounds.width < 80.0 || bounds.height < 80.0 {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let layout = ChartLayout::for_bounds(bounds.size(), self.show_volume);
        let low = self
            .candles
            .iter()
            .map(|c| c.low)
            .fold(f64::INFINITY, f64::min);
        let high = self
            .candles
            .iter()
            .map(|c| c.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let span = (high - low).max(f64::EPSILON);
        let y = |price: f64| layout.top + ((high - price) / span) as f32 * layout.price_height;

        for index in 0..=4 {
            let fraction = index as f32 / 4.0;
            let line_y = layout.top + fraction * layout.price_height;
            frame.stroke(
                &canvas::Path::line(
                    Point::new(layout.left, line_y),
                    Point::new(layout.left + layout.plot_width, line_y),
                ),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(self.grid),
            );
            let price = high - span * f64::from(fraction);
            frame.fill_text(canvas::Text {
                content: format!("{price:.2}"),
                position: Point::new(
                    layout.left + layout.plot_width + layout.label_gap,
                    line_y - 6.0,
                ),
                color: self.label,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
        }

        let slot = layout.plot_width / self.candles.len() as f32;
        // Scale candle bodies with their allocation. A fixed maximum made a
        // sparse series look lost when a widget snapped to a large tile.
        let body_width = (slot * 0.62).clamp(1.0, 48.0).min(slot.max(1.0));
        let max_volume = self
            .candles
            .iter()
            .map(|c| c.volume)
            .fold(0.0, f64::max)
            .max(f64::EPSILON);
        for (index, candle) in self.candles.iter().enumerate() {
            let x = layout.left + slot * (index as f32 + 0.5);
            let color = if candle.close >= candle.open {
                self.rising
            } else {
                self.falling
            };
            frame.stroke(
                &canvas::Path::line(Point::new(x, y(candle.high)), Point::new(x, y(candle.low))),
                canvas::Stroke::default().with_width(1.0).with_color(color),
            );
            let open_y = y(candle.open);
            let close_y = y(candle.close);
            frame.fill(
                &canvas::Path::rectangle(
                    Point::new(x - body_width / 2.0, open_y.min(close_y)),
                    Size::new(body_width, (open_y - close_y).abs().max(1.5)),
                ),
                color,
            );
            if self.show_volume {
                let height =
                    (candle.volume / max_volume) as f32 * (layout.volume_height - 6.0).max(1.0);
                frame.fill(
                    &canvas::Path::rectangle(
                        Point::new(x - body_width / 2.0, bounds.height - height),
                        Size::new(body_width, height),
                    ),
                    alpha(color, self.volume.a),
                );
            }
        }

        let last = self.candles.last().unwrap().close;
        let last_y = y(last);
        frame.stroke(
            &canvas::Path::line(
                Point::new(layout.left, last_y),
                Point::new(layout.left + layout.plot_width, last_y),
            ),
            canvas::Stroke {
                style: canvas::Style::Solid(self.label),
                width: 1.0,
                line_dash: canvas::LineDash {
                    segments: &[4.0, 4.0],
                    offset: 0,
                },
                ..Default::default()
            },
        );
        frame.fill_text(canvas::Text {
            content: format!("{last:.2}"),
            position: Point::new(
                layout.left + layout.plot_width + layout.label_gap,
                last_y - 6.0,
            ),
            color: self.label,
            size: iced::Pixels(self.label_size),
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChartLayout {
    left: f32,
    top: f32,
    plot_width: f32,
    price_height: f32,
    volume_height: f32,
    label_gap: f32,
}

impl ChartLayout {
    fn for_bounds(bounds: Size, show_volume: bool) -> Self {
        let left = (bounds.width * 0.012).clamp(6.0, 16.0);
        let right = (bounds.width * 0.10).clamp(44.0, 76.0);
        let top = (bounds.height * 0.025).clamp(6.0, 18.0);
        let bottom = (bounds.height * 0.025).clamp(6.0, 18.0);
        let volume_height = if show_volume {
            (bounds.height * 0.22).clamp(24.0, 180.0)
        } else {
            0.0
        };
        let plot_width = (bounds.width - left - right).max(1.0);
        let price_height = (bounds.height - top - bottom - volume_height).max(1.0);
        Self {
            left,
            top,
            plot_width,
            price_height,
            volume_height,
            label_gap: (bounds.width * 0.012).clamp(5.0, 10.0),
        }
    }
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}
fn alpha(color: Color, value: f32) -> Color {
    Color { a: value, ..color }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_layout_reflows_without_preserving_an_intrinsic_aspect_ratio() {
        let wide = ChartLayout::for_bounds(Size::new(1_200.0, 420.0), true);
        let tall = ChartLayout::for_bounds(Size::new(420.0, 900.0), true);
        assert!(wide.plot_width > tall.plot_width);
        assert!(tall.price_height > wide.price_height);
        assert!(wide.volume_height <= 180.0);
        assert!(tall.volume_height <= 180.0);
    }

    #[test]
    fn chart_layout_uses_the_price_area_when_volume_is_hidden() {
        let with_volume = ChartLayout::for_bounds(Size::new(800.0, 500.0), true);
        let without_volume = ChartLayout::for_bounds(Size::new(800.0, 500.0), false);
        assert_eq!(without_volume.volume_height, 0.0);
        assert!(without_volume.price_height > with_volume.price_height);
    }
}
