use crate::quant::{Candle, QuantWidget};
use dock_core::VisualTheme;
use iced::mouse;
use iced::widget::{Canvas, canvas, column, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

pub(super) fn element<'a, Message: 'a>(
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
                text(format!(
                    "O {:.2}  H {:.2}  L {:.2}  C {:.2}",
                    last.open, last.high, last.low, last.close
                ))
                .size(m.small)
                .color(rgb(p.text_muted)),
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
                volume: alpha(rgb(p.info), visual.muted_opacity),
                show_volume: *show_volume,
            })
            .width(Length::Fill)
            .height(Length::Fill);
            super::panel(column![header, chart].spacing(m.space_3).into(), visual)
        }
    }
}

struct CandlestickProgram<'a> {
    candles: &'a [Candle],
    rising: Color,
    falling: Color,
    grid: Color,
    label: Color,
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
        let left = 8.0;
        let right = 58.0;
        let top = 8.0;
        let volume_height = if self.show_volume {
            bounds.height * 0.20
        } else {
            0.0
        };
        let price_bottom = bounds.height - volume_height - 12.0;
        let plot_width = (bounds.width - left - right).max(1.0);
        let price_height = (price_bottom - top).max(1.0);
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
        let y = |price: f64| top + ((high - price) / span) as f32 * price_height;

        for index in 0..=4 {
            let fraction = index as f32 / 4.0;
            let line_y = top + fraction * price_height;
            frame.stroke(
                &canvas::Path::line(
                    Point::new(left, line_y),
                    Point::new(left + plot_width, line_y),
                ),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(self.grid),
            );
            let price = high - span * f64::from(fraction);
            frame.fill_text(canvas::Text {
                content: format!("{price:.2}"),
                position: Point::new(left + plot_width + 6.0, line_y - 6.0),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
        }

        let slot = plot_width / self.candles.len() as f32;
        let body_width = (slot * 0.62).clamp(1.0, 14.0);
        let max_volume = self
            .candles
            .iter()
            .map(|c| c.volume)
            .fold(0.0, f64::max)
            .max(f64::EPSILON);
        for (index, candle) in self.candles.iter().enumerate() {
            let x = left + slot * (index as f32 + 0.5);
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
                let height = (candle.volume / max_volume) as f32 * (volume_height - 6.0).max(1.0);
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
                Point::new(left, last_y),
                Point::new(left + plot_width, last_y),
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
            position: Point::new(left + plot_width + 6.0, last_y - 6.0),
            color: self.label,
            size: iced::Pixels(10.0),
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}
fn alpha(color: Color, value: f32) -> Color {
    Color { a: value, ..color }
}
