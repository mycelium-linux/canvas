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
use std::cell::Cell;
use std::hash::{Hash, Hasher};

use canvas_core::{ThemePalette, VisualTheme};
use iced::mouse;
use iced::widget::canvas::{Cache, Geometry, Path};
use iced::widget::{Canvas, canvas, column, container, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

mod quant;

/// Per-program cache state. Stores an iced `Cache` plus a data-derived key.
///
/// `key` is computed from the Program's data in `draw`. When it differs from
/// the stored value, `cache.clear()` runs first so the tessellator re-runs
/// only on actual data changes — not on every repaint.
#[derive(Default)]
pub struct CacheState {
    cache: Cache,
    key: Cell<u64>,
}

impl CacheState {
    pub fn draw_cached<F>(&self, renderer: &Renderer, size: Size, key: u64, draw: F) -> Vec<Geometry>
    where
        F: FnOnce(&mut canvas::Frame),
    {
        if self.key.get() != key {
            self.cache.clear();
            self.key.set(key);
        }
        vec![self.cache.draw(renderer, size, draw)]
    }
}

/// Builds a cache key by accumulating the bits of the data that changes.
pub struct Key {
    hash: u64,
}

impl Key {
    pub fn new() -> Self {
        Self { hash: 0 }
    }
    pub fn add_u64(mut self, v: u64) -> Self {
        self.hash = self.hash.wrapping_mul(31).wrapping_add(v);
        self
    }
    pub fn add_f32(self, v: f32) -> Self {
        self.add_u64(v.to_bits() as u64)
    }
    pub fn add_f64(self, v: f64) -> Self {
        self.add_u64(v.to_bits())
    }
    pub fn add_usize(self, v: usize) -> Self {
        self.add_u64(v as u64)
    }
    pub fn add_color(self, c: iced::Color) -> Self {
        self.add_u64(c.r.to_bits() as u64)
            .add_u64(c.g.to_bits() as u64)
            .add_u64(c.b.to_bits() as u64)
            .add_u64(c.a.to_bits() as u64)
    }
    pub fn add_str(self, s: &str) -> Self {
        self.add_u64(s.len() as u64).add_u64(hash_str(s))
    }
    pub fn build(self) -> u64 {
        self.hash
    }
}

impl Default for Key {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_str(s: &str) -> u64 {
    s.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
}

/// Hash any `Hash`-implementing data. For floats and colors, use `Key` instead.
pub fn hash_data<D: Hash>(data: D) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// Render any widget spec using the same semantic theme contract.
pub fn element<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    // Freshness is carried by the value, so the body draws from a palette the
    // data state has already been applied to. `panel` is handed the true theme:
    // the provenance badge, the state dot and the footer have to stay legible
    // in exactly the states that drain everything else.
    let fresh = freshness(spec.data.state, visual);
    let palette = &fresh.values;
    let metrics = visual.metrics;
    match &spec.kind {
        // Quant draws through its own module, so the drained palette has to be
        // handed over inside a theme rather than as a loose palette — without
        // this a stale candlestick renders identically to a live one, which is
        // the exact failure rule 2 names.
        WidgetKind::Quant { widget } => {
            let mut drained = *visual;
            drained.palette = fresh.values;
            quant::element(spec, widget, &drained)
        }
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
                    .size(metrics.hero)
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
            // Label above the value, as the gauge also does: the label names the
            // metric, and a name reads as a caption rather than a footnote.
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
        WidgetKind::Module { module, .. } => panel(
            spec,
            container(text(format!("unresolved module: {module}")).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into(),
            visual,
        ),
        WidgetKind::Text { text: body } => panel(
            spec,
            container(text(body).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into(),
            visual,
        ),
        WidgetKind::Clock { format } => {
            let now = chrono_now();
            let rendered = strftime(&now, format);
            panel(
                spec,
                container(text(rendered).size(metrics.hero).color(color(palette.text)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center(Length::Fill)
                    .into(),
                visual,
            )
        }
        WidgetKind::Ticker { entries } => {
            let quotes = entries
                .iter()
                .map(|entry| {
                    let state_color = entry.state.as_ref().map_or(color(palette.text), |status| {
                        match status {
                            StatusLevel::Good => color(palette.success),
                            StatusLevel::Warning => color(palette.warning),
                            StatusLevel::Critical => color(palette.danger),
                            StatusLevel::Neutral => color(palette.text),
                        }
                    });
                    column![
                        text(&entry.label)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                        text(format!("{:.2}", entry.value))
                            .size(metrics.large)
                            .color(color(palette.text)),
                        text(entry.state.as_ref().map(|s| format!("{s:?}")).unwrap_or_default())
                            .size(metrics.small)
                            .color(state_color),
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
                    // Same order as Stat — label, value, then the track the
                    // value fills. The two disagreeing was the tell that
                    // neither had been placed on purpose.
                    .push(
                        text(label)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                    )
                    .push(text(pct).size(metrics.hero).color(color(palette.text)))
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
                    label_size: metrics.micro,
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
                        .map(|bar| {
                            (
                                bar.label.clone(),
                                bar.value as f32,
                                bar.state.map(|level| status_color(level, palette)),
                            )
                        })
                        .collect(),
                    palette: series_colors(palette),
                    label: color(palette.text_muted),
                    label_size: metrics.micro,
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
        } => {
            // One ramp for the whole widget: the legend and the bars have to
            // agree, and rebuilding it per legend item is a Vec per series.
            let colors = series_colors(palette);
            panel(
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
                            status_dot(colors[index % colors.len()], 7.0),
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
                        colors,
                        label: color(palette.text_muted),
                        label_size: metrics.micro,
                    })
                    .width(Length::Fill)
                    .height(Length::Fill),
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::StateTimeline { title, lanes } => panel(
            spec,
            column![
                text(title).size(metrics.body).color(color(palette.text)),
                Canvas::new(StateTimelineProgram {
                    lanes: lanes.clone(),
                    palette: *palette,
                    label: color(palette.text_muted),
                    label_size: metrics.micro,
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
                    label_size: metrics.micro,
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
            let lines = if let Some(overflow) = overflow_label(rows.len(), 8, "rows") {
                lines.push(
                    text(overflow)
                        .size(metrics.small)
                        .color(color(palette.warning)),
                )
            } else {
                lines
            };
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
            let lines = if let Some(overflow) = overflow_label(entries.len(), 7, "events") {
                lines.push(
                    text(overflow)
                        .size(metrics.small)
                        .color(color(palette.warning)),
                )
            } else {
                lines
            };
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
            let lines = if let Some(overflow) = overflow_label(nodes.len(), 8, "nodes") {
                lines.push(
                    text(overflow)
                        .size(metrics.small)
                        .color(color(palette.warning)),
                )
            } else {
                lines
            };
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
    let metrics = visual.metrics;
    let panel_opacity = visual.panel_opacity;
    // Resolved before the closure below: capturing `visual` would tie the style
    // to the theme's lifetime, and Color is Copy.
    let border = freshness(spec.data.state, visual).border;
    let content: Element<'a, Message> = match spec.data.state {
        WidgetDataState::Loading
        | WidgetDataState::NoData
        | WidgetDataState::Disconnected
        | WidgetDataState::Error => data_state_content(spec, visual),
        // Stale and Partial keep their body: an old number and an incomplete
        // set are both still information. Neither is dimmed — the value itself
        // already carries Stale (drained of colour by [`freshness`] where the
        // body was built) and the border carries Partial, so dimming here would
        // only cost the labels contrast they need in every state.
        WidgetDataState::Ready | WidgetDataState::Stale | WidgetDataState::Partial => body,
    };
    let mut layout = column![].spacing(metrics.space_2);
    // The badge sits above the body and is drawn in every state, including
    // Ready: a demo widget that has data is exactly the one that would
    // otherwise pass for real.
    if let Some(badge) = provenance_badge(spec, visual) {
        layout = layout.push(badge);
    }
    layout = layout.push(content);
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
                color: border,
                width: 1.0,
                radius: metrics.radius_lg.into(),
            },
            // No shadow. The host draws a plane with its own shadow behind every
            // widget surface, and a second concentric one at a near-identical
            // radius is the single clearest signal that this is unfinished.
            ..Default::default()
        })
        .into()
}

/// How a data state marks the widget it belongs to.
///
/// Stale and Partial are different claims and must not share a mark: Stale says
/// the number is old, Partial says the number is current but the set behind it
/// is incomplete. Both used to dim the whole body to `muted_opacity`, which
/// stated neither — it faded the labels and the chrome along with the value, so
/// a stale widget read as a widget in a low-contrast theme.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Freshness {
    /// The palette the body draws from. Every value-bearing mark in this
    /// library resolves through a palette role, so a state is applied once here
    /// rather than at each of the marks.
    values: ThemePalette,
    /// The panel's border.
    border: Color,
}

fn freshness(state: WidgetDataState, visual: &VisualTheme) -> Freshness {
    let palette = visual.palette;
    let quiet = with_alpha(color(palette.border), visual.muted_opacity);
    match state {
        // A stale value keeps its size, its weight and its opacity — it is
        // still the number someone reads from across the room — and gives up
        // its chroma at fixed perceptual lightness. Contrast is untouched,
        // which is what separates "this is old" from "this theme is washed
        // out"; the value reads as drained rather than dim.
        WidgetDataState::Stale => Freshness {
            values: drain(palette, STALE_CHROMA),
            border: quiet,
        },
        // Partial data is current, so the value is drawn exactly as a live one.
        // What is incomplete is the extent the panel claims to cover, so the
        // panel edge is what carries it.
        WidgetDataState::Partial => Freshness {
            values: palette,
            border: with_alpha(data_state_color(state, &palette), visual.muted_opacity),
        },
        // The remaining states replace the body outright, so they have no value
        // left to mark.
        WidgetDataState::Ready
        | WidgetDataState::Loading
        | WidgetDataState::NoData
        | WidgetDataState::Disconnected
        | WidgetDataState::Error => Freshness {
            values: palette,
            border: quiet,
        },
    }
}

/// The share of a role's chroma a stale value keeps.
///
/// Not zero: colour is identity as well as vitality, and six series drained to
/// one grey is a chart whose legend no longer maps to anything. Where a drained
/// role meets a rule with a chroma floor of its own — [`series_colors`] — the
/// floor wins, and that is the right way round: staleness may take colour from
/// an identity, but never the identity.
const STALE_CHROMA: f32 = 0.35;

/// Every role pulled toward grey at its own perceptual lightness.
///
/// The whole palette, rather than an enumerated list of "value" roles: text,
/// surface and border carry almost no chroma to begin with and barely move, so
/// the visible change lands on exactly the marks that carry a value — the
/// sparkline, the gauge fill, the change percentage, the status dots — without
/// anything having to keep a list of those in step with the widget kinds.
fn drain(palette: ThemePalette, keep: f32) -> ThemePalette {
    let role = |rgb: [u8; 3]| {
        let value = to_oklch(color(rgb));
        let drained = from_oklch(Oklch {
            c: value.c * keep,
            ..value
        });
        [
            channel_u8(drained.r),
            channel_u8(drained.g),
            channel_u8(drained.b),
        ]
    };
    // Named field by field on purpose: a role added to the palette upstream
    // should stop this build rather than quietly keep its colour when stale.
    ThemePalette {
        background: role(palette.background),
        background_strong: role(palette.background_strong),
        surface: role(palette.surface),
        surface_muted: role(palette.surface_muted),
        text: role(palette.text),
        text_muted: role(palette.text_muted),
        primary: role(palette.primary),
        success: role(palette.success),
        warning: role(palette.warning),
        danger: role(palette.danger),
        info: role(palette.info),
        border: role(palette.border),
        border_strong: role(palette.border_strong),
    }
}

fn channel_u8(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A persistent mark on any widget whose values are not real.
///
/// Deliberately not a tooltip and not dismissible: someone walking past has to
/// see it, and the whole point is that it survives the widget looking healthy.
fn provenance_badge<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
) -> Option<Element<'a, Message>> {
    let label = spec.data.provenance.badge()?;
    let metrics = visual.metrics;
    let accent = color(visual.palette.warning);
    Some(
        row![
            space::horizontal(),
            container(
                text(label)
                    .size(metrics.small)
                    .color(color(visual.palette.background)),
            )
            .padding([2, 6])
            .style(move |_| container::Style {
                background: Some(accent.into()),
                border: iced::Border {
                    color: accent,
                    width: 1.0,
                    radius: metrics.radius_sm.into(),
                },
                ..Default::default()
            }),
        ]
        .align_y(iced::alignment::Vertical::Center)
        .into(),
    )
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
    let state = spec.data.state;
    // The observed time is drawn on its own rather than inside the metadata
    // run, because it is the one field the state label makes a claim about.
    let observed = observed_label(state, spec.data.observed_at.as_deref());
    let metadata = [spec.data.source.as_deref(), spec.data.window.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(FOOTER_SEPARATOR);
    let has_metadata = !metadata.is_empty();
    if !has_metadata && observed.is_none() && state == WidgetDataState::Ready {
        return None;
    }
    let metrics = visual.metrics;
    let muted = color(visual.palette.text_muted);
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
    if has_metadata || observed.is_some() {
        footer = footer.push(space::horizontal());
    }
    if has_metadata {
        footer = footer.push(text(metadata).size(metrics.small).color(muted));
    }
    if let Some(observed) = observed {
        if has_metadata {
            footer = footer.push(
                text(FOOTER_SEPARATOR.trim())
                    .size(metrics.small)
                    .color(muted),
            );
        }
        // "Stale" is a claim about this time, so the two are drawn as one
        // statement in one colour instead of a state badge at one end of the
        // footer and an unrelated-looking timestamp at the other. A second
        // freshness indicator would be a second thing to keep in agreement.
        footer = footer.push(text(observed).size(metrics.small).color(
            if state == WidgetDataState::Stale {
                data_state_color(state, &visual.palette)
            } else {
                muted
            },
        ));
    }
    Some(footer.align_y(iced::alignment::Vertical::Center).into())
}

/// Between fields in the footer's metadata run.
const FOOTER_SEPARATOR: &str = "  ·  ";

/// When the widget's value was observed, as the footer states it.
///
/// A widget that claims to be stale but cannot say what it is stale *since*
/// is a payload bug, and saying so is louder than showing a bare "Stale" that
/// a reader will silently date to now.
fn observed_label(state: WidgetDataState, observed_at: Option<&str>) -> Option<String> {
    match (observed_at, state) {
        (Some(observed), _) => Some(observed.to_string()),
        (None, WidgetDataState::Stale) => Some("observed time unknown".to_string()),
        (None, _) => None,
    }
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

/// Return a visible disclosure when a bounded operational list is clipped.
/// Silent truncation makes an executive view look complete when it is not.
fn overflow_label(total: usize, shown: usize, noun: &str) -> Option<String> {
    total
        .checked_sub(shown)
        .filter(|remaining| *remaining > 0)
        .map(|remaining| format!("+{remaining} more {noun}"))
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

/// Distinct colours a series is identified by, derived from the theme.
///
/// Categorical colour and status colour are two languages, and this used to
/// speak one where it meant the other: the ramp *was* `[primary, success, info,
/// warning, danger]`, so `ops-stacked-bar`'s `["ok", "retried", "failed"]`
/// rendered green, cyan, amber by list position. Colour was making a claim
/// about severity that the data never made.
///
/// The ramp rotates hue off `primary` in OKLCH — perceptually even steps, so
/// neighbours separate as well at the end of the ramp as at the start — and
/// alternates lightness, which keeps neighbours apart in greyscale and for a
/// red-green colour-blind reader. The hue windows the status roles occupy are
/// excluded from the walk rather than merely stepped over, so no palette,
/// imported ones included, can produce a series that reads as a state.
pub fn series_colors(palette: &ThemePalette) -> Vec<Color> {
    let base = to_oklch(color(palette.primary));
    let open = categorical_arcs(&[
        to_oklch(color(palette.success)).h,
        to_oklch(color(palette.warning)).h,
        to_oklch(color(palette.danger)).h,
        to_oklch(color(palette.info)).h,
    ]);
    let span = arc_span(&open);
    let start = arc_offset(&open, base.h);
    let chroma = base.c.clamp(SERIES_CHROMA_MIN, SERIES_CHROMA_MAX);
    // Lightness comes from the theme's own primary, so a light theme's ramp
    // lands dark enough to read on white and a dark theme's light enough to
    // read on black, without either being written down anywhere.
    (0..SERIES_COLORS)
        .map(|index| {
            let lift = if index % 2 == 0 {
                1.0
            } else {
                1.0 + SERIES_LIGHTNESS_STEP
            };
            from_oklch(Oklch {
                l: (base.l * lift).clamp(SERIES_LIGHTNESS_MIN, SERIES_LIGHTNESS_MAX),
                c: chroma,
                h: arc_hue(&open, start + span * index as f32 / SERIES_COLORS as f32),
            })
        })
        .collect()
}

/// How many identities the ramp carries before it repeats. Beyond this a chart
/// has more series than a person can hold in one glance anyway, and the honest
/// answer is fewer series, not more colours.
const SERIES_COLORS: usize = 6;
/// Degrees of hue held clear either side of a status hue.
const STATUS_HUE_GUARD: f32 = 25.0;
/// The floor keeps a desaturated brand from yielding six greys; the ceiling
/// keeps a rotated hue inside sRGB, where an over-saturated one would clip and
/// collapse onto its neighbour.
const SERIES_CHROMA_MIN: f32 = 0.08;
const SERIES_CHROMA_MAX: f32 = 0.15;
/// Fraction of the base lightness that alternating entries are lifted by.
const SERIES_LIGHTNESS_STEP: f32 = 0.12;
const SERIES_LIGHTNESS_MIN: f32 = 0.30;
const SERIES_LIGHTNESS_MAX: f32 = 0.92;

/// The hue arcs a categorical colour may occupy: the circle, less a guard
/// window around each status hue, with overlapping windows merged.
fn categorical_arcs(status_hues: &[f32]) -> Vec<(f32, f32)> {
    let mut blocked: Vec<(f32, f32)> = Vec::new();
    for hue in status_hues {
        let start = (hue - STATUS_HUE_GUARD).rem_euclid(360.0);
        let end = start + STATUS_HUE_GUARD * 2.0;
        // A window straddling 0° is two windows in a sorted list.
        if end > 360.0 {
            blocked.push((start, 360.0));
            blocked.push((0.0, end - 360.0));
        } else {
            blocked.push((start, end));
        }
    }
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (start, end) in blocked {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut open = Vec::new();
    let mut cursor = 0.0f32;
    for (start, end) in merged {
        if start > cursor {
            open.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < 360.0 {
        open.push((cursor, 360.0));
    }
    open
}

fn arc_span(arcs: &[(f32, f32)]) -> f32 {
    arcs.iter().map(|(start, end)| end - start).sum()
}

/// The hue `distance` degrees along the open arcs, wrapping at the end.
fn arc_hue(arcs: &[(f32, f32)], distance: f32) -> f32 {
    let span = arc_span(arcs);
    if span <= f32::EPSILON {
        return distance.rem_euclid(360.0);
    }
    let mut remaining = distance.rem_euclid(span);
    for (start, end) in arcs {
        if remaining < end - start {
            return start + remaining;
        }
        remaining -= end - start;
    }
    arcs[0].0
}

/// Where `hue` sits along the open arcs. A hue inside a guarded window resolves
/// to the start of the next open arc, which is what keeps the guarantee true
/// even for a palette whose primary is itself a status colour.
fn arc_offset(arcs: &[(f32, f32)], hue: f32) -> f32 {
    let mut travelled = 0.0;
    for (start, end) in arcs {
        if hue < *end {
            return travelled + (hue - start).max(0.0);
        }
        travelled += end - start;
    }
    0.0
}

/// Oklab in cylindrical form: perceptual lightness, chroma, and hue in degrees.
/// Rotating hue here keeps apparent brightness roughly fixed, which sRGB hue
/// rotation does not — the reason the ramp is not built in RGB.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Oklch {
    l: f32,
    c: f32,
    h: f32,
}

// Matrices are Björn Ottosson's published sRGB ↔ Oklab pair.
fn to_oklch(value: Color) -> Oklch {
    let r = srgb_decode(value.r);
    let g = srgb_decode(value.g);
    let b = srgb_decode(value.b);
    let long = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b).cbrt();
    let medium = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let short = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let lightness = 0.210_454_26 * long + 0.793_617_8 * medium - 0.004_072_047 * short;
    let green_red = 1.977_998_5 * long - 2.428_592_2 * medium + 0.450_593_7 * short;
    let blue_yellow = 0.025_904_037 * long + 0.782_771_77 * medium - 0.808_675_77 * short;
    Oklch {
        l: lightness,
        c: green_red.hypot(blue_yellow),
        h: blue_yellow.atan2(green_red).to_degrees().rem_euclid(360.0),
    }
}

/// The nearest displayable colour at this lightness and hue.
///
/// Chroma is reduced until the conversion lands inside sRGB, rather than the
/// channels being clipped where they leave it: clipping moves the hue — by 14°
/// on a light saturated blue, straight into the window the status guard keeps
/// clear — while dropping chroma keeps the hue the ramp chose.
fn from_oklch(value: Oklch) -> Color {
    let mut chroma = value.c;
    if !in_srgb_gamut(oklch_to_linear(value)) {
        // Grey is always displayable, so the low end is a known-good bound and
        // eight halvings land within a percent of the gamut edge.
        let (mut lo, mut hi) = (0.0, value.c);
        for _ in 0..8 {
            let mid = (lo + hi) / 2.0;
            if in_srgb_gamut(oklch_to_linear(Oklch { c: mid, ..value })) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chroma = lo;
    }
    let [r, g, b] = oklch_to_linear(Oklch { c: chroma, ..value });
    Color::from_rgb(srgb_encode(r), srgb_encode(g), srgb_encode(b))
}

fn oklch_to_linear(value: Oklch) -> [f32; 3] {
    let (sin, cos) = value.h.to_radians().sin_cos();
    let (green_red, blue_yellow) = (value.c * cos, value.c * sin);
    let long = (value.l + 0.396_337_78 * green_red + 0.215_803_76 * blue_yellow).powi(3);
    let medium = (value.l - 0.105_561_346 * green_red - 0.063_854_17 * blue_yellow).powi(3);
    let short = (value.l - 0.089_484_18 * green_red - 1.291_485_5 * blue_yellow).powi(3);
    [
        4.076_741_7 * long - 3.307_711_6 * medium + 0.230_969_94 * short,
        -1.268_438 * long + 2.609_757_4 * medium - 0.341_319_38 * short,
        -0.004_196_086 * long - 0.703_418_6 * medium + 1.707_614_7 * short,
    ]
}

fn in_srgb_gamut(linear: [f32; 3]) -> bool {
    linear
        .iter()
        .all(|channel| (-1e-4..=1.0 + 1e-4).contains(channel))
}

fn srgb_decode(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// The clamp is a backstop for the fraction of a percent the gamut search in
/// [`from_oklch`] leaves on the table, not the gamut policy itself.
fn srgb_encode(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
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
    /// Resolved from the theme at construction, like the colours above: a
    /// canvas draws its own text and has no access to the type scale.
    label_size: f32,
}

impl<Message> canvas::Program<Message> for HeatmapProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let val_sum: u64 = self.values.iter().flatten().map(|v| v.to_bits()).sum();
        let key = Key::new()
            .add_usize(self.columns.len())
            .add_usize(self.rows.len())
            .add_usize(self.values.iter().flatten().count())
            .add_u64(val_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
        }
        for (row, (name, values)) in self.rows.iter().zip(&self.values).enumerate() {
            let y = top + row as f32 * cell_h;
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(0.0, y + cell_h * 0.65),
                color: self.label,
                size: iced::Pixels(self.label_size),
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
        })
    }
}

struct RankedBarProgram {
    /// Colour resolved at construction: the draw pass has the series ramp
    /// but not the palette, and a declared status must win over rank position.
    bars: Vec<(String, f32, Option<Color>)>,
    palette: Vec<Color>,
    label: Color,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for RankedBarProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let bar_sum: f32 = self.bars.iter().map(|b| b.1).sum();
        let key = Key::new()
            .add_usize(self.bars.len())
            .add_usize(self.palette.len())
            .add_f32(bar_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
        let mut bars = self.bars.clone();
        bars.sort_by(|a, b| b.1.total_cmp(&a.1));
        let max = bars
            .iter()
            .map(|bar| bar.1)
            .fold(0.0, f32::max)
            .max(f32::EPSILON);
        let row_h = bounds.height / bars.len() as f32;
        let label_w = (bounds.width * 0.24).clamp(52.0, 140.0);
        for (index, (name, value, state)) in bars.iter().enumerate() {
            let y = index as f32 * row_h;
            frame.fill_text(canvas::Text {
                content: name.clone(),
                position: Point::new(0.0, y + row_h * 0.65),
                color: self.label,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            frame.fill(
                &Path::rectangle(
                    Point::new(label_w, y + row_h * 0.2),
                    Size::new((bounds.width - label_w) * *value / max, row_h * 0.55),
                ),
                // A bar that declares a status is coloured by it. Ranking is
                // already conveyed by position, so spending colour on rank too
                // discards meaning the payload went to the trouble of carrying.
                state.unwrap_or(self.palette[index % self.palette.len()]),
            );
         }
        })
    }
}

struct StackedBarProgram {
    bars: Vec<(String, Vec<f64>)>,
    colors: Vec<Color>,
    label: Color,
    label_size: f32,
}
impl<Message> canvas::Program<Message> for StackedBarProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let val_sum: f64 = self.bars.iter().flat_map(|b| &b.1).sum();
        let key = Key::new()
            .add_usize(self.bars.len())
            .add_usize(self.colors.len())
            .add_f64(val_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
                size: iced::Pixels(self.label_size),
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
        })
    }
}

struct StateTimelineProgram {
    lanes: Vec<StateLane>,
    palette: ThemePalette,
    label: Color,
    label_size: f32,
}
impl<Message> canvas::Program<Message> for StateTimelineProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let seg_sum: f64 = self.lanes.iter().flat_map(|l| l.segments.iter()).map(|s| s.start + s.end).sum();
        let key = Key::new()
            .add_usize(self.lanes.len())
            .add_usize(self.lanes.iter().map(|l| l.segments.len()).sum())
            .add_f64(seg_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
                size: iced::Pixels(self.label_size),
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
        })
    }
}

struct BurnRateProgram {
    windows: Vec<BurnRateWindow>,
    good: Color,
    warning: Color,
    danger: Color,
    track: Color,
    label: Color,
    label_size: f32,
}
impl<Message> canvas::Program<Message> for BurnRateProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let rate_sum: f64 = self.windows.iter().map(|w| w.burn_rate).sum();
        let key = Key::new()
            .add_usize(self.windows.len())
            .add_f64(rate_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
                size: iced::Pixels(self.label_size),
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
        })
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

/// The value window a plot maps onto its bounds, so a mapping is stated once
/// and shared: a sparkline and a time series that scale differently are two
/// instruments, not one library.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlotRange {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl PlotRange {
    fn over<'a>(points: impl IntoIterator<Item = &'a (f32, f32)>) -> Self {
        points.into_iter().fold(
            Self {
                min_x: f32::INFINITY,
                max_x: f32::NEG_INFINITY,
                min_y: f32::INFINITY,
                max_y: f32::NEG_INFINITY,
            },
            |range, (x, y)| Self {
                min_x: range.min_x.min(*x),
                max_x: range.max_x.max(*x),
                min_y: range.min_y.min(*y),
                max_y: range.max_y.max(*y),
            },
        )
    }

    /// Screen position of one value inside `area`, y inverted. A series with no
    /// spread is centred rather than pinned to the floor: a flat line along the
    /// bottom edge reads as "no data" when it means "steady".
    fn project(&self, (x, y): (f32, f32), area: Rectangle) -> Point {
        let span_x = self.max_x - self.min_x;
        let span_y = self.max_y - self.min_y;
        let across = if span_x > f32::EPSILON {
            (x - self.min_x) / span_x
        } else {
            0.5
        };
        let up = if span_y > f32::EPSILON {
            (y - self.min_y) / span_y
        } else {
            0.5
        };
        Point::new(
            area.x + across * area.width,
            area.y + area.height - up * area.height,
        )
    }
}

/// The drawable rectangle for a line, held off the canvas edge by the head
/// dot's radius so the dot and the round caps are not sliced in half by it.
fn plot_area(bounds: Size) -> Rectangle {
    Rectangle {
        x: HEAD_RADIUS,
        y: HEAD_RADIUS,
        width: (bounds.width - HEAD_RADIUS * 2.0).max(1.0),
        height: (bounds.height - HEAD_RADIUS * 2.0).max(1.0),
    }
}

/// Stroke width shared by every plotted line, so a sparkline and a time series
/// read as one instrument.
const LINE_WIDTH: f32 = 2.0;
/// Opacity of the area under a line, at the line itself, falling to nothing at
/// the baseline. Divided among overlaid series so a four-series chart does not
/// silt up.
const AREA_ALPHA: f32 = 0.22;
/// Radius of the dot marking the newest point.
const HEAD_RADIUS: f32 = LINE_WIDTH * 1.6;

/// Draw one series: the area beneath it, the line as a single path, and a
/// filled head on the newest point.
///
/// Every vertex used to be the meeting of two separately stroked `Path::line`
/// segments, which on a 2px butt-capped stroke leaves a visible notch at each
/// one. One path with round joins removes them, and hands the tessellator one
/// shape instead of n.
fn draw_series_line(
    frame: &mut canvas::Frame,
    points: &[Point],
    baseline: f32,
    stroke: Color,
    area_alpha: f32,
) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    // A single sample is data, not an empty plot. There is no area or line to
    // draw through one point, but the head marks where the series actually is —
    // returning early here rendered a fully drawn, entirely blank chart.
    if points.len() < 2 {
        frame.fill(&Path::circle(*first, HEAD_RADIUS), stroke);
        return;
    }
    let crest = points.iter().map(|point| point.y).fold(baseline, f32::min);
    let area = Path::new(|path| {
        path.move_to(Point::new(first.x, baseline));
        for point in points {
            path.line_to(*point);
        }
        path.line_to(Point::new(last.x, baseline));
        path.close();
    });
    frame.fill(
        &area,
        canvas::gradient::Linear::new(Point::new(first.x, crest), Point::new(first.x, baseline))
            .add_stop(0.0, with_alpha(stroke, area_alpha))
            .add_stop(1.0, with_alpha(stroke, 0.0)),
    );
    let line = Path::new(|path| {
        path.move_to(*first);
        for point in &points[1..] {
            path.line_to(*point);
        }
    });
    frame.stroke(
        &line,
        canvas::Stroke {
            style: canvas::Style::Solid(stroke),
            width: LINE_WIDTH,
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..Default::default()
        },
    );
    // The newest point is what a live series is read for; without it the eye
    // has to work out which end of the line is now.
    frame.fill(&Path::circle(*last, HEAD_RADIUS), stroke);
}

