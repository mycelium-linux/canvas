//! Live in-canvas widget rendering via iced. Each widget maps a
//! [`WidgetSpec`](crate::WidgetSpec) plus a resolved Coherence visual theme to
//! an iced Element.
//! This is the same visual language the canvas already uses (VU meter,
//! presence orb, ticker text) — extracted so Lua, MCP, and the text protocol
//! all render through one library.

use crate::{
    BurnRateWindow, ChartType, MeterStyle, StateLane, StatusLevel, StatusState, WidgetDataState,
    WidgetKind, WidgetSpec,
};
use canvas_core::{ThemePalette, VisualTheme};
use iced::mouse;
use iced::widget::canvas::{Geometry, Path};
use iced::widget::{Canvas, canvas, column, container, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

mod quant;

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
        WidgetKind::Quant { widget } => quant::element(spec, widget, visual),
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
            panel(spec, body.into(), visual)
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
            panel(spec, body.into(), visual)
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
            panel(spec, row(quotes).spacing(metrics.space_5).into(), visual)
        }
        WidgetKind::Status { state } => {
            let orb = match state.unwrap_or(StatusState::Unknown) {
                StatusState::Present => with_alpha(color(palette.success), 0.85),
                StatusState::Away => with_alpha(color(palette.danger), 0.85),
                StatusState::Unknown => with_alpha(color(palette.warning), 0.85),
            };
            panel(
                spec,
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
                spec,
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
                spec,
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
            spec,
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
        WidgetKind::Heatmap {
            title,
            rows,
            columns,
            values,
            unit,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(unit.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(HeatmapProgram {
                    rows: rows.clone(),
                    columns: columns.clone(),
                    values: values.clone(),
                    low: color(palette.surface_muted),
                    high: color(palette.primary),
                    label: color(palette.text_muted),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::RankedBar { title, bars, unit } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(unit.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(RankedBarProgram {
                    bars: bars
                        .iter()
                        .map(|bar| (bar.label.clone(), bar.value as f32, bar.state))
                        .collect(),
                    palette: series_colors(palette),
                    label: color(palette.text_muted),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::StackedBar {
            title,
            series,
            bars,
            unit,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(unit.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                row(series
                    .iter()
                    .enumerate()
                    .map(|(index, label)| row![
                        status_dot(series_colors(palette)[index % 5], 7.0),
                        text(label)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                    ]
                    .spacing(metrics.space_1)
                    .into())
                    .collect::<Vec<Element<'_, Message>>>())
                .spacing(metrics.space_3),
                Canvas::new(StackedBarProgram {
                    bars: bars
                        .iter()
                        .map(|bar| (bar.label.clone(), bar.values.clone()))
                        .collect(),
                    colors: series_colors(palette),
                    label: color(palette.text_muted),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::StateTimeline { title, lanes } => panel(
            spec,
            column![
                text(title).size(metrics.body).color(color(palette.text)),
                Canvas::new(StateTimelineProgram {
                    lanes: lanes.clone(),
                    palette: *palette,
                    label: color(palette.text_muted),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::SloBurnRate {
            title,
            objective,
            windows,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!("{objective:.3}% objective"))
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(BurnRateProgram {
                    windows: windows.clone(),
                    good: color(palette.success),
                    warning: color(palette.warning),
                    danger: color(palette.danger),
                    track: color(palette.surface_muted),
                    label: color(palette.text_muted),
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
                spec,
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
                spec,
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
                spec,
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
                text("chart (live) — see canvas ChartProgram").color(color(palette.text_muted)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
    }
}

pub(crate) fn panel<'a, Message: 'a>(
    spec: &'a WidgetSpec,
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
    let content: Element<'a, Message> = match spec.data.state {
        WidgetDataState::Loading
        | WidgetDataState::NoData
        | WidgetDataState::Disconnected
        | WidgetDataState::Error => data_state_content(spec, visual),
        WidgetDataState::Ready | WidgetDataState::Stale | WidgetDataState::Partial => body,
    };
    let mut layout = column![content].spacing(metrics.space_2);
    if let Some(footer) = data_footer(spec, visual) {
        layout = layout.push(footer);
    }
    container(layout)
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

fn data_state_content<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let state = spec.data.state;
    let mut content = column![
        status_dot(data_state_color(state, &visual.palette), 9.0),
        text(data_state_label(state))
            .size(metrics.body)
            .color(color(visual.palette.text)),
    ]
    .spacing(metrics.space_2)
    .align_x(iced::alignment::Horizontal::Center);
    if let Some(detail) = &spec.data.detail {
        content = content.push(
            text(detail)
                .size(metrics.small)
                .color(color(visual.palette.text_muted)),
        );
    }
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .into()
}

fn data_footer<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
) -> Option<Element<'a, Message>> {
    let metadata = [
        spec.data.source.as_deref(),
        spec.data.window.as_deref(),
        spec.data.observed_at.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("  ·  ");
    if metadata.is_empty() && spec.data.state == WidgetDataState::Ready {
        return None;
    }
    let metrics = visual.metrics;
    let state = spec.data.state;
    let mut footer = row![].spacing(metrics.space_2);
    if state != WidgetDataState::Ready {
        footer = footer
            .push(status_dot(data_state_color(state, &visual.palette), 6.0))
            .push(
                text(data_state_label(state))
                    .size(metrics.small)
                    .color(data_state_color(state, &visual.palette)),
            );
    }
    if !metadata.is_empty() {
        footer = footer.push(space::horizontal()).push(
            text(metadata)
                .size(metrics.small)
                .color(color(visual.palette.text_muted)),
        );
    }
    Some(footer.align_y(iced::alignment::Vertical::Center).into())
}

fn data_state_label(state: WidgetDataState) -> &'static str {
    match state {
        WidgetDataState::Ready => "Live",
        WidgetDataState::Loading => "Loading",
        WidgetDataState::NoData => "No data",
        WidgetDataState::Stale => "Stale",
        WidgetDataState::Partial => "Partial data",
        WidgetDataState::Disconnected => "Disconnected",
        WidgetDataState::Error => "Source error",
    }
}

fn data_state_color(state: WidgetDataState, palette: &ThemePalette) -> Color {
    color(match state {
        WidgetDataState::Ready => palette.success,
        WidgetDataState::Loading => palette.info,
        WidgetDataState::NoData => palette.text_muted,
        WidgetDataState::Stale | WidgetDataState::Partial => palette.warning,
        WidgetDataState::Disconnected | WidgetDataState::Error => palette.danger,
    })
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

struct HeatmapProgram {
    rows: Vec<String>,
    columns: Vec<String>,
    values: Vec<Vec<f64>>,
    low: Color,
    high: Color,
    label: Color,
}

impl<Message> canvas::Program<Message> for HeatmapProgram {
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
        let label_width = (bounds.width * 0.18).clamp(42.0, 110.0);
        let top = 18.0;
        let cell_w = ((bounds.width - label_width) / self.columns.len() as f32).max(1.0);
        let cell_h = ((bounds.height - top) / self.rows.len() as f32).max(1.0);
        let (min, max) = self
            .values
            .iter()
            .flatten()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(*value), hi.max(*value))
            });
        for (column, name) in self.columns.iter().enumerate() {
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(label_width + column as f32 * cell_w + 3.0, 10.0),
                color: self.label,
                size: iced::Pixels(9.0),
                ..Default::default()
            });
        }
        for (row, (name, values)) in self.rows.iter().zip(&self.values).enumerate() {
            let y = top + row as f32 * cell_h;
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(0.0, y + cell_h * 0.65),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
            for (column, value) in values.iter().enumerate() {
                let t = ((*value - min) / (max - min).max(f64::EPSILON)) as f32;
                let fill = lerp_color(self.low, self.high, t);
                frame.fill(
                    &Path::rectangle(
                        Point::new(label_width + column as f32 * cell_w + 1.0, y + 1.0),
                        Size::new((cell_w - 2.0).max(1.0), (cell_h - 2.0).max(1.0)),
                    ),
                    fill,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

struct RankedBarProgram {
    bars: Vec<(String, f32, Option<StatusLevel>)>,
    palette: Vec<Color>,
    label: Color,
}

impl<Message> canvas::Program<Message> for RankedBarProgram {
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
        let mut bars = self.bars.clone();
        bars.sort_by(|a, b| b.1.total_cmp(&a.1));
        let max = bars
            .iter()
            .map(|bar| bar.1)
            .fold(0.0, f32::max)
            .max(f32::EPSILON);
        let row_h = bounds.height / bars.len() as f32;
        let label_w = (bounds.width * 0.24).clamp(52.0, 140.0);
        for (index, (name, value, _)) in bars.iter().enumerate() {
            let y = index as f32 * row_h;
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(0.0, y + row_h * 0.65),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
            frame.fill(
                &Path::rectangle(
                    Point::new(label_w, y + row_h * 0.2),
                    Size::new((bounds.width - label_w) * *value / max, row_h * 0.55),
                ),
                self.palette[index % self.palette.len()],
            );
        }
        vec![frame.into_geometry()]
    }
}

struct StackedBarProgram {
    bars: Vec<(String, Vec<f64>)>,
    colors: Vec<Color>,
    label: Color,
}
impl<Message> canvas::Program<Message> for StackedBarProgram {
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
        let max = self
            .bars
            .iter()
            .map(|(_, values)| values.iter().sum::<f64>())
            .fold(0.0, f64::max)
            .max(f64::EPSILON);
        let row_h = bounds.height / self.bars.len() as f32;
        let label_w = (bounds.width * 0.22).clamp(48.0, 130.0);
        for (row, (name, values)) in self.bars.iter().enumerate() {
            let y = row as f32 * row_h;
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(0.0, y + row_h * 0.65),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
            let mut x = label_w;
            for (index, value) in values.iter().enumerate() {
                let width = (bounds.width - label_w) * (*value / max) as f32;
                frame.fill(
                    &Path::rectangle(
                        Point::new(x, y + row_h * 0.2),
                        Size::new(width, row_h * 0.55),
                    ),
                    self.colors[index % self.colors.len()],
                );
                x += width;
            }
        }
        vec![frame.into_geometry()]
    }
}

struct StateTimelineProgram {
    lanes: Vec<StateLane>,
    palette: ThemePalette,
    label: Color,
}
impl<Message> canvas::Program<Message> for StateTimelineProgram {
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
        let min = self
            .lanes
            .iter()
            .flat_map(|lane| &lane.segments)
            .map(|segment| segment.start)
            .fold(f64::INFINITY, f64::min);
        let max = self
            .lanes
            .iter()
            .flat_map(|lane| &lane.segments)
            .map(|segment| segment.end)
            .fold(f64::NEG_INFINITY, f64::max);
        let span = (max - min).max(f64::EPSILON);
        let label_w = (bounds.width * 0.2).clamp(48.0, 130.0);
        let row_h = bounds.height / self.lanes.len() as f32;
        for (row, lane) in self.lanes.iter().enumerate() {
            let y = row as f32 * row_h;
            frame.fill_text(canvas::Text {
                content: lane.label.clone(),
                position: Point::new(0.0, y + row_h * 0.65),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
            for segment in &lane.segments {
                let x = label_w + ((segment.start - min) / span) as f32 * (bounds.width - label_w);
                let width =
                    ((segment.end - segment.start) / span) as f32 * (bounds.width - label_w);
                frame.fill(
                    &Path::rectangle(
                        Point::new(x, y + row_h * 0.18),
                        Size::new(width.max(1.0), row_h * 0.62),
                    ),
                    status_color(segment.state, &self.palette),
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

struct BurnRateProgram {
    windows: Vec<BurnRateWindow>,
    good: Color,
    warning: Color,
    danger: Color,
    track: Color,
    label: Color,
}
impl<Message> canvas::Program<Message> for BurnRateProgram {
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
        let max = self
            .windows
            .iter()
            .map(|window| window.burn_rate)
            .fold(1.0, f64::max);
        let row_h = bounds.height / self.windows.len() as f32;
        let label_w = (bounds.width * 0.2).clamp(44.0, 110.0);
        for (row, window) in self.windows.iter().enumerate() {
            let y = row as f32 * row_h;
            frame.fill_text(canvas::Text {
                content: format!("{}  {:.2}×", window.label, window.burn_rate),
                position: Point::new(0.0, y + row_h * 0.65),
                color: self.label,
                size: iced::Pixels(10.0),
                ..Default::default()
            });
            let rect = |width| {
                Path::rectangle(
                    Point::new(label_w, y + row_h * 0.25),
                    Size::new(width, row_h * 0.5),
                )
            };
            frame.fill(&rect(bounds.width - label_w), self.track);
            let fill = if window.burn_rate <= 1.0 {
                self.good
            } else if window.burn_rate <= 2.0 {
                self.warning
            } else {
                self.danger
            };
            frame.fill(
                &rect((bounds.width - label_w) * (window.burn_rate / max) as f32),
                fill,
            );
        }
        vec![frame.into_geometry()]
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
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
