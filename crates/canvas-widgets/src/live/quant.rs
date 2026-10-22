use crate::{
    WidgetSpec,
    quant::{
        BacktestEvidence, BacktestMarker, BacktestMarkerSide, BacktestRegime, BacktestSeries,
        BacktestSeriesRole, BacktestStatTone, BookLevel, Candle, QuantWidget, TradeSide,
    },
};
use canvas_core::VisualTheme;
use iced::mouse;
use iced::widget::{Canvas, canvas, column, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::time::{SystemTime, UNIX_EPOCH};

use super::signal_scene;
use super::{CacheState, Key};

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
        QuantWidget::OrderBook {
            symbol,
            venue,
            bids,
            asks,
        } => {
            let p = visual.palette;
            let m = visual.metrics;
            let spread = asks[0].price - bids[0].price;
            let header = row![
                text(symbol).size(m.title).color(rgb(p.text)),
                text(venue).size(m.small).color(rgb(p.text_muted)),
                space::horizontal(),
                text(format!("{:.2} × {:.2}", bids[0].price, asks[0].price))
                    .size(m.body)
                    .color(rgb(p.text)),
                text(format!("Δ {spread:.2}"))
                    .size(m.small)
                    .color(rgb(p.text_muted)),
            ]
            .spacing(m.space_2)
            .align_y(iced::Alignment::Center);
            let book = Canvas::new(OrderBookProgram {
                bids,
                asks,
                bid: rgb(p.success),
                ask: rgb(p.danger),
                text: rgb(p.text),
                muted: rgb(p.text_muted),
                label_size: m.micro,
            })
            .width(Length::Fill)
            .height(Length::Fill);
            super::panel(
                spec,
                column![header, book].spacing(m.space_3).into(),
                visual,
            )
        }
        QuantWidget::TimeAndSales {
            symbol,
            venue,
            trades,
        } => {
            let p = visual.palette;
            let m = visual.metrics;
            let header = row![
                text(symbol).size(m.title).color(rgb(p.text)),
                text(venue).size(m.small).color(rgb(p.text_muted)),
                space::horizontal(),
                text("TIME  PRICE  SIZE")
                    .size(m.micro)
                    .color(rgb(p.text_muted)),
            ]
            .spacing(m.space_2);
            let rows =
                trades
                    .iter()
                    .rev()
                    .take(18)
                    .fold(column![].spacing(m.space_1), |rows, trade| {
                        let side = match trade.side {
                            TradeSide::Buy => p.success,
                            TradeSide::Sell => p.danger,
                            TradeSide::Unknown => p.text_muted,
                        };
                        rows.push(
                            row![
                                text(format_trade_time(trade.time))
                                    .size(m.micro)
                                    .color(rgb(p.text_muted))
                                    .width(Length::FillPortion(2)),
                                text(format!("{:.2}", trade.price))
                                    .size(m.small)
                                    .color(rgb(side))
                                    .width(Length::FillPortion(3)),
                                text(format_size(trade.size))
                                    .size(m.small)
                                    .color(rgb(p.text))
                                    .width(Length::FillPortion(2)),
                            ]
                            .spacing(m.space_2),
                        )
                    });
            super::panel(
                spec,
                column![header, rows].spacing(m.space_2).into(),
                visual,
            )
        }
        QuantWidget::MevOpportunity {
            chain_id,
            block_number,
            block_hash,
            token,
            route,
            amount_in,
            amount_after_first,
            amount_out,
            gross_profit,
            gas_cost,
            builder_payment,
            net_profit,
            profitable,
            evaluated_candidates,
        } => {
            let m = visual.metrics;
            let signals = signal_scene::SignalPalette::from_theme(visual);
            Canvas::new(MevOpportunityProgram {
                chain_id: *chain_id,
                block_number: *block_number,
                block_hash,
                token,
                route,
                amount_in: *amount_in,
                amount_after_first: *amount_after_first,
                amount_out: *amount_out,
                gross_profit: *gross_profit,
                gas_cost: *gas_cost,
                builder_payment: *builder_payment,
                net_profit: *net_profit,
                profitable: *profitable,
                evaluated_candidates: *evaluated_candidates,
                signal: if *profitable {
                    signals.positive
                } else {
                    signals.negative
                },
                info: signals.source,
                warning: signals.transform,
                text: signals.text,
                muted: signals.muted,
                grid: signals.grid,
                label_size: m.micro,
                body_size: m.body,
                title_size: m.title,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
        QuantWidget::Backtest {
            title,
            evidence,
            market,
            regimes,
            series,
            trades,
            stats,
        } => {
            let p = visual.palette;
            let m = visual.metrics;
            let badge = match evidence {
                BacktestEvidence::SyntheticProof => "SYNTHETIC PROOF · NOT EXPECTED PERFORMANCE",
                BacktestEvidence::Market => "MARKET EVIDENCE",
            };
            let badge_color = match evidence {
                BacktestEvidence::SyntheticProof => p.warning,
                BacktestEvidence::Market => p.success,
            };
            let stat_rail =
                stats
                    .iter()
                    .take(12)
                    .fold(column![].spacing(m.space_2), |column, stat| {
                        let tone = match stat.tone {
                            BacktestStatTone::Positive => p.success,
                            BacktestStatTone::Negative => p.danger,
                            BacktestStatTone::Neutral => p.text,
                        };
                        column.push(
                            row![
                                text(stat.label.to_uppercase())
                                    .size(m.micro)
                                    .color(rgb(p.text_muted))
                                    .width(Length::Fill),
                                text(&stat.value).size(m.small).color(rgb(tone))
                            ]
                            .align_y(iced::Alignment::Center),
                        )
                    });
            let header = row![
                column![
                    text(title).size(m.title).color(rgb(p.text)),
                    text("PORTFOLIO EQUITY · BENCHMARK COMPARISON · UNDERWATER RISK")
                        .size(m.micro)
                        .color(rgb(p.text_muted)),
                ]
                .spacing(2),
                space::horizontal(),
                text(badge).size(m.micro).color(rgb(badge_color))
            ]
            .align_y(iced::Alignment::Center);
            let chart = Canvas::new(BacktestProgram {
                market,
                regimes,
                series,
                trades,
                strategy: rgb(p.primary),
                benchmark: rgb(p.text_muted),
                comparison: rgb(p.info),
                positive: rgb(p.success),
                negative: rgb(p.danger),
                grid: alpha(rgb(p.border), visual.grid_opacity),
                text: rgb(p.text),
                muted: rgb(p.text_muted),
                surface: rgb(p.surface),
                label_size: m.micro,
            })
            .width(Length::Fill)
            .height(Length::Fill);
            super::panel(
                spec,
                column![
                    header,
                    row![
                        chart.width(Length::FillPortion(4)),
                        column![
                            text("RUN DIAGNOSTICS")
                                .size(m.micro)
                                .color(rgb(p.text_muted)),
                            stat_rail
                        ]
                        .spacing(m.space_3)
                        .width(Length::FillPortion(1))
                    ]
                    .spacing(m.space_4)
                    .height(Length::Fill)
                ]
                .spacing(m.space_3)
                .into(),
                visual,
            )
        }
    }
}

struct MevOpportunityProgram<'a> {
    chain_id: u64,
    block_number: u64,
    block_hash: &'a str,
    token: &'a str,
    route: &'a [String],
    amount_in: f64,
    amount_after_first: f64,
    amount_out: f64,
    gross_profit: f64,
    gas_cost: f64,
    builder_payment: f64,
    net_profit: f64,
    profitable: bool,
    evaluated_candidates: u32,
    signal: Color,
    info: Color,
    warning: Color,
    text: Color,
    muted: Color,
    grid: Color,
    label_size: f32,
    body_size: f32,
    title_size: f32,
}

impl<Message> canvas::Program<Message> for MevOpportunityProgram<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if bounds.width < 640.0 || bounds.height < 400.0 {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f32;
        let phase = signal_scene::phase(now_ms as u128, 4_000);
        let margin = bounds.width * 0.035;
        let top = bounds.height * 0.055;
        let bottom = bounds.height * 0.93;

        // A restrained operator grid gives motion and geometry a stable frame.
        let grid_step = (bounds.width / 24.0).max(48.0);
        let field_origin = Point::new(margin, top);
        let field_size = Size::new(bounds.width - margin * 2.0, bottom - top);
        signal_scene::grid(&mut frame, field_origin, field_size, grid_step, self.grid);

        // A live scan line makes time first-class without obscuring evidence.
        signal_scene::scan_line(&mut frame, field_origin, field_size, phase, self.info);

        draw_text(
            &mut frame,
            "MEV / ROUTE PROCESSOR",
            margin,
            top,
            self.label_size,
            self.info,
        );
        draw_text(
            &mut frame,
            &format!(
                "CHAIN {}  ·  SAFE BLOCK {}",
                self.chain_id, self.block_number
            ),
            margin,
            top + 28.0,
            self.title_size,
            self.text,
        );
        draw_text_right(
            &mut frame,
            if self.profitable {
                "EDGE LOCKED"
            } else {
                "ROUTE REJECTED"
            },
            bounds.width - margin,
            top + 4.0,
            self.body_size,
            self.signal,
        );
        draw_text_right(
            &mut frame,
            &short_hash(self.block_hash),
            bounds.width - margin,
            top + 32.0,
            self.label_size,
            self.muted,
        );

        // Net economics remains the dominant reading, but shares the field
        // with route state rather than floating alone like a presentation KPI.
        let hero_y = bounds.height * 0.24;
        draw_text(
            &mut frame,
            "NET / GAS + BUILDER",
            margin,
            hero_y,
            self.label_size,
            self.muted,
        );
        draw_text(
            &mut frame,
            &format!("{:+.6}", self.net_profit),
            margin,
            hero_y + 64.0,
            (bounds.height * 0.105).clamp(54.0, 126.0),
            self.signal,
        );
        draw_text(
            &mut frame,
            self.token,
            margin + bounds.width * 0.335,
            hero_y + 62.0,
            self.title_size,
            self.muted,
        );

        let route_y = bounds.height * 0.59;
        let node_x = [
            bounds.width * 0.17,
            bounds.width * 0.50,
            bounds.width * 0.83,
        ];
        let node_colors = [self.info, self.warning, self.signal];
        for (index, center_x) in node_x.iter().enumerate() {
            let center = Point::new(*center_x, route_y);
            signal_scene::pulse_node(
                &mut frame,
                center,
                (phase + index as f32 * 0.23) % 1.0,
                node_colors[index],
            );
        }
        for leg in 0..2 {
            let start = node_x[leg] + 44.0;
            let end = node_x[leg + 1] - 44.0;
            signal_scene::flow(
                &mut frame,
                Point::new(start, route_y),
                Point::new(end, route_y),
                (phase + leg as f32 * 0.13) % 1.0,
                node_colors[leg],
            );
        }
        let node_labels = [
            ("CAPITAL", format!("{:.6} {}", self.amount_in, self.token)),
            (
                &self.route[0],
                format!("{:.2} INTERMEDIATE", self.amount_after_first),
            ),
            (
                &self.route[1],
                format!("{:.6} {}", self.amount_out, self.token),
            ),
        ];
        for (index, (label, value)) in node_labels.iter().enumerate() {
            draw_text_center(
                &mut frame,
                &label.to_uppercase(),
                node_x[index],
                route_y + 62.0,
                self.label_size,
                node_colors[index],
            );
            draw_text_center(
                &mut frame,
                value,
                node_x[index],
                route_y + 88.0,
                self.body_size,
                self.text,
            );
        }

        let rail_y = bounds.height * 0.83;
        frame.stroke(
            &canvas::Path::line(
                Point::new(margin, rail_y),
                Point::new(bounds.width - margin, rail_y),
            ),
            canvas::Stroke::default()
                .with_width(1.0)
                .with_color(alpha(self.info, 0.42)),
        );
        let metrics = [
            ("GROSS", format!("{:+.6}", self.gross_profit), self.signal),
            ("GAS", format!("-{:.6}", self.gas_cost), self.warning),
            (
                "BUILDER",
                format!("-{:.6}", self.builder_payment),
                self.muted,
            ),
            (
                "SEARCH",
                format!("{} SIZES", self.evaluated_candidates),
                self.info,
            ),
            (
                "STATUS",
                if self.profitable {
                    "CAPTURE".into()
                } else {
                    "REJECT".into()
                },
                self.signal,
            ),
        ];
        let metric_width = (bounds.width - margin * 2.0) / metrics.len() as f32;
        for (index, (label, value, color)) in metrics.iter().enumerate() {
            let metric_x = margin + index as f32 * metric_width;
            draw_text(
                &mut frame,
                label,
                metric_x,
                rail_y + 30.0,
                self.label_size,
                self.muted,
            );
            draw_text(
                &mut frame,
                value,
                metric_x,
                rail_y + 58.0,
                self.body_size,
                *color,
            );
        }
        draw_text_right(
            &mut frame,
            "RPC PINNED  /  READ ONLY  /  NO SIGNER",
            bounds.width - margin,
            bottom,
            self.label_size,
            self.muted,
        );
        vec![frame.into_geometry()]
    }
}