struct SparklineProgram {
    values: Vec<f32>,
    color: Color,
}

impl<Message> canvas::Program<Message> for SparklineProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.values.len() < 2 {
            return Vec::new();
        }
        let val_sum: f32 = self.values.iter().sum();
        let key = Key::new()
            .add_usize(self.values.len())
            .add_f32(val_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
        let series: Vec<(f32, f32)> = self
            .values
            .iter()
            .enumerate()
            .map(|(index, value)| (index as f32, *value))
            .collect();
        let range = PlotRange::over(&series);
        let area = plot_area(bounds.size());
        let points: Vec<Point> = series
            .iter()
            .map(|value| range.project(*value, area))
            .collect();
        draw_series_line(frame, &points, bounds.height, self.color, AREA_ALPHA);
        })
    }
}

struct GaugeProgram {
    value: f32,
    track: Color,
    fill: Color,
    radius: f32,
}

impl<Message> canvas::Program<Message> for GaugeProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = Key::new().add_f32(self.value).build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
        })
    }
}

struct HistogramProgram {
    values: Vec<f32>,
    color: Color,
    grid: Color,
}

impl<Message> canvas::Program<Message> for HistogramProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.values.is_empty() {
            return Vec::new();
        }
        let val_sum: f32 = self.values.iter().sum();
        let key = Key::new()
            .add_usize(self.values.len())
            .add_f32(val_sum)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
        })
    }
}

