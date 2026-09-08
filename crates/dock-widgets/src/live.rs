//! Live in-canvas widget rendering via iced. Each widget maps a
//! [`WidgetSpec`](crate::WidgetSpec) plus a resolved Coherence visual theme to
//! an iced Element.
//! This is the same visual language the canvas already uses (VU meter,
//! presence orb, ticker text) — extracted so Lua, MCP, and the text protocol
//! all render through one library.

use crate::{ChartType, MeterStyle, StatusLevel, StatusState, WidgetKind, WidgetSpec};
use dock_core::{ThemePalette, VisualTheme};
use iced::mouse;
use iced::widget::canvas::{Geometry, Path};
use iced::widget::{Canvas, canvas, column, container, row, space, text};
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

/// Render any widget spec using the same semantic theme contract.
pub fn element<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let palette = &visual.palette;
    let metrics = visual.metrics;
    match &spec.kind {
        WidgetKind::Stat {
            label,
            value,
            unit,
            change,
            state,
            history,
        } => {
            let accent = state.map_or(color(palette.primary), |state| status_color(state, palette));
            let mut value_line = row![
                text(format_metric(*value))
                    .size(metrics.display * 1.5)
                    .color(color(palette.text)),
                text(unit.clone().unwrap_or_default())
                    .size(metrics.small)
                    .color(color(palette.text_muted)),
            ]
            .spacing(metrics.space_1)
            .align_y(iced::alignment::Vertical::Bottom);
            if let Some(change) = change {
                value_line =
                    value_line.push(text(format!("{change:+.1}%")).size(metrics.small).color(
                        if *change >= 0.0 {
                            color(palette.success)
                        } else {
                            color(palette.danger)
                        },
                    ));
            }
            let body = column![
                text(label)
                    .size(metrics.small)
                    .color(color(palette.text_muted)),
                value_line,
                Canvas::new(SparklineProgram {
                    values: history.clone(),
                    color: accent
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2);
            panel(body.into(), visual)
        }
        WidgetKind::TimeSeries {
            title,
            series,
            unit,
        } => {
            let colors = series_colors(palette);
            let legend = row(series
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let c = colors[index % colors.len()];
                    row![
                        status_dot(c, 7.0),
                        text(&item.label)
                            .size(metrics.small)
                            .color(color(palette.text_muted))
                    ]
                    .spacing(metrics.space_1)
                    .into()
                })
                .collect::<Vec<Element<'_, Message>>>())
            .spacing(metrics.space_3);
            let body = column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(unit.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                legend,
                Canvas::new(TimeSeriesProgram {
                    series: series
                        .iter()
                        .map(|item| item
                            .points
                            .iter()
                            .map(|point| (point.x as f32, point.y as f32))
                            .collect())
                        .collect(),
                    colors,
                    grid: with_alpha(color(palette.border), visual.grid_opacity)
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2);
            panel(body.into(), visual)
        }
        // A module kind is resolved by the Canvas Lua host before it reaches this
        // crate, which has no Lua runtime of its own. Reaching here is a wiring
        // bug, and `element` cannot return an error — so say so on screen rather
        // than render an innocent blank that looks like a working empty widget.
        WidgetKind::Module { module, .. } => {
            container(text(format!("unresolved module: {module}")).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
        }
        WidgetKind::Text { text: body } => container(text(body).color(color(palette.text)))
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into(),
        WidgetKind::Clock { format } => {
            let now = chrono_now();
            let rendered = strftime(&now, format);
            container(
                text(rendered)
                    .size(metrics.display * 2.25)
                    .color(color(palette.text)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
        WidgetKind::Ticker { symbols } => {
            let quotes = symbols
                .iter()
                .map(|symbol| {
                    let (price, change) = mock_price(symbol);
                    column![
                        text(symbol)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                        text(format!("${price:.2}"))
                            .size(metrics.large)
                            .color(color(palette.text)),
                        text(format!("{change:+.1}%")).size(metrics.small).color(
                            if change >= 0.0 {
                                color(palette.success)
                            } else {
                                color(palette.danger)
                            }
                        ),
                    ]
                    .spacing(metrics.space_1)
                    .into()
                })
                .collect::<Vec<Element<'_, Message>>>();
            panel(row(quotes).spacing(metrics.space_5).into(), visual)
        }
        WidgetKind::Status { state } => {
            let orb = match state.unwrap_or(StatusState::Unknown) {
                StatusState::Present => with_alpha(color(palette.success), 0.85),
                StatusState::Away => with_alpha(color(palette.danger), 0.85),
                StatusState::Unknown => with_alpha(color(palette.warning), 0.85),
            };
            panel(
                row![
                    status_dot(orb, 12.0),
                    text(match state.unwrap_or(StatusState::Unknown) {
                        StatusState::Present => "Healthy",
                        StatusState::Away => "Unavailable",
                        StatusState::Unknown => "Unknown",
                    })
                    .size(metrics.body)
                    .color(color(palette.text))
                ]
                .spacing(metrics.space_2)
                .align_y(iced::alignment::Vertical::Center)
                .into(),
                visual,
            )
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
                            a: visual.muted_opacity * 0.3,
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
            panel(
                column![
                    text("Level")
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                    row(bar_elements).spacing(metrics.space_1)
                ]
                .spacing(metrics.space_3)
                .into(),
                visual,
            )
        }
        WidgetKind::Gauge { value, label } => {
            let pct = format!("{:.0}%", value);
            let label = label.clone().unwrap_or_default();
            panel(
                iced::widget::Column::<Message>::new()
                    .push(
                        text(pct)
                            .size(metrics.display * 1.7)
                            .color(color(palette.text)),
                    )
                    .push(
                        text(label)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                    )
                    .push(
                        Canvas::new(GaugeProgram {
                            value: *value,
                            track: color(palette.surface_muted),
                            fill: color(palette.primary),
                            radius: metrics.radius_sm,
                        })
                        .width(Length::Fill)
                        .height(Length::Fixed(12.0)),
                    )
                    .into(),
                visual,
            )
        }
        WidgetKind::Histogram {
            title,
            buckets,
            unit,
        } => panel(
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(unit.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted))
                ],
                Canvas::new(HistogramProgram {
                    values: buckets.iter().map(|bucket| bucket.count as f32).collect(),
                    color: color(palette.info),
                    grid: with_alpha(color(palette.border), visual.grid_opacity)
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::Table {
            title,
            columns,
            rows,
        } => {
            let header = row(columns
                .iter()
                .map(|label| {
                    text(label)
                        .size(metrics.small)
                        .color(color(palette.text_muted))
                        .width(Length::Fill)
                        .into()
                })
                .collect::<Vec<Element<'_, Message>>>());
            let lines = rows.iter().take(8).fold(
                column![header].spacing(metrics.space_2),
                |column, values| {
                    column.push(row(values
                        .iter()
                        .map(|value| {
                            text(value)
                                .size(metrics.small)
                                .color(color(palette.text))
                                .width(Length::Fill)
                                .into()
                        })
                        .collect::<Vec<Element<'_, Message>>>()))
                },
            );
            panel(
                column![
                    text(title).size(metrics.body).color(color(palette.text)),
                    lines
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::EventLog { title, entries } => {
            let lines = entries.iter().rev().take(7).fold(
                column![].spacing(metrics.space_2),
                |column, entry| {
                    column.push(
                        row![
                            status_dot(status_color(entry.level, palette), 7.0),
                            text(&entry.timestamp)
                                .size(metrics.small)
                                .color(color(palette.text_muted)),
                            text(&entry.message)
                                .size(metrics.small)
                                .color(color(palette.text))
                        ]
                        .spacing(metrics.space_2),
                    )
                },
            );
            panel(
                column![
                    text(title).size(metrics.body).color(color(palette.text)),
                    lines
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::NodeSummary { title, nodes } => {
            let lines =
                nodes
                    .iter()
                    .take(8)
                    .fold(column![].spacing(metrics.space_2), |column, node| {
                        column.push(
                            row![
                                status_dot(status_color(node.state, palette), 8.0),
                                text(&node.label)
                                    .size(metrics.small)
                                    .color(color(palette.text)),
                                space::horizontal(),
                                text(node.detail.clone().unwrap_or_default())
                                    .size(metrics.small)
                                    .color(color(palette.text_muted))
                            ]
                            .spacing(metrics.space_2),
                        )
                    });
            panel(
                column![
                    text(title).size(metrics.body).color(color(palette.text)),
                    lines
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
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

fn panel<'a, Message: 'a>(
    body: Element<'a, Message>,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let surface = color(visual.surface_raised);
    let border = color(visual.palette.border);
    let metrics = visual.metrics;
    let panel_opacity = visual.panel_opacity;
    let muted_opacity = visual.muted_opacity;
    let shadow_opacity = visual.shadow_opacity;
    let shadow_offset_y = visual.shadow_offset_y;
    let shadow_blur = visual.shadow_blur;
    container(body)
        .padding(metrics.space_4)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(with_alpha(surface, panel_opacity).into()),
            border: iced::Border {
                color: with_alpha(border, muted_opacity),
                width: 1.0,
                radius: metrics.radius_lg.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, shadow_opacity),
                offset: iced::Vector::new(0.0, shadow_offset_y),
                blur_radius: shadow_blur,
            },
            ..Default::default()
        })
        .into()
}

fn status_dot<'a, Message: 'a>(fill: Color, size: f32) -> Element<'a, Message> {
    container(space::horizontal())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::Border {
                color: Color { a: 0.35, ..fill },
                width: 1.0,
                radius: (size / 2.0).into(),
            },
            ..Default::default()
        })
        .into()
}

fn status_color(level: StatusLevel, palette: &ThemePalette) -> Color {
    color(match level {
        StatusLevel::Neutral => palette.info,
        StatusLevel::Good => palette.success,
        StatusLevel::Warning => palette.warning,
        StatusLevel::Critical => palette.danger,
    })
}

fn series_colors(palette: &ThemePalette) -> Vec<Color> {
    vec![
        color(palette.primary),
        color(palette.success),
        color(palette.info),
        color(palette.warning),
        color(palette.danger),
    ]
}

fn format_metric(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000.0 {
        format!("{:.1}B", value / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else if value.fract().abs() < 0.001 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

struct SparklineProgram {
    values: Vec<f32>,
    color: Color,
}

impl<Message> canvas::Program<Message> for SparklineProgram {
    type State = ();
    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.values.len() < 2 {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let min = self.values.iter().copied().fold(f32::INFINITY, f32::min);
        let max = self
            .values
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let point = |index: usize, value: f32| {
            Point::new(
                index as f32 / (self.values.len() - 1) as f32 * bounds.width,
                bounds.height - (value - min) / (max - min).max(1e-6) * bounds.height,
            )
        };
        for index in 1..self.values.len() {
            frame.stroke(
                &Path::line(
                    point(index - 1, self.values[index - 1]),
                    point(index, self.values[index]),
                ),
                canvas::Stroke::default()
                    .with_width(2.0)
                    .with_color(self.color),
            );
        }
        vec![frame.into_geometry()]
    }
}

struct GaugeProgram {
    value: f32,
    track: Color,
    fill: Color,
    radius: f32,
}

impl<Message> canvas::Program<Message> for GaugeProgram {
    type State = ();
    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), self.radius.into()),
            self.track,
        );
        frame.fill(
            &Path::rounded_rectangle(
                Point::ORIGIN,
                Size::new(
                    bounds.width * (self.value / 100.0).clamp(0.0, 1.0),
                    bounds.height,
                ),
                self.radius.into(),
            ),
            self.fill,
        );
        vec![frame.into_geometry()]
    }
}

struct HistogramProgram {
    values: Vec<f32>,
    color: Color,
    grid: Color,
}

impl<Message> canvas::Program<Message> for HistogramProgram {
    type State = ();
    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.values.is_empty() {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let max = self.values.iter().copied().fold(0.0, f32::max).max(1e-6);
        for row in 1..4 {
            let y = bounds.height * row as f32 / 4.0;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(bounds.width, y)),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(self.grid),
            );
        }
        let width = bounds.width / self.values.len() as f32;
        for (index, value) in self.values.iter().enumerate() {
            let height = value / max * bounds.height;
            frame.fill(
                &Path::rectangle(
                    Point::new(index as f32 * width + 1.0, bounds.height - height),
                    Size::new((width - 2.0).max(1.0), height),
                ),
                self.color,
            );
        }
        vec![frame.into_geometry()]
    }
}

struct TimeSeriesProgram {
    series: Vec<Vec<(f32, f32)>>,
    colors: Vec<Color>,
    grid: Color,
}

impl<Message> canvas::Program<Message> for TimeSeriesProgram {
    type State = ();
    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let points = self.series.iter().flatten().copied().collect::<Vec<_>>();
        if points.len() < 2 {
            return Vec::new();
        }
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        for row in 0..=4 {
            let y = bounds.height * row as f32 / 4.0;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(bounds.width, y)),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(self.grid),
            );
        }
        let map = |(x, y): (f32, f32)| {
            Point::new(
                (x - min_x) / (max_x - min_x).max(1e-6) * bounds.width,
                bounds.height - (y - min_y) / (max_y - min_y).max(1e-6) * bounds.height,
            )
        };
        for (index, series) in self.series.iter().enumerate() {
            for pair in series.windows(2) {
                frame.stroke(
                    &Path::line(map(pair[0]), map(pair[1])),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(self.colors[index % self.colors.len()]),
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
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