fn draw_text(frame: &mut canvas::Frame, content: &str, x: f32, y: f32, size: f32, color: Color) {
    frame.fill_text(canvas::Text {
        content: content.into(),
        position: Point::new(x, y),
        color,
        size: iced::Pixels(size),
        ..Default::default()
    });
}

fn draw_text_center(
    frame: &mut canvas::Frame,
    content: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Color,
) {
    frame.fill_text(canvas::Text {
        content: content.into(),
        position: Point::new(x, y),
        color,
        size: iced::Pixels(size),
        align_x: iced::alignment::Horizontal::Center.into(),
        ..Default::default()
    });
}

fn draw_text_right(
    frame: &mut canvas::Frame,
    content: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Color,
) {
    frame.fill_text(canvas::Text {
        content: content.into(),
        position: Point::new(x, y),
        color,
        size: iced::Pixels(size),
        align_x: iced::alignment::Horizontal::Right.into(),
        ..Default::default()
    });
}

fn short_hash(value: &str) -> String {
    if value.len() > 20 {
        format!("{}…{}", &value[..10], &value[value.len() - 8..])
    } else {
        value.into()
    }
}

struct OrderBookProgram<'a> {
    bids: &'a [BookLevel],
    asks: &'a [BookLevel],
    bid: Color,
    ask: Color,
    text: Color,
    muted: Color,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for OrderBookProgram<'_> {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let key = self.bids.iter().chain(self.asks).fold(
            Key::new()
                .add_usize(self.bids.len())
                .add_usize(self.asks.len()),
            |key, level| {
                key.add_f64(level.price)
                    .add_f64(level.size)
                    .add_u64(level.orders as u64)
            },
        );
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let visible = self
                .bids
                .len()
                .min(self.asks.len())
                .min(((bounds.height - 24.0) / 18.0).max(1.0) as usize);
            let max_size = self
                .bids
                .iter()
                .take(visible)
                .chain(self.asks.iter().take(visible))
                .map(|level| level.size)
                .fold(f64::EPSILON, f64::max);
            let center = bounds.width * 0.5;
            frame.fill_text(canvas::Text {
                content: "BID SIZE       PRICE       ASK SIZE".into(),
                position: Point::new(8.0, 12.0),
                color: self.muted,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            for row in 0..visible {
                let y = 26.0 + row as f32 * 18.0;
                let bid = self.bids[row];
                let ask = self.asks[row];
                let bid_width = (bid.size / max_size) as f32 * (center - 12.0);
                let ask_width = (ask.size / max_size) as f32 * (bounds.width - center - 12.0);
                frame.fill(
                    &canvas::Path::rectangle(
                        Point::new(center - bid_width, y - 11.0),
                        Size::new(bid_width, 15.0),
                    ),
                    alpha(self.bid, 0.18),
                );
                frame.fill(
                    &canvas::Path::rectangle(
                        Point::new(center, y - 11.0),
                        Size::new(ask_width, 15.0),
                    ),
                    alpha(self.ask, 0.18),
                );
                frame.fill_text(canvas::Text {
                    content: format_size(bid.size),
                    position: Point::new(8.0, y),
                    color: self.bid,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                frame.fill_text(canvas::Text {
                    content: format!("{:.2}", bid.price),
                    position: Point::new(center - 76.0, y),
                    color: self.text,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                frame.fill_text(canvas::Text {
                    content: format!("{:.2}", ask.price),
                    position: Point::new(center + 8.0, y),
                    color: self.text,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                frame.fill_text(canvas::Text {
                    content: format_size(ask.size),
                    position: Point::new(bounds.width - 62.0, y),
                    color: self.ask,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        })
    }
}

fn format_size(size: f64) -> String {
    if size >= 1_000_000.0 {
        format!("{:.1}m", size / 1_000_000.0)
    } else if size >= 1_000.0 {
        format!("{:.1}k", size / 1_000.0)
    } else if size < 1.0 {
        format!("{size:.4}")
    } else {
        format!("{size:.2}")
    }
}

fn format_trade_time(time: i64) -> String {
    let seconds = time.div_euclid(1_000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

struct BacktestProgram<'a> {
    market: &'a [Candle],
    regimes: &'a [BacktestRegime],
    series: &'a [BacktestSeries],
    trades: &'a [BacktestMarker],
    strategy: Color,
    benchmark: Color,
    comparison: Color,
    positive: Color,
    negative: Color,
    grid: Color,
    text: Color,
    muted: Color,
    surface: Color,
    label_size: f32,
}

impl BacktestProgram<'_> {
    fn color(&self, series: &BacktestSeries, index: usize) -> Color {
        match series.role {
            BacktestSeriesRole::Strategy if index == 0 => self.strategy,
            BacktestSeriesRole::Benchmark => self.benchmark,
            BacktestSeriesRole::Strategy | BacktestSeriesRole::Comparison => {
                [self.comparison, self.positive, self.negative][index % 3]
            }
        }
    }
}

impl<Message> canvas::Program<Message> for BacktestProgram<'_> {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if bounds.width < 180.0 || bounds.height < 140.0 {
            return Vec::new();
        }
        let key = self.series.iter().fold(
            Key::new()
                .add_usize(self.market.len())
                .add_usize(self.regimes.len())
                .add_usize(self.series.len())
                .add_usize(self.trades.len()),
            |key, item| {
                item.points.iter().fold(
                    key.add_str(&item.label).add_u64(item.role as u64),
                    |key, point| key.add_u64(point.time as u64).add_f64(point.value),
                )
            },
        );
        let mut output = state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let layout = BacktestLayout::for_bounds(bounds.size());
            let all = self.series.iter().flat_map(|item| item.points.iter());
            let (min_time, max_time, min_value, max_value) = all.fold(
                (i64::MAX, i64::MIN, f64::INFINITY, f64::NEG_INFINITY),
                |range, point| {
                    (
                        range.0.min(point.time),
                        range.1.max(point.time),
                        range.2.min(point.value),
                        range.3.max(point.value),
                    )
                },
            );
            let value_padding = ((max_value - min_value) * 0.06)
                .max(max_value.abs() * 0.002)
                .max(f64::EPSILON);
            let value_low = min_value - value_padding;
            let value_high = max_value + value_padding;
            let x = |time: i64| {
                layout.left
                    + ((time - min_time) as f64 / (max_time - min_time).max(1) as f64) as f32
                        * layout.width
            };
            let y = |value: f64| {
                layout.equity_top
                    + ((value_high - value) / (value_high - value_low)) as f32
                        * layout.equity_height
            };
            if !self.market.is_empty() {
                let price_low = self
                    .market
                    .iter()
                    .map(|candle| candle.low)
                    .fold(f64::INFINITY, f64::min);
                let price_high = self
                    .market
                    .iter()
                    .map(|candle| candle.high)
                    .fold(f64::NEG_INFINITY, f64::max);
                let price_y = |value: f64| {
                    layout.price_top
                        + ((price_high - value) / (price_high - price_low).max(f64::EPSILON)) as f32
                            * layout.price_height
                };
                for (index, regime) in self.regimes.iter().enumerate() {
                    let start = x(regime.time);
                    let end = self
                        .regimes
                        .get(index + 1)
                        .map_or(layout.left + layout.width, |next| x(next.time));
                    let color = match regime.state {
                        0 => self.negative,
                        1 => self.benchmark,
                        _ => self.positive,
                    };
                    frame.fill(
                        &canvas::Path::rectangle(
                            Point::new(start, layout.price_top),
                            Size::new((end - start).max(1.0), layout.price_height),
                        ),
                        alpha(color, (0.025 + regime.confidence as f32 * 0.075).min(0.10)),
                    );
                }
                let candle_width = (layout.width / self.market.len() as f32 * 0.72).clamp(1.0, 7.0);
                for candle in self.market {
                    let px = x(candle.time);
                    let color = if candle.close >= candle.open {
                        self.positive
                    } else {
                        self.negative
                    };
                    frame.stroke(
                        &canvas::Path::line(
                            Point::new(px, price_y(candle.high)),
                            Point::new(px, price_y(candle.low)),
                        ),
                        canvas::Stroke::default().with_width(1.0).with_color(color),
                    );
                    let body_top = price_y(candle.open.max(candle.close));
                    let body_bottom = price_y(candle.open.min(candle.close));
                    frame.fill(
                        &canvas::Path::rectangle(
                            Point::new(px - candle_width * 0.5, body_top),
                            Size::new(candle_width, (body_bottom - body_top).max(1.0)),
                        ),
                        color,
                    );
                }
                for tick in 0..=2 {
                    let fraction = tick as f32 / 2.0;
                    frame.fill_text(canvas::Text {
                        content: format!(
                            "${:.2}",
                            price_high - (price_high - price_low) * f64::from(fraction)
                        ),
                        position: Point::new(
                            layout.left + layout.width + 8.0,
                            layout.price_top + fraction * layout.price_height - 6.0,
                        ),
                        color: self.muted,
                        size: iced::Pixels(self.label_size),
                        ..Default::default()
                    });
                }
                frame.fill_text(canvas::Text {
                    content: "PRICE / FILLS".into(),
                    position: Point::new(layout.left + 5.0, layout.price_top + 13.0),
                    color: self.muted,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
            for tick in 0..=4 {
                let fraction = tick as f32 / 4.0;
                let line_y = layout.equity_top + fraction * layout.equity_height;
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(layout.left, line_y),
                        Point::new(layout.left + layout.width, line_y),
                    ),
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(self.grid),
                );
                frame.fill_text(canvas::Text {
                    content: compact_money(
                        value_high - (value_high - value_low) * f64::from(fraction),
                    ),
                    position: Point::new(layout.left + layout.width + 8.0, line_y - 6.0),
                    color: self.muted,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
            for tick in 0..=6 {
                let fraction = tick as f32 / 6.0;
                let line_x = layout.left + fraction * layout.width;
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(line_x, layout.price_top),
                        Point::new(line_x, layout.drawdown_bottom),
                    ),
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(self.grid),
                );
                frame.fill_text(canvas::Text {
                    content: format_axis_time(
                        min_time + ((max_time - min_time) as f32 * fraction) as i64,
                    ),
                    position: Point::new(line_x, layout.drawdown_bottom + 5.0),
                    color: self.muted,
                    size: iced::Pixels(self.label_size),
                    align_x: iced::alignment::Horizontal::Center.into(),
                    ..Default::default()
                });
            }
            for (index, item) in self.series.iter().enumerate() {
                let color = self.color(item, index);
                let path = canvas::Path::new(|builder| {
                    if let Some(first) = item.points.first() {
                        builder.move_to(Point::new(x(first.time), y(first.value)));
                    }
                    for point in item.points.iter().skip(1) {
                        builder.line_to(Point::new(x(point.time), y(point.value)));
                    }
                });
                let stroke = canvas::Stroke {
                    style: canvas::Style::Solid(color),
                    width: if item.role == BacktestSeriesRole::Benchmark {
                        1.5
                    } else {
                        2.25
                    },
                    line_dash: if item.role == BacktestSeriesRole::Benchmark {
                        canvas::LineDash {
                            segments: &[5.0, 4.0],
                            offset: 0,
                        }
                    } else {
                        canvas::LineDash::default()
                    },
                    ..Default::default()
                };
                frame.stroke(&path, stroke);
                let legend_x = layout.left + index as f32 * 128.0;
                frame.fill(
                    &canvas::Path::rectangle(Point::new(legend_x, 2.0), Size::new(12.0, 3.0)),
                    color,
                );
                frame.fill_text(canvas::Text {
                    content: item.label.clone(),
                    position: Point::new(legend_x + 17.0, 7.0),
                    color: self.text,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
            if let Some(primary) = self
                .series
                .iter()
                .find(|item| item.role == BacktestSeriesRole::Strategy)
            {
                let mut peak = f64::NEG_INFINITY;
                let drawdowns: Vec<_> = primary
                    .points
                    .iter()
                    .map(|point| {
                        peak = peak.max(point.value);
                        (
                            point.time,
                            if peak > 0.0 {
                                point.value / peak - 1.0
                            } else {
                                0.0
                            },
                        )
                    })
                    .collect();
                let min_drawdown = drawdowns
                    .iter()
                    .map(|point| point.1)
                    .fold(0.0, f64::min)
                    .min(-f64::EPSILON);
                let dd_y = |value: f64| {
                    layout.drawdown_top + (value / min_drawdown) as f32 * layout.drawdown_height
                };
                let area = canvas::Path::new(|builder| {
                    if let Some(first) = drawdowns.first() {
                        builder.move_to(Point::new(x(first.0), layout.drawdown_top));
                    }
                    for point in &drawdowns {
                        builder.line_to(Point::new(x(point.0), dd_y(point.1)));
                    }
                    if let Some(last) = drawdowns.last() {
                        builder.line_to(Point::new(x(last.0), layout.drawdown_top));
                        builder.close();
                    }
                });
                frame.fill(&area, alpha(self.negative, 0.20));
                frame.fill_text(canvas::Text {
                    content: format!("DRAWDOWN  {:.1}%", min_drawdown * 100.0),
                    position: Point::new(layout.left + 5.0, layout.drawdown_top + 14.0),
                    color: self.negative,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
            for marker in self.trades {
                let px = x(marker.time);
                let Some((price_low, price_high)) = (!self.market.is_empty()).then(|| {
                    self.market
                        .iter()
                        .fold((f64::INFINITY, f64::NEG_INFINITY), |range, candle| {
                            (range.0.min(candle.low), range.1.max(candle.high))
                        })
                }) else {
                    continue;
                };
                let py = layout.price_top
                    + ((price_high - marker.value) / (price_high - price_low).max(f64::EPSILON))
                        as f32
                        * layout.price_height;
                let color = if marker.side == BacktestMarkerSide::Entry {
                    self.positive
                } else {
                    self.negative
                };
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(px, layout.price_top),
                        Point::new(px, layout.price_top + layout.price_height),
                    ),
                    canvas::Stroke {
                        style: canvas::Style::Solid(alpha(color, 0.38)),
                        width: 1.0,
                        line_dash: canvas::LineDash {
                            segments: &[3.0, 4.0],
                            offset: 0,
                        },
                        ..Default::default()
                    },
                );
                let triangle = canvas::Path::new(|builder| {
                    let direction = if marker.side == BacktestMarkerSide::Entry {
                        -1.0
                    } else {
                        1.0
                    };
                    builder.move_to(Point::new(px, py));
                    builder.line_to(Point::new(px - 7.0, py + direction * 11.0));
                    builder.line_to(Point::new(px + 7.0, py + direction * 11.0));
                    builder.close();
                });
                frame.fill(&triangle, color);
                let action = if marker.side == BacktestMarkerSide::Entry {
                    "BUY"
                } else {
                    "SELL"
                };
                frame.fill_text(canvas::Text {
                    content: marker
                        .label
                        .as_deref()
                        .map_or_else(|| action.to_string(), |label| format!("{action} · {label}")),
                    position: Point::new(
                        (px + 9.0).min(layout.left + layout.width - 120.0),
                        if marker.side == BacktestMarkerSide::Entry {
                            py + 18.0
                        } else {
                            py - 10.0
                        },
                    ),
                    color,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        });
        let Some(position) = cursor.position_in(bounds) else {
            return output;
        };
        let layout = BacktestLayout::for_bounds(bounds.size());
        if position.x < layout.left
            || position.x > layout.left + layout.width
            || position.y < layout.price_top
            || position.y > layout.drawdown_bottom
        {
            return output;
        }
        let Some(primary) = self
            .series
            .iter()
            .find(|item| item.role == BacktestSeriesRole::Strategy)
        else {
            return output;
        };
        let fraction = ((position.x - layout.left) / layout.width).clamp(0.0, 1.0);
        let index = ((primary.points.len() - 1) as f32 * fraction).round() as usize;
        let point = primary.points[index];
        let mut overlay = canvas::Frame::new(renderer, bounds.size());
        overlay.stroke(
            &canvas::Path::line(
                Point::new(position.x, layout.price_top),
                Point::new(position.x, layout.drawdown_bottom),
            ),
            canvas::Stroke::default()
                .with_width(1.0)
                .with_color(self.muted),
        );
        let tooltip = format!(
            "{}  {}  {}",
            format_axis_time(point.time),
            primary.label,
            compact_money(point.value)
        );
        let width = tooltip.chars().count() as f32 * self.label_size * 0.58 + 18.0;
        let origin = Point::new(
            (position.x + 10.0).min(bounds.width - width - 2.0),
            (position.y - 30.0).max(2.0),
        );
        overlay.fill(
            &canvas::Path::rounded_rectangle(origin, Size::new(width, 23.0), 4.0.into()),
            self.surface,
        );
        overlay.fill_text(canvas::Text {
            content: tooltip,
            position: Point::new(origin.x + 9.0, origin.y + 16.0),
            color: self.text,
            size: iced::Pixels(self.label_size),
            ..Default::default()
        });
        output.push(overlay.into_geometry());
        output
    }
}

#[derive(Clone, Copy)]
struct BacktestLayout {
    left: f32,
    price_top: f32,
    price_height: f32,
    equity_top: f32,
    width: f32,
    equity_height: f32,
    drawdown_top: f32,
    drawdown_height: f32,
    drawdown_bottom: f32,
}
impl BacktestLayout {
    fn for_bounds(bounds: Size) -> Self {
        let left = 10.0;
        let price_top = 18.0;
        let width = (bounds.width - 82.0).max(1.0);
        let usable = (bounds.height - 50.0).max(1.0);
        let price_height = usable * 0.36;
        let equity_top = price_top + price_height + 14.0;
        let equity_height = usable * 0.36;
        let drawdown_top = equity_top + equity_height + 14.0;
        let drawdown_height = (usable * 0.16).max(20.0);
        Self {
            left,
            price_top,
            price_height,
            equity_top,
            width,
            equity_height,
            drawdown_top,
            drawdown_height,
            drawdown_bottom: drawdown_top + drawdown_height,
        }
    }
}

fn compact_money(value: f64) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("${:.2}m", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("${:.1}k", value / 1_000.0)
    } else {
        format!("${value:.2}")
    }
}
fn format_axis_time(time: i64) -> String {
    if time > 10_000_000_000 {
        let seconds = time / 1_000;
        format!(
            "D{} {:02}:{:02}",
            seconds / 86_400,
            seconds / 3_600 % 24,
            seconds / 60 % 60
        )
    } else {
        format!("S{}", time.div_euclid(2))
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
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if bounds.width < 80.0 || bounds.height < 80.0 {
            return Vec::new();
        }
        let key = self.candles.iter().fold(
            Key::new()
                .add_usize(self.candles.len())
                .add_color(self.rising)
                .add_color(self.falling)
                .add_color(self.grid)
                .add_color(self.label)
                .add_color(self.volume)
                .add_f32(self.label_size)
                .add_u64(self.show_volume as u64),
            |key, candle| {
                key.add_u64(candle.time as u64)
                    .add_f64(candle.open)
                    .add_f64(candle.high)
                    .add_f64(candle.low)
                    .add_f64(candle.close)
                    .add_f64(candle.volume)
            },
        );
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
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
                    &canvas::Path::line(
                        Point::new(x, y(candle.high)),
                        Point::new(x, y(candle.low)),
                    ),
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
        })
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