struct TimeSeriesProgram {
    series: Vec<Vec<(f32, f32)>>,
    colors: Vec<Color>,
    grid: Color,
}

impl<Message> canvas::Program<Message> for TimeSeriesProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.series.iter().flatten().count() < 2 {
            return Vec::new();
        }
        let pts: usize = self.series.iter().flatten().count();
        let val: f32 = self.series.iter().flatten().map(|(_, y)| *y).sum();
        let key = Key::new()
            .add_usize(pts)
            .add_f32(val)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
        // One range over every series: two series on their own scales share an
        // axis that means nothing.
        let range = PlotRange::over(self.series.iter().flatten());
        let area = plot_area(bounds.size());
        for row in 0..=4 {
            let y = bounds.height * row as f32 / 4.0;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(bounds.width, y)),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(self.grid),
            );
        }
        // The fills stack, so each is worth a share of the budget rather than
        // all of it.
        let area_alpha = AREA_ALPHA / self.series.len().max(1) as f32;
        for (index, series) in self.series.iter().enumerate() {
            let points: Vec<Point> = series
                .iter()
                .map(|value| range.project(*value, area))
                .collect();
            draw_series_line(
                frame,
                &points,
                bounds.height,
                self.colors[index % self.colors.len()],
                area_alpha,
            );
        }
        })
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
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.data.is_empty() {
            return Vec::new();
        }
        let pts: usize = self.data.len();
        let val: f32 = self.data.iter().map(|(_, y)| *y).sum();
        let key = Key::new()
            .add_usize(pts)
            .add_f32(val)
            .add_usize(self.chart_type as usize)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
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
                let points: Vec<Point> = self
                    .data
                    .iter()
                    .map(|(x, y)| Point::new(sx(*x), sy(*y)))
                    .collect();
                draw_series_line(
                    frame,
                    &points,
                    bounds.height - margin,
                    self.line_color,
                    AREA_ALPHA,
                );
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn bounded_lists_disclose_hidden_items() {
        assert_eq!(overflow_label(8, 8, "rows"), None);
        assert_eq!(
            overflow_label(9, 8, "rows").as_deref(),
            Some("+1 more rows")
        );
        assert_eq!(
            overflow_label(12, 7, "events").as_deref(),
            Some("+5 more events")
        );
    }

    fn palettes() -> Vec<(&'static str, ThemePalette)> {
        use canvas_core::{Accent, ThemeMode, ThemeSelection};
        let dark = ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).palette();
        let light = ThemeSelection::coherence(ThemeMode::Light, Accent::Brand).palette();
        // A palette whose primary *is* a status colour is the case the guard
        // exists for: an imported palette owes us nothing.
        let mut collided = light;
        collided.primary = collided.danger;
        vec![
            ("dark", dark),
            ("light", light),
            ("primary-collides-with-danger", collided),
        ]
    }

    /// Shortest angular distance between two hues, in degrees.
    fn hue_gap(a: f32, b: f32) -> f32 {
        ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
    }

    /// Distance in Oklab, where a fixed step means the same perceived
    /// difference anywhere in the space — the property a ramp built by
    /// rotating RGB hue does not have.
    fn separation(a: Oklch, b: Oklch) -> f32 {
        let point = |c: Oklch| {
            let (sin, cos) = c.h.to_radians().sin_cos();
            (c.l, c.c * cos, c.c * sin)
        };
        let (l1, a1, b1) = point(a);
        let (l2, a2, b2) = point(b);
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }

    fn visual(palette: ThemePalette) -> VisualTheme {
        canvas_core::ThemeSelection::Palette { palette }.visual_theme()
    }

    #[test]
    fn every_data_state_a_widget_keeps_its_body_in_reads_differently() {
        for (name, palette) in palettes() {
            let visual = visual(palette);
            let ready = freshness(WidgetDataState::Ready, &visual);
            let stale = freshness(WidgetDataState::Stale, &visual);
            let partial = freshness(WidgetDataState::Partial, &visual);
            assert_ne!(ready, stale, "{name} draws a stale widget as a live one");
            assert_ne!(ready, partial, "{name} draws partial data as complete");
            assert_ne!(stale, partial, "{name} cannot tell old from incomplete");
        }
    }

    #[test]
    fn stale_marks_the_value_and_partial_marks_the_extent() {
        for (name, palette) in palettes() {
            let visual = visual(palette);
            let ready = freshness(WidgetDataState::Ready, &visual);
            let stale = freshness(WidgetDataState::Stale, &visual);
            let partial = freshness(WidgetDataState::Partial, &visual);
            // Partial data is current. The number is not old, so it is drawn
            // exactly as a live one and the frame says what is missing.
            assert_eq!(partial.values, ready.values, "{name} ages a current value");
            assert_ne!(
                partial.border, ready.border,
                "{name} leaves incomplete data unmarked"
            );
            // Staleness is the other way round: the value carries it, and the
            // chrome — which has to stay legible — does not.
            assert_ne!(stale.values, ready.values, "{name} hides staleness");
            assert_eq!(stale.border, ready.border, "{name} marks age on the frame");
        }
    }

    #[test]
    fn a_stale_value_gives_up_colour_and_not_contrast() {
        for (name, palette) in palettes() {
            let stale = drain(palette, STALE_CHROMA);
            for (role, live, drained) in [
                ("primary", palette.primary, stale.primary),
                ("success", palette.success, stale.success),
                ("warning", palette.warning, stale.warning),
                ("danger", palette.danger, stale.danger),
                ("info", palette.info, stale.info),
            ] {
                let (live, drained) = (to_oklch(color(live)), to_oklch(color(drained)));
                assert!(
                    drained.c < live.c,
                    "{name} {role} keeps {:.3} of its {:.3} chroma when stale",
                    drained.c,
                    live.c
                );
                // Lightness is contrast, and a stale number is still read from
                // across the room: draining must not cost it any. This is the
                // difference from the dimming this replaced, which cost only
                // contrast and no colour at all.
                assert!(
                    (drained.l - live.l).abs() < 0.02,
                    "{name} {role} moved from lightness {:.3} to {:.3}",
                    live.l,
                    drained.l
                );
            }
        }
    }

    #[test]
    fn a_stale_chart_can_still_be_read_against_its_legend() {
        // Draining is how staleness shows, but colour is identity too: six
        // series collapsed toward one grey is a chart whose legend maps to
        // nothing, and that is worse than an old chart. The ramp's own chroma
        // floor is what bounds the drain.
        for (name, palette) in palettes() {
            let colors = series_colors(&drain(palette, STALE_CHROMA));
            for (index, first) in colors.iter().enumerate() {
                for second in &colors[index + 1..] {
                    let apart = separation(to_oklch(*first), to_oklch(*second));
                    assert!(apart > 0.05, "{name} has two stale series {apart:.3} apart");
                }
            }
        }
    }

    #[test]
    fn a_widget_claiming_staleness_always_states_an_observed_time() {
        assert_eq!(observed_label(WidgetDataState::Ready, None), None);
        assert_eq!(observed_label(WidgetDataState::Partial, None), None);
        assert_eq!(
            observed_label(WidgetDataState::Ready, Some("19:30Z")).as_deref(),
            Some("19:30Z")
        );
        // One statement, one time: the footer already renders `observed_at`, so
        // "Stale" qualifies that field rather than implying a second one.
        assert_eq!(
            observed_label(WidgetDataState::Stale, Some("19:30Z")).as_deref(),
            Some("19:30Z")
        );
        assert_eq!(
            observed_label(WidgetDataState::Stale, None).as_deref(),
            Some("observed time unknown")
        );
    }

    #[test]
    fn a_series_colour_is_never_a_status_colour() {
        // The defect this ramp replaces: `ops-stacked-bar` declares
        // ["ok", "retried", "failed"] and the third series rendered in the
        // palette's `info` cyan purely because it was third. Colour has to stop
        // making claims about severity that the data did not make.
        for (name, palette) in palettes() {
            let status = [
                ("success", to_oklch(color(palette.success)).h),
                ("warning", to_oklch(color(palette.warning)).h),
                ("danger", to_oklch(color(palette.danger)).h),
                ("info", to_oklch(color(palette.info)).h),
            ];
            for (index, series) in series_colors(&palette).into_iter().enumerate() {
                let hue = to_oklch(series).h;
                for (role, status_hue) in status {
                    // A degree of slack for the trip out through 8-bit sRGB and
                    // back; the placement itself is exact.
                    assert!(
                        hue_gap(hue, status_hue) >= STATUS_HUE_GUARD - 1.0,
                        "{name} series {index} at {hue:.1}° reads as {role}"
                    );
                }
            }
        }
    }

    #[test]
    fn series_colours_stay_apart_from_each_other() {
        for (name, palette) in palettes() {
            let colors = series_colors(&palette);
            assert!(
                colors.len() >= 3,
                "{name} cannot even give three series their own colour"
            );
            for (i, first) in colors.iter().enumerate() {
                for second in &colors[i + 1..] {
                    let apart = separation(to_oklch(*first), to_oklch(*second));
                    assert!(apart > 0.05, "{name} has two series {apart:.3} apart");
                }
            }
        }
    }

    #[test]
    fn the_ramp_follows_the_theme_rather_than_being_a_fixed_list() {
        // Light and dark differ in lightness in the direction the background
        // demands; a hard-coded ramp would be identical in both.
        let lightness = |palette: &ThemePalette| {
            let colors = series_colors(palette);
            colors.iter().map(|c| to_oklch(*c).l).sum::<f32>() / colors.len() as f32
        };
        use canvas_core::{Accent, ThemeMode, ThemeSelection};
        let dark = ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).palette();
        let light = ThemeSelection::coherence(ThemeMode::Light, Accent::Brand).palette();
        assert!(lightness(&dark) > lightness(&light));
        // And the first entry still carries the brand: identity starts where
        // the theme's primary already points.
        let brand = to_oklch(color(light.primary)).h;
        assert!(hue_gap(to_oklch(series_colors(&light)[0]).h, brand) < 1.0);
    }

    #[test]
    fn oklch_survives_a_round_trip_through_srgb() {
        for rgb in [[36, 73, 216], [255, 255, 255], [0, 0, 0], [177, 43, 64]] {
            let original = color(rgb);
            let round_tripped = from_oklch(to_oklch(original));
            for (a, b) in [
                (original.r, round_tripped.r),
                (original.g, round_tripped.g),
                (original.b, round_tripped.b),
            ] {
                assert!(
                    (a - b).abs() < 0.01,
                    "{rgb:?} round-tripped to {b} from {a}"
                );
            }
        }
    }

    #[test]
    fn a_plot_fills_its_area_and_keeps_the_head_inside_it() {
        let bounds = Size::new(200.0, 100.0);
        let area = plot_area(bounds);
        let series: Vec<(f32, f32)> = vec![(0.0, 5.0), (1.0, 20.0), (2.0, 10.0)];
        let range = PlotRange::over(&series);
        let points: Vec<Point> = series
            .iter()
            .map(|value| range.project(*value, area))
            .collect();
        // The extremes reach the edges of the drawable area, or the series is
        // being drawn smaller than the room it was given.
        let reaches = |value: f32, edge: f32| (value - edge).abs() < 0.01;
        assert!(reaches(points[0].x, area.x));
        assert!(reaches(points[2].x, area.x + area.width));
        assert!(reaches(points[1].y, area.y), "the peak reaches the top");
        assert!(
            reaches(points[0].y, area.y + area.height),
            "the trough reaches the bottom"
        );
        // The head dot and the round caps have to survive the canvas edge, so
        // no point may sit closer to it than the dot's radius.
        let held_off = |value: f32, edge: f32| {
            value >= HEAD_RADIUS - 0.01 && value <= edge - HEAD_RADIUS + 0.01
        };
        for point in &points {
            assert!(held_off(point.x, bounds.width), "{point:?} clips the side");
            assert!(held_off(point.y, bounds.height), "{point:?} clips the edge");
        }
    }

    #[test]
    fn a_single_sample_projects_into_the_plot_rather_than_nowhere() {
        // One sample gives min == max, so a naive range divides by zero and the
        // point lands at NaN — which draws nothing and reads as a broken source.
        // draw_series_line also has to draw its head for this case, since there
        // is no line or area through a single point.
        let area = plot_area(Size::new(200.0, 100.0));
        let series = vec![(0.0, 7.0)];
        let range = PlotRange::over(&series);
        let point = range.project(series[0], area);
        assert!(point.x.is_finite() && point.y.is_finite(), "got {point:?}");
        assert!(
            point.y >= area.y && point.y <= area.y + area.height,
            "outside the plot: {point:?}"
        );
    }

    #[test]
    fn a_flat_series_is_drawn_through_the_middle() {
        // Pinned to the floor, a steady value is indistinguishable from an
        // empty widget — the one reading it must never carry.
        let area = plot_area(Size::new(200.0, 100.0));
        let series = vec![(0.0, 7.0), (1.0, 7.0), (2.0, 7.0)];
        let range = PlotRange::over(&series);
        for value in &series {
            let point = range.project(*value, area);
            assert!((point.y - (area.y + area.height / 2.0)).abs() < 0.001);
        }
    }

    #[test]
    fn cache_state_clears_on_key_change() {
        let key_a = Key::new().add_f32(1.0).build();
        let key_b = Key::new().add_f32(2.0).build();
        assert_ne!(key_a, key_b);
        assert_ne!(key_a, Key::new().add_f32(3.0).build());
    }

    #[test]
    fn cache_state_key_for_f32_values() {
        let a = Key::new().add_f32(3.5).add_usize(10).build();
        let b = Key::new().add_f32(3.5).add_usize(10).build();
        assert_eq!(a, b);
        let c = Key::new().add_f32(3.6).add_usize(10).build();
        let d = Key::new().add_f32(3.5).add_usize(11).build();
        assert_ne!(a, c);
        assert_ne!(a, d);
    }
}
