//! Live in-canvas widget rendering via iced. Each widget maps a
//! [`WidgetSpec`](crate::WidgetSpec) plus a resolved Coherence visual theme to
//! an iced Element.
//! This is the same visual language the canvas already uses (VU meter,
//! presence orb, ticker text) — extracted so Lua, MCP, and the text protocol
//! all render through one library.

use crate::{
    ApprovalStatus, BurnRateWindow, ChartType, ClockMode, DawNote, DawTrack, DistributionGroup,
    DistributionStyle, EventEntry, HeatmapStyle, HierarchyNode, HierarchyStyle, MeterStyle,
    MetricBoardStyle, ModelAnnotation, ModelView, NetworkLink, NetworkLinkKind, NetworkNode,
    NetworkNodeKind, NetworkSegment, NetworkSegmentKind, NeuralEdge, NeuralNode, RichCardFormat,
    SpanTimelineStyle, SpatialBounds, SpatialTrack, StateLane, StatusLevel, StatusState,
    TargetMetric, TimedSpan, WidgetChrome, WidgetDataState, WidgetKind, WidgetSpec,
};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use canvas_core::{ThemeMetrics, ThemePalette, VisualTheme};
use iced::mouse;
use iced::widget::canvas::{Cache, Geometry, Path};
use iced::widget::{
    Canvas, canvas, column, container, image, progress_bar, responsive, row, scrollable, space,
    text,
};
use iced::{Color, ContentFit, Element, Length, Point, Rectangle, Renderer, Size, Theme};

pub mod app_chrome;
mod clock;
mod primitives;
mod quality;
mod quant;
mod signal_scene;
mod wasm_cdg;
mod wasm_hyperspace;
use clock::AnalogClockProgram;
use primitives::{
    header as primitive_header, list_row as primitive_list_row, metric as primitive_metric,
};

pub fn loading_rails<Message: 'static>(visual: VisualTheme) -> Element<'static, Message> {
    let palette = visual.palette;
    column![
        progress_bar(0.0..=1.0, 0.82).style(move |_| progress_bar::Style {
            background: with_alpha(color(palette.surface_muted), 0.7).into(),
            bar: color(palette.primary).into(),
            border: iced::Border::default(),
        }),
        progress_bar(0.0..=1.0, 0.56).style(move |_| progress_bar::Style {
            background: with_alpha(color(palette.surface_muted), 0.7).into(),
            bar: with_alpha(color(palette.primary), 0.72).into(),
            border: iced::Border::default(),
        }),
    ]
    .spacing(visual.metrics.space_1)
    .into()
}

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
    pub fn draw_cached<F>(
        &self,
        renderer: &Renderer,
        size: Size,
        key: u64,
        draw: F,
    ) -> Vec<Geometry>
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
    s.bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
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
    element_at(spec, visual, chrono_now())
}

/// Render a widget against an explicit wall-clock value.
///
/// Canvas uses this entry point so its clock text is derived from application
/// state. That makes each second a real iced tree change instead of relying on
/// an incidental wall-clock read during a potentially memoized view pass.
pub fn element_at<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
    now: EpochParts,
) -> Element<'a, Message> {
    element_at_with_artwork(spec, visual, now, None)
}

/// Render with an artwork handle resolved by the host's bounded asset loader.
pub fn element_at_with_artwork<'a, Message: 'a>(
    spec: &'a WidgetSpec,
    visual: &VisualTheme,
    now: EpochParts,
    artwork: Option<image::Handle>,
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
            let stale = matches!(spec.data.state, WidgetDataState::Stale);
            let accent = state.map_or(color(palette.primary), |state| status_color(state, palette));
            let value_color = if stale {
                with_alpha(color(palette.text), STALE_VALUE_ALPHA)
            } else {
                color(palette.text)
            };
            let body = column![
                primitive_metric(
                    label.clone(),
                    format_quantity(*value, unit.as_deref()),
                    change.map(|value| format!("{value:+.1}%")),
                    value_color,
                    visual,
                ),
                Canvas::new(SparklineProgram {
                    values: history.clone(),
                    color: accent,
                    label: color(palette.text_muted),
                    label_size: metrics.micro,
                    unit: unit.clone(),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2);
            panel(spec, body.into(), visual)
        }
        WidgetKind::MediaCard {
            title,
            artist,
            album,
            source,
            elapsed_seconds,
            duration_seconds,
            observed_at_ms,
            playing,
            ..
        } => {
            let advanced = if *playing {
                (now.epoch_millis - observed_at_ms).max(0) as f64 / 1_000.0
            } else {
                0.0
            };
            let elapsed = (elapsed_seconds + advanced).clamp(0.0, duration_seconds.max(0.0));
            let ratio = if *duration_seconds > 0.0 {
                (elapsed / duration_seconds).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let primary = color(palette.primary);
            let track = with_alpha(color(palette.text_muted), 0.22);
            // Keep progress as a float all the way to layout. Splitting the
            // rail into integer FillPortions quantized a four-minute song to
            // roughly quarter-second visual jumps despite a 40 ms host tick.
            let rail = progress_bar(0.0..=1.0, ratio as f32)
                .girth(4.0)
                .style(move |_| progress_bar::Style {
                    background: track.into(),
                    bar: primary.into(),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                });
            let time = row![
                text(format_media_time(elapsed))
                    .size(metrics.micro)
                    .color(color(palette.text_muted)),
                space().width(Length::Fill),
                text(format_media_time(*duration_seconds))
                    .size(metrics.micro)
                    .color(color(palette.text_muted)),
            ];
            let detail = album
                .as_deref()
                .filter(|value| !value.is_empty())
                .map_or_else(|| artist.clone(), |value| format!("{artist} · {value}"));
            let state = if *playing { "PLAYING" } else { "PAUSED" };
            let copy = column![
                text(title).size(metrics.title).color(color(palette.text)),
                text(detail)
                    .size(metrics.small)
                    .color(color(palette.text_muted)),
                space().height(Length::Fill),
                rail,
                time,
                text(format!(
                    "{}  ·  {}",
                    source.as_deref().unwrap_or("MEDIA"),
                    state
                ))
                .size(metrics.micro)
                .color(color(palette.text_muted)),
            ]
            .spacing(metrics.space_2)
            .width(Length::FillPortion(7));
            let art: Element<'a, Message> = if let Some(handle) = artwork {
                image(handle)
                    .content_fit(ContentFit::Cover)
                    .width(Length::FillPortion(4))
                    .height(Length::Fill)
                    .into()
            } else {
                container(text("♪").size(metrics.display).color(primary))
                    .center(Length::Fill)
                    .width(Length::FillPortion(4))
                    .height(Length::Fill)
                    .into()
            };
            panel(
                spec,
                row![art, copy]
                    .spacing(metrics.space_3)
                    .height(Length::Fill)
                    .into(),
                visual,
            )
        }
        WidgetKind::TimeSeries {
            title,
            series,
            unit,
        } => {
            let colors = series_colors(visual);
            let legend = responsive_series_legend(
                series.iter().map(|item| item.label.clone()).collect(),
                colors.clone(),
                metrics,
                color(palette.text_muted),
            );
            let body = column![
                primitive_header(title.clone(), unit.clone(), visual),
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
                    grid: with_alpha(color(palette.border), visual.grid_opacity),
                    label: color(palette.text_muted),
                    label_size: metrics.micro,
                    unit: unit.clone(),
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
        WidgetKind::Launcher {
            items,
            pinned: _,
            orientation: _,
        } => {
            let tile_color = color(palette.surface);
            let alert_color = color(palette.danger);
            let tiles: Vec<Element<'_, Message>> = if items.is_empty() {
                vec![
                    text("No projects")
                        .size(metrics.body)
                        .color(color(palette.text))
                        .into(),
                ]
            } else {
                items
                    .iter()
                    .map(|item| {
                        container(primitive_list_row(
                            item.label.clone(),
                            None,
                            item.unread.then_some(alert_color),
                            visual,
                        ))
                        .padding(4)
                        .style(move |_| container::Style {
                            background: Some(tile_color.into()),
                            ..Default::default()
                        })
                        .into()
                    })
                    .collect()
            };
            panel(spec, row(tiles).spacing(metrics.space_2).into(), visual)
        }
        WidgetKind::LyricsDisplay {
            title,
            artist,
            artwork_source: _,
            lines: _,
            position_ms: _,
            duration_ms: _,
            playing: _,
            style: _,
            scroll_offset: _,
            highlight_intensity: _,
        } => {
            // Placeholder - full implementation in progress
            // TODO: Implement lyrics display with hyperspace animations
            container(
                column![
                    text(title)
                        .size(visual.metrics.title)
                        .color(color(palette.text)),
                    text(artist)
                        .size(visual.metrics.small)
                        .color(color(palette.text_muted)),
                    text("[LYRICS DISPLAY - IMPLEMENTATION IN PROGRESS]")
                        .size(visual.metrics.small)
                        .color(color(palette.info)),
                ]
                .spacing(visual.metrics.space_2)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
        WidgetKind::HyperspaceShader {
            shader_id,
            speed,
            scale,
        } => {
            // Hyperspace shader visualizer drawing through the canvas program.
            // The renderer starts at t=0; a host tick drives `update` once
            // animation wiring lands.
            let renderer = wasm_hyperspace::HyperspaceRenderer::new(shader_id, *speed, *scale);
            container(
                column![
                    text(format!("Hyperspace: {}", shader_id))
                        .size(visual.metrics.title)
                        .color(color(palette.text)),
                    text(format!("Speed: {:.2} | Scale: {:.1}x", speed, scale))
                        .size(visual.metrics.small)
                        .color(color(palette.text_muted)),
                    renderer.into_element(),
                ]
                .spacing(visual.metrics.space_2)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
        WidgetKind::CdgViewer {
            title,
            source,
            current_frame,
            total_frames,
            position_ms: _,
            duration_ms: _,
            playing: _,
            scale,
        } => {
            // CDG viewer drawing through the canvas program. The framebuffer
            // starts in its border state; frame packets are applied by
            // `init_from_frame` once a CDG data source is wired up.
            let renderer = wasm_cdg::CdgRenderer::new(title, fresh.values);
            container(
                column![
                    text(title)
                        .size(visual.metrics.title)
                        .color(color(palette.text)),
                    text(format!("CDG: {}", source))
                        .size(visual.metrics.small)
                        .color(color(palette.text_muted)),
                    text(format!(
                        "Frame {}/{}",
                        current_frame,
                        (*total_frames).max(1)
                    ))
                    .size(visual.metrics.small)
                    .color(color(palette.info)),
                    text(format!("Scale: {:.1}x", scale))
                        .size(visual.metrics.micro)
                        .color(color(palette.text_muted)),
                    renderer.into_element(),
                ]
                .spacing(visual.metrics.space_2)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
        }
        WidgetKind::Text { text: body } => panel(
            spec,
            container(text(body).color(color(palette.text)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into(),
            visual,
        ),
        WidgetKind::Clock { format, mode } => {
            let content = match mode {
                ClockMode::Digital => container(
                    text(strftime(&now, format))
                        .size(metrics.hero)
                        .color(color(palette.text)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into(),
                ClockMode::Analog => Canvas::new(AnalogClockProgram {
                    hour: now.hour,
                    minute: now.minute,
                    second: now.second as f32 + now.millisecond as f32 / 1_000.0,
                    face: color(palette.surface),
                    marks: color(palette.text_muted),
                    hands: color(palette.text),
                    accent: color(palette.primary),
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            };
            panel(spec, content, visual)
        }
        WidgetKind::Ticker { entries } => {
            let quotes = entries
                .iter()
                .map(|entry| {
                    let state_color =
                        entry
                            .state
                            .as_ref()
                            .map_or(color(palette.text), |status| match status {
                                StatusLevel::Good => color(palette.success),
                                StatusLevel::Warning => color(palette.warning),
                                StatusLevel::Critical => color(palette.danger),
                                StatusLevel::Neutral => color(palette.text),
                            });
                    column![
                        text(&entry.label)
                            .size(metrics.small)
                            .color(color(palette.text_muted)),
                        text(format!("{:.2}", entry.value))
                            .size(metrics.large)
                            .color(color(palette.text)),
                        text(
                            entry
                                .state
                                .as_ref()
                                .map(|s| format!("{s:?}"))
                                .unwrap_or_default()
                        )
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
        WidgetKind::DawDeck {
            title,
            playing,
            paused,
            recording,
            bpm,
            position_ticks,
            cycle_ticks,
            tracks,
            notes,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!(
                        "{} · {} · {}",
                        if *recording {
                            "● REC"
                        } else if *playing {
                            "▶ PLAY"
                        } else if *paused {
                            "Ⅱ PAUSE"
                        } else {
                            "■ STOP"
                        },
                        bpm.map_or_else(|| "-- BPM".into(), |value| format!("{value:.1} BPM")),
                        format!(
                            "{:02}:{:02}",
                            position_ticks / 24 / 4,
                            (position_ticks / 24) % 4 + 1
                        ),
                    ))
                    .size(metrics.small)
                    .color(color(if *recording {
                        palette.danger
                    } else {
                        palette.text_muted
                    })),
                ],
                Canvas::new(DawDeckProgram {
                    playing: *playing,
                    recording: *recording,
                    position_ticks: *position_ticks,
                    cycle_ticks: *cycle_ticks,
                    tracks: tracks.clone(),
                    notes: notes.clone(),
                    palette: *palette,
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::Gauge { value, label } => {
            let pct = format!("{:.0}%", value);
            let label = label.clone().unwrap_or_default();
            panel(
                spec,
                iced::widget::Column::<Message>::new()
                    .push(primitive_metric(
                        label,
                        pct,
                        None,
                        color(palette.text),
                        visual,
                    ))
                    .push(
                        Canvas::new(GaugeProgram {
                            value: *value,
                            track: color(palette.surface_muted),
                            fill: color(palette.primary),
                            radius: metrics.radius_sm,
                            label: color(palette.text_muted),
                            label_size: metrics.micro,
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
                primitive_header(title.clone(), unit.clone(), visual),
                Canvas::new(HistogramProgram {
                    values: buckets.iter().map(|bucket| bucket.count as f32).collect(),
                    color: color(palette.info),
                    grid: with_alpha(color(palette.border), visual.grid_opacity),
                    label: color(palette.text_muted),
                    label_size: metrics.micro,
                    unit: unit.clone(),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::SpectralWaterfall {
            title,
            rows,
            min_frequency,
            max_frequency,
            unit,
            ..
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!(
                        "{}–{} {}",
                        short_value(*min_frequency),
                        short_value(*max_frequency),
                        unit.as_deref().unwrap_or("Hz")
                    ))
                    .size(metrics.small)
                    .color(color(palette.text_muted)),
                ],
                Canvas::new(SpectralWaterfallProgram {
                    rows: rows.clone(),
                    ramp: vec![
                        color(palette.background_strong),
                        color(palette.info),
                        color(palette.primary),
                        color(palette.warning),
                        color(palette.danger),
                    ],
                    label: color(palette.text_muted),
                    scan: color(palette.text),
                    label_size: metrics.micro,
                    min_frequency: *min_frequency,
                    max_frequency: *max_frequency,
                    unit: unit.clone().unwrap_or_else(|| "Hz".into()),
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
            style,
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
                    style: *style,
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
                    palette: series_colors(visual),
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
            let colors = series_colors(visual);
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
        WidgetKind::PieChart {
            title,
            values,
            unit,
            period,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(period.clone().unwrap_or_default())
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(PieChartProgram {
                    values: values
                        .iter()
                        .map(|value| (value.label.clone(), value.value))
                        .collect(),
                    colors: series_colors(visual),
                    text: color(palette.text),
                    surface: color(palette.surface),
                    label_size: metrics.small,
                    unit: unit.clone(),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::Sankey {
            title,
            nodes,
            links,
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
                Canvas::new(SankeyProgram {
                    nodes: nodes
                        .iter()
                        .map(|node| (
                            node.id.clone(),
                            node.label.clone(),
                            node.state.map(|state| status_color(state, palette))
                        ))
                        .collect(),
                    links: links
                        .iter()
                        .map(|link| (link.from.clone(), link.to.clone(), link.value as f32))
                        .collect(),
                    colors: series_colors(visual),
                    label: color(palette.text_muted),
                    surface: color(palette.surface_muted),
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::Hierarchy {
            title,
            style,
            nodes,
            unit,
            focus,
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
                Canvas::new(HierarchyProgram {
                    style: *style,
                    nodes: nodes.clone(),
                    focus: focus.clone(),
                    colors: series_colors(visual),
                    text: color(palette.text),
                    border: color(palette.border),
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::MetricBoard {
            title,
            style,
            metrics: values,
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
                Canvas::new(MetricBoardProgram {
                    style: *style,
                    metrics: values.clone(),
                    palette: *palette,
                    colors: series_colors(visual),
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::Distribution {
            title,
            style,
            groups,
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
                Canvas::new(DistributionProgram {
                    style: *style,
                    groups: groups.clone(),
                    colors: series_colors(visual),
                    text: color(palette.text_muted),
                    grid: with_alpha(color(palette.border), visual.grid_opacity),
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::SpanTimeline {
            title,
            style,
            spans,
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
                Canvas::new(SpanTimelineProgram {
                    style: *style,
                    spans: spans.clone(),
                    colors: series_colors(visual),
                    palette: *palette,
                    label_size: metrics.micro,
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
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::RateBars {
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
            // The table contract is already bounded to 256 rows. Render that
            // bounded dataset into a viewport instead of imposing a second,
            // unrelated eight-row cap: tall tiles should reveal more data,
            // while short tiles remain scrollable rather than lying about
            // how much space is available.
            let lines = rows.iter().fold(
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
                    scrollable(lines).height(Length::Fill)
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::EventLog {
            title,
            entries,
            query,
            paused,
            follow,
        } => {
            let query_lower = query.as_deref().unwrap_or_default().to_lowercase();
            let visible = entries
                .iter()
                .filter(|entry| {
                    query_lower.is_empty()
                        || entry.message.to_lowercase().contains(&query_lower)
                        || entry.timestamp.to_lowercase().contains(&query_lower)
                })
                .collect::<Vec<_>>();
            let ordered = if *follow {
                Box::new(visible.iter().rev().copied()) as Box<dyn Iterator<Item = &EventEntry>>
            } else {
                Box::new(visible.iter().copied()) as Box<dyn Iterator<Item = &EventEntry>>
            };
            let lines = ordered.fold(column![].spacing(metrics.space_2), |column, entry| {
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
            });
            panel(
                spec,
                column![
                    row![
                        text(title).size(metrics.body).color(color(palette.text)),
                        space::horizontal(),
                        text(if *paused {
                            "PAUSED"
                        } else if *follow {
                            "FOLLOW"
                        } else {
                            "HOLD"
                        })
                        .size(metrics.micro)
                        .color(color(if *paused {
                            palette.warning
                        } else {
                            palette.text_muted
                        })),
                    ],
                    query.as_ref().map(|value| text(format!("filter: {value}"))
                        .size(metrics.micro)
                        .color(color(palette.info))),
                    scrollable(lines).height(Length::Fill)
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::RichCard {
            title,
            format,
            body,
            language,
            citations,
        } => {
            let rich_background = with_alpha(color(palette.background_strong), 0.72);
            let rich_border = color(palette.border);
            let rich_radius = metrics.radius_sm;
            let body_color = match format {
                RichCardFormat::Diff => color(palette.info),
                _ => color(palette.text),
            };
            let body_text = text(body).size(metrics.small).color(body_color);
            let body_text = match format {
                RichCardFormat::Code | RichCardFormat::Diff => {
                    body_text.font(iced::Font::MONOSPACE)
                }
                RichCardFormat::Markdown => body_text,
            };
            let mut content = column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(
                        language
                            .clone()
                            .unwrap_or_else(|| format!("{format:?}").to_lowercase())
                    )
                    .size(metrics.micro)
                    .color(color(palette.text_muted)),
                ],
                scrollable(
                    container(body_text)
                        .padding(metrics.space_2)
                        .style(move |_| container::Style {
                            background: Some(rich_background.into()),
                            border: iced::Border {
                                color: rich_border,
                                width: 1.0,
                                radius: rich_radius.into()
                            },
                            ..Default::default()
                        })
                )
                .height(Length::Fill)
            ]
            .spacing(metrics.space_2);
            if !citations.is_empty() {
                content = content.push(
                    row(citations
                        .iter()
                        .map(|citation| {
                            text(format!("[{}]", citation.label))
                                .size(metrics.micro)
                                .color(color(palette.info))
                                .into()
                        })
                        .collect::<Vec<Element<'_, Message>>>())
                    .spacing(metrics.space_2),
                );
            }
            panel(spec, content.into(), visual)
        }
        WidgetKind::ApprovalCard {
            title,
            summary,
            scope,
            expires_at,
            provenance,
            approve_action,
            deny_action,
            status,
            ..
        } => {
            let approval_text = color(palette.text);
            let approval_background = color(palette.background);
            let approval_surface = with_alpha(color(palette.surface_muted), 0.8);
            let approval_border = color(palette.border);
            let approval_success = color(palette.success);
            let approval_danger = color(palette.danger);
            let approval_radius = metrics.radius_sm;
            let status_color_value = match status {
                ApprovalStatus::Pending => palette.warning,
                ApprovalStatus::Approved => palette.success,
                ApprovalStatus::Denied | ApprovalStatus::Expired => palette.danger,
            };
            let scopes = scope
                .iter()
                .map(|item| {
                    container(text(item).size(metrics.micro).color(approval_text))
                        .padding([3, 7])
                        .style(move |_| container::Style {
                            background: Some(approval_surface.into()),
                            border: iced::Border {
                                color: approval_border,
                                width: 1.0,
                                radius: approval_radius.into(),
                            },
                            ..Default::default()
                        })
                        .into()
                })
                .collect::<Vec<Element<'_, Message>>>();
            panel(
                spec,
                column![
                    row![
                        text(title).size(metrics.body).color(color(palette.text)),
                        space::horizontal(),
                        text(format!("{status:?}").to_uppercase())
                            .size(metrics.micro)
                            .color(color(status_color_value))
                    ],
                    text(summary).size(metrics.small).color(color(palette.text)),
                    row(scopes).spacing(metrics.space_1),
                    row![
                        text(format!("from {provenance}"))
                            .size(metrics.micro)
                            .color(color(palette.text_muted)),
                        space::horizontal(),
                        text(format!("expires {expires_at}"))
                            .size(metrics.micro)
                            .color(color(palette.text_muted))
                    ],
                    row![
                        container(
                            text(format!("Approve · {approve_action}"))
                                .size(metrics.small)
                                .color(approval_background)
                        )
                        .padding([7, 12])
                        .style(move |_| container::Style {
                            background: Some(approval_success.into()),
                            border: iced::Border {
                                radius: approval_radius.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        container(
                            text(format!("Deny · {deny_action}"))
                                .size(metrics.small)
                                .color(approval_text)
                        )
                        .padding([7, 12])
                        .style(move |_| container::Style {
                            background: Some(with_alpha(approval_danger, 0.22).into()),
                            border: iced::Border {
                                color: approval_danger,
                                width: 1.0,
                                radius: approval_radius.into()
                            },
                            ..Default::default()
                        })
                    ]
                    .spacing(metrics.space_2)
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::CommandPalette {
            query,
            results,
            selected,
        } => {
            let command_text = color(palette.text);
            let command_muted = color(palette.text_muted);
            let command_info = color(palette.info);
            let command_primary = color(palette.primary);
            let command_background = color(palette.background_strong);
            let command_border = color(palette.border);
            let command_radius = metrics.radius_sm;
            let rows = results.iter().enumerate().fold(
                column![].spacing(metrics.space_1),
                |column, (index, result)| {
                    let is_selected = index == *selected;
                    column.push(
                        container(
                            row![
                                text(&result.label).size(metrics.small).color(command_text),
                                text(&result.kind).size(metrics.micro).color(command_muted),
                                space::horizontal(),
                                text(result.shortcut.clone().unwrap_or_default())
                                    .size(metrics.micro)
                                    .color(command_info)
                            ]
                            .spacing(metrics.space_2)
                            .align_y(iced::Alignment::Center),
                        )
                        .padding([6, 9])
                        .style(move |_| container::Style {
                            background: is_selected
                                .then(|| with_alpha(command_primary, 0.18).into()),
                            border: iced::Border {
                                color: if is_selected {
                                    command_primary
                                } else {
                                    Color::TRANSPARENT
                                },
                                width: 1.0,
                                radius: command_radius.into(),
                            },
                            ..Default::default()
                        }),
                    )
                },
            );
            panel(
                spec,
                column![
                    container(
                        row![
                            text("⌕").size(metrics.body).color(command_info),
                            text(query).size(metrics.body).color(command_text)
                        ]
                        .spacing(metrics.space_2)
                    )
                    .padding([7, 10])
                    .style(move |_| container::Style {
                        background: Some(command_background.into()),
                        border: iced::Border {
                            color: command_border,
                            width: 1.0,
                            radius: command_radius.into()
                        },
                        ..Default::default()
                    }),
                    scrollable(rows).height(Length::Fill)
                ]
                .spacing(metrics.space_2)
                .into(),
                visual,
            )
        }
        WidgetKind::SocialFeed {
            title,
            status,
            detail,
            posts,
            rotate_seconds,
        } => {
            let status_color = if status == "healthy" {
                color(palette.success)
            } else {
                color(palette.danger)
            };
            let page_size = 3usize;
            let start = if posts.is_empty() {
                0
            } else {
                (((now.minute * 60 + now.second) / (*rotate_seconds).max(1)) as usize * page_size)
                    % posts.len()
            };
            let mut cards = column![].spacing(metrics.space_3);
            for offset in 0..posts.len().min(page_size) {
                let post = &posts[(start + offset) % posts.len()];
                let accent = color(palette.primary);
                let surface = color(palette.surface_muted);
                let matches = if post.matches.is_empty() {
                    String::new()
                } else {
                    format!("#{}", post.matches.join("  #"))
                };
                cards = cards.push(
                    container(
                        column![
                            row![
                                text(format!("@{}", post.author))
                                    .size(metrics.body)
                                    .color(accent),
                                space::horizontal(),
                                text(&post.age)
                                    .size(metrics.small)
                                    .color(color(palette.text_muted)),
                            ],
                            text(&post.text)
                                .size(metrics.body)
                                .color(color(palette.text))
                                .width(Length::Fill),
                            row![
                                text(format!("{} interactions", post.engagement))
                                    .size(metrics.small)
                                    .color(color(palette.text_muted)),
                                space::horizontal(),
                                text(matches).size(metrics.small).color(accent),
                            ],
                        ]
                        .spacing(metrics.space_2),
                    )
                    .padding(metrics.space_3)
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(surface.into()),
                        border: iced::Border {
                            color: accent,
                            width: 1.0,
                            radius: metrics.radius_md.into(),
                        },
                        ..Default::default()
                    }),
                );
            }
            if posts.is_empty() {
                cards = cards.push(
                    text(detail.as_deref().unwrap_or("Waiting for posts"))
                        .size(metrics.body)
                        .color(color(palette.text_muted)),
                );
            }
            panel(
                spec,
                column![
                    row![
                        status_dot(status_color, 9.0),
                        text(title).size(metrics.large).color(color(palette.text)),
                        space::horizontal(),
                        text(format!(
                            "{} / {} · auto {}s",
                            if posts.is_empty() { 0 } else { start + 1 },
                            posts.len(),
                            rotate_seconds
                        ))
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                    ]
                    .spacing(metrics.space_2),
                    cards,
                ]
                .spacing(metrics.space_3)
                .into(),
                visual,
            )
        }
        WidgetKind::MediaTransport {
            title,
            source,
            codec,
            width,
            height,
            fps,
            bitrate_kbps,
            buffer_ms,
            delay_ms,
            route,
            state,
        } => {
            let media_surface = with_alpha(color(palette.surface_muted), 0.8);
            let media_radius = metrics.radius_sm;
            let route_items = route
                .iter()
                .enumerate()
                .flat_map(|(index, hop)| {
                    let mut items: Vec<Element<'_, Message>> = Vec::new();
                    if index > 0 {
                        items.push(
                            text("→")
                                .size(metrics.small)
                                .color(color(palette.text_muted))
                                .into(),
                        );
                    }
                    items.push(
                        container(text(hop).size(metrics.micro).color(color(palette.text)))
                            .padding([4, 7])
                            .style(move |_| container::Style {
                                background: Some(media_surface.into()),
                                border: iced::Border {
                                    radius: media_radius.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                            .into(),
                    );
                    items
                })
                .collect::<Vec<_>>();
            let health = state.map_or(palette.info, |level| match level {
                StatusLevel::Neutral => palette.info,
                StatusLevel::Good => palette.success,
                StatusLevel::Warning => palette.warning,
                StatusLevel::Critical => palette.danger,
            });
            panel(
                spec,
                column![
                    row![
                        text(title).size(metrics.body).color(color(palette.text)),
                        space::horizontal(),
                        status_dot(color(health), 8.0),
                        text(source)
                            .size(metrics.small)
                            .color(color(palette.text_muted))
                    ]
                    .spacing(metrics.space_2)
                    .align_y(iced::Alignment::Center),
                    row(route_items).spacing(metrics.space_1),
                    row![
                        metric_pair("Codec", codec.clone(), metrics, palette),
                        metric_pair(
                            "Frame",
                            format!("{width}×{height} @ {fps:.1}"),
                            metrics,
                            palette
                        )
                    ]
                    .spacing(metrics.space_5),
                    row![
                        metric_pair(
                            "Bitrate",
                            format!("{bitrate_kbps:.0} kbps"),
                            metrics,
                            palette
                        ),
                        metric_pair("Buffer", format!("{buffer_ms:.0} ms"), metrics, palette),
                        metric_pair("A/V delay", format!("{delay_ms:.0} ms"), metrics, palette)
                    ]
                    .spacing(metrics.space_5)
                ]
                .spacing(metrics.space_3)
                .into(),
                visual,
            )
        }
        WidgetKind::ModelTelemetry {
            title,
            model,
            view,
            annotations,
            exploded,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!("{view:?} · explode {:.0}%", *exploded * 100.0))
                        .size(metrics.micro)
                        .color(color(palette.text_muted))
                ],
                Canvas::new(ModelTelemetryProgram {
                    model: model.clone(),
                    view: *view,
                    annotations: annotations.clone(),
                    exploded: *exploded,
                    palette: *palette,
                    label_size: metrics.micro
                })
                .width(Length::Fill)
                .height(Length::Fill)
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::StatusList { title, nodes } => {
            let title = title.clone();
            let nodes = nodes.clone();
            let visual = *visual;
            panel(
                spec,
                responsive(move |bounds| {
                    let metrics = visual.metrics;
                    let palette = visual.palette;
                    let available = (bounds.height - metrics.large - metrics.space_4).max(1.0);
                    let columns = usize::from(bounds.width >= 840.0 && nodes.len() >= 4) + 1;
                    let minimum_card_height = metrics.body + metrics.small + metrics.space_4 * 2.0;
                    let row_capacity = ((available / minimum_card_height).floor() as usize).max(1);
                    let shown = nodes.len().min(row_capacity * columns);
                    let mut cards = column![].spacing(metrics.space_2).height(Length::Fill);
                    for (row_index, chunk) in nodes[..shown].chunks(columns).enumerate() {
                        let mut card_row = row![]
                            .spacing(metrics.space_2)
                            .height(Length::FillPortion(1));
                        for (column_index, node) in chunk.iter().enumerate() {
                            let index = row_index * columns + column_index;
                            let fill = with_alpha(
                                color(if index % 2 == 0 {
                                    palette.surface_muted
                                } else {
                                    palette.surface
                                }),
                                if index % 2 == 0 { 0.52 } else { 0.28 },
                            );
                            card_row = card_row.push(
                                container(primitive_list_row(
                                    node.label.clone(),
                                    node.detail.clone(),
                                    Some(status_color(node.state, &palette)),
                                    &visual,
                                ))
                                .padding([metrics.space_3, metrics.space_4])
                                .width(Length::FillPortion(1))
                                .height(Length::Fill)
                                .align_y(iced::alignment::Vertical::Center)
                                .style(move |_| {
                                    container::Style {
                                        background: Some(fill.into()),
                                        border: iced::Border {
                                            color: with_alpha(color(palette.border), 0.42),
                                            width: 1.0,
                                            radius: metrics.radius_sm.into(),
                                        },
                                        ..Default::default()
                                    }
                                }),
                            );
                        }
                        if chunk.len() < columns {
                            card_row = card_row.push(space().width(Length::FillPortion(1)));
                        }
                        cards = cards.push(card_row);
                    }
                    if let Some(overflow) = overflow_label(nodes.len(), shown, "systems") {
                        cards = cards.push(
                            text(overflow)
                                .size(metrics.small)
                                .color(color(palette.warning)),
                        );
                    }
                    column![primitive_header(title.clone(), None, &visual), cards]
                        .spacing(metrics.space_3)
                        .into()
                })
                .into(),
                &visual,
            )
        }
        WidgetKind::NeuralGraph {
            title,
            nodes,
            edges,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!("{} nodes · {} paths", nodes.len(), edges.len()))
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(NeuralGraphProgram {
                    nodes: nodes.clone(),
                    edges: edges.clone(),
                    palette: *palette,
                    label_size: metrics.micro,
                    dag: false,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::NetworkTopology {
            title,
            segments,
            nodes,
            links,
            conflicts,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(if *conflicts == 0 {
                        format!("{} zones · {} nodes", segments.len(), nodes.len())
                    } else {
                        format!(
                            "{} zones · {} nodes · {} conflicts",
                            segments.len(),
                            nodes.len(),
                            conflicts
                        )
                    })
                    .size(metrics.small)
                    .color(color(if *conflicts == 0 {
                        palette.text_muted
                    } else {
                        palette.warning
                    })),
                ],
                Canvas::new(NetworkTopologyProgram {
                    segments: segments.clone(),
                    nodes: nodes.clone(),
                    links: links.clone(),
                    palette: *palette,
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::DagGraph {
            title,
            nodes,
            edges,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!(
                        "{} agents · {} delegations",
                        nodes.len(),
                        edges.len()
                    ))
                    .size(metrics.small)
                    .color(color(palette.text_muted)),
                ],
                Canvas::new(NeuralGraphProgram {
                    nodes: nodes.clone(),
                    edges: edges.clone(),
                    palette: *palette,
                    label_size: metrics.micro,
                    dag: true,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
        WidgetKind::SpatialTracks {
            title,
            tracks,
            bounds,
        } => panel(
            spec,
            column![
                row![
                    text(title).size(metrics.body).color(color(palette.text)),
                    space::horizontal(),
                    text(format!("{} tracks", tracks.len()))
                        .size(metrics.small)
                        .color(color(palette.text_muted)),
                ],
                Canvas::new(SpatialTracksProgram {
                    tracks: tracks.clone(),
                    bounds: bounds.clone(),
                    palette: *palette,
                    label_size: metrics.micro,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(metrics.space_2)
            .into(),
            visual,
        ),
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

struct SpatialTracksProgram {
    tracks: Vec<SpatialTrack>,
    bounds: SpatialBounds,
    palette: ThemePalette,
    label_size: f32,
}

struct ModelTelemetryProgram {
    model: String,
    view: ModelView,
    annotations: Vec<ModelAnnotation>,
    exploded: f32,
    palette: ThemePalette,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for ModelTelemetryProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self
            .annotations
            .iter()
            .fold(
                Key::new()
                    .add_str(&self.model)
                    .add_usize(self.view as usize)
                    .add_f32(self.exploded)
                    .add_f32(self.label_size)
                    .add_color(color(self.palette.primary)),
                |key, item| {
                    key.add_str(&item.id)
                        .add_str(&item.label)
                        .add_str(&item.value)
                        .add_f32(item.position[0])
                        .add_f32(item.position[1])
                        .add_f32(item.position[2])
                        .add_usize(item.state.map_or(usize::MAX, |state| state as usize))
                },
            )
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let center = Point::new(bounds.width * 0.48, bounds.height * 0.5);
            let size = bounds.width.min(bounds.height) * 0.28;
            let offset = size * (0.35 + self.exploded * 0.35);
            let front = [
                Point::new(center.x - size, center.y - size),
                Point::new(center.x + size, center.y - size),
                Point::new(center.x + size, center.y + size),
                Point::new(center.x - size, center.y + size),
            ];
            let back = front.map(|point| Point::new(point.x + offset, point.y - offset));
            let wire = with_alpha(color(self.palette.primary), 0.72);
            for face in [front, back] {
                for index in 0..4 {
                    frame.stroke(
                        &Path::line(face[index], face[(index + 1) % 4]),
                        canvas::Stroke::default().with_width(1.5).with_color(wire),
                    );
                }
            }
            for index in 0..4 {
                frame.stroke(
                    &Path::line(front[index], back[index]),
                    canvas::Stroke::default().with_width(1.0).with_color(wire),
                );
            }
            frame.fill_text(canvas::Text {
                content: self.model.clone(),
                position: Point::new(8.0, bounds.height - 5.0),
                color: color(self.palette.text_muted),
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            for annotation in &self.annotations {
                let [a, b, _c] = match self.view {
                    ModelView::Top | ModelView::Bottom => [
                        annotation.position[0],
                        annotation.position[2],
                        annotation.position[1],
                    ],
                    ModelView::Left | ModelView::Right => [
                        annotation.position[2],
                        annotation.position[1],
                        annotation.position[0],
                    ],
                    _ => annotation.position,
                };
                let point = Point::new(center.x + a * size, center.y - b * size);
                let mark = annotation.state.map_or(color(self.palette.info), |state| {
                    status_color(state, &self.palette)
                });
                frame.fill(&Path::circle(point, 4.0), mark);
                frame.stroke(
                    &Path::line(point, Point::new(point.x + 18.0, point.y - 14.0)),
                    canvas::Stroke::default().with_width(1.0).with_color(mark),
                );
                frame.fill_text(canvas::Text {
                    content: format!("{} · {}", annotation.label, annotation.value),
                    position: Point::new(point.x + 21.0, point.y - 15.0),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        })
    }
}

impl<Message> canvas::Program<Message> for SpatialTracksProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.tracks.iter().fold(Key::new(), |key, track| {
            track.trail.iter().fold(
                key.add_u64(u64::from(track.id))
                    .add_f32(track.position.x as f32)
                    .add_f32(track.position.z as f32)
                    .add_f32(track.confidence as f32)
                    .add_f32(track.motion as f32),
                |key, point| key.add_f32(point.x as f32).add_f32(point.z as f32),
            )
        });
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let pad = 12.0;
            let area = Rectangle::new(
                Point::new(pad, pad),
                Size::new(
                    (bounds.width - pad * 2.0).max(1.0),
                    (bounds.height - pad * 2.0).max(1.0),
                ),
            );
            let project = |x: f64, z: f64| {
                let nx = ((x - self.bounds.min_x) / (self.bounds.max_x - self.bounds.min_x)) as f32;
                let nz = ((z - self.bounds.min_z) / (self.bounds.max_z - self.bounds.min_z)) as f32;
                Point::new(
                    area.x + nx.clamp(0.0, 1.0) * area.width,
                    area.y + (1.0 - nz.clamp(0.0, 1.0)) * area.height,
                )
            };
            let grid = with_alpha(color(self.palette.border), 0.35);
            for step in 0..=4 {
                let t = step as f32 / 4.0;
                frame.stroke(
                    &Path::line(
                        Point::new(area.x + area.width * t, area.y),
                        Point::new(area.x + area.width * t, area.y + area.height),
                    ),
                    canvas::Stroke::default().with_width(1.0).with_color(grid),
                );
                frame.stroke(
                    &Path::line(
                        Point::new(area.x, area.y + area.height * t),
                        Point::new(area.x + area.width, area.y + area.height * t),
                    ),
                    canvas::Stroke::default().with_width(1.0).with_color(grid),
                );
            }
            if self.tracks.is_empty() {
                frame.fill_text(canvas::Text {
                    content: "Waiting for RuView tracks".into(),
                    position: Point::new(area.x + 8.0, area.y + area.height * 0.5),
                    color: color(self.palette.text_muted),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                return;
            }
            for track in &self.tracks {
                let trail = track
                    .trail
                    .iter()
                    .map(|point| project(point.x, point.z))
                    .collect::<Vec<_>>();
                if trail.len() > 1 {
                    let path = Path::new(|builder| {
                        builder.move_to(trail[0]);
                        for point in &trail[1..] {
                            builder.line_to(*point);
                        }
                    });
                    frame.stroke(
                        &path,
                        canvas::Stroke::default()
                            .with_width(2.0)
                            .with_color(with_alpha(color(self.palette.primary), 0.35)),
                    );
                }
                let position = project(track.position.x, track.position.z);
                let heat = (track.motion as f32 / 100.0).clamp(0.0, 1.0);
                let dot = lerp_oklch(color(self.palette.info), color(self.palette.primary), heat);
                frame.fill(
                    &Path::circle(position, 5.0 + heat * 4.0),
                    with_alpha(dot, 0.6 + track.confidence as f32 * 0.4),
                );
                frame.fill_text(canvas::Text {
                    content: track.id.to_string(),
                    position: Point::new(position.x + 9.0, position.y - 7.0),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        })
    }
}

struct NetworkTopologyProgram {
    segments: Vec<NetworkSegment>,
    nodes: Vec<NetworkNode>,
    links: Vec<NetworkLink>,
    palette: ThemePalette,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for NetworkTopologyProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.nodes.iter().fold(
            Key::new()
                .add_usize(self.segments.len())
                .add_usize(self.links.len()),
            |key, node| {
                let key = key
                    .add_str(&node.id)
                    .add_str(&node.label)
                    .add_u64(node.state as u64);
                let key = node.sites.iter().fold(key, |key, site| key.add_str(site));
                match &node.detail {
                    Some(detail) => key.add_str(detail),
                    None => key,
                }
            },
        );
        let key = self
            .segments
            .iter()
            .fold(key, |key, segment| match &segment.site {
                Some(site) => key.add_str(site),
                None => key,
            });
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let mut ordered_segments = self.segments.iter().collect::<Vec<_>>();
            ordered_segments.sort_by(|left, right| {
                left.site
                    .cmp(&right.site)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let count = ordered_segments.len().max(1) as f32;
            let gap = 10.0;
            let top = 68.0;
            let zone_w = ((bounds.width - gap * (count + 1.0)) / count).max(80.0);
            let zone_h = (bounds.height - top - gap).max(60.0);
            let mut zones = std::collections::BTreeMap::new();
            let mut site_extents = std::collections::BTreeMap::<String, (f32, f32)>::new();
            for (index, segment) in ordered_segments.iter().enumerate() {
                let origin = Point::new(gap + index as f32 * (zone_w + gap), top);
                let rect = Path::rounded_rectangle(origin, Size::new(zone_w, zone_h), 5.0.into());
                let accent = match segment.kind {
                    NetworkSegmentKind::Lan => self.palette.info,
                    NetworkSegmentKind::Vlan => self.palette.primary,
                    NetworkSegmentKind::Wan => self.palette.warning,
                    NetworkSegmentKind::PointToPoint => self.palette.success,
                };
                frame.fill(&rect, with_alpha(color(accent), 0.08));
                frame.stroke(
                    &rect,
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(with_alpha(color(accent), 0.55)),
                );
                frame.fill_text(canvas::Text {
                    content: segment.label.clone(),
                    position: Point::new(origin.x + 8.0, origin.y + 16.0),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                let meta = segment
                    .subnet
                    .clone()
                    .or_else(|| segment.vlan.map(|vlan| format!("VLAN {vlan}")))
                    .unwrap_or_else(|| format!("{:?}", segment.kind).to_lowercase());
                frame.fill_text(canvas::Text {
                    content: meta,
                    position: Point::new(origin.x + 8.0, origin.y + 31.0),
                    color: color(self.palette.text_muted),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                zones.insert(segment.id.clone(), (origin, Size::new(zone_w, zone_h)));
                if let Some(site) = &segment.site {
                    let extent = site_extents
                        .entry(site.clone())
                        .or_insert((origin.x, origin.x + zone_w));
                    extent.0 = extent.0.min(origin.x);
                    extent.1 = extent.1.max(origin.x + zone_w);
                }
            }
            for (site, (left, right)) in site_extents {
                frame.stroke(
                    &Path::line(Point::new(left, 30.0), Point::new(right, 30.0)),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(with_alpha(color(self.palette.primary), 0.55)),
                );
                frame.fill_text(canvas::Text {
                    content: site,
                    position: Point::new(left + 4.0, 23.0),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }

            let mut positions = std::collections::BTreeMap::new();
            let infrastructure = self
                .nodes
                .iter()
                .filter(|node| node.segments.len() != 1)
                .collect::<Vec<_>>();
            for (index, node) in infrastructure.iter().enumerate() {
                positions.insert(
                    node.id.clone(),
                    Point::new(28.0 + index as f32 * 128.0, 48.0),
                );
            }
            for segment in ordered_segments {
                let members = self
                    .nodes
                    .iter()
                    .filter(|node| node.segments.len() == 1 && node.segments[0] == segment.id)
                    .collect::<Vec<_>>();
                if let Some((origin, size)) = zones.get(&segment.id) {
                    for (index, node) in members.iter().enumerate() {
                        let columns = ((size.width - 20.0) / 105.0).floor().max(1.0) as usize;
                        let row = index / columns;
                        let column = index % columns;
                        positions.insert(
                            node.id.clone(),
                            Point::new(
                                origin.x + 18.0 + column as f32 * 105.0,
                                origin.y + 58.0 + row as f32 * 42.0,
                            ),
                        );
                    }
                }
            }
            for link in &self.links {
                if let (Some(from), Some(to)) = (positions.get(&link.from), positions.get(&link.to))
                {
                    let width = match link.kind {
                        NetworkLinkKind::Trunk => 3.0,
                        NetworkLinkKind::Uplink => 2.5,
                        _ => 1.5,
                    };
                    frame.stroke(
                        &Path::line(*from, *to),
                        canvas::Stroke::default()
                            .with_width(width)
                            .with_color(with_alpha(status_color(link.state, &self.palette), 0.72)),
                    );
                }
            }
            for node in &self.nodes {
                let Some(position) = positions.get(&node.id) else {
                    continue;
                };
                let semantic = status_color(node.state, &self.palette);
                let shape = match node.kind {
                    NetworkNodeKind::Host => Path::circle(*position, 6.0),
                    NetworkNodeKind::Device => Path::rounded_rectangle(
                        Point::new(position.x - 7.0, position.y - 6.0),
                        Size::new(14.0, 12.0),
                        2.0.into(),
                    ),
                    NetworkNodeKind::Gateway => Path::new(|path| {
                        path.move_to(Point::new(position.x, position.y - 8.0));
                        path.line_to(Point::new(position.x + 8.0, position.y));
                        path.line_to(Point::new(position.x, position.y + 8.0));
                        path.line_to(Point::new(position.x - 8.0, position.y));
                        path.close();
                    }),
                };
                frame.fill(&shape, semantic);
                frame.fill_text(canvas::Text {
                    content: node.label.clone(),
                    position: Point::new(position.x + 11.0, position.y + 4.0),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                let physical = if node.sites.is_empty() {
                    node.detail.clone()
                } else {
                    Some(match &node.detail {
                        Some(detail) => format!("{} · {detail}", node.sites.join(", ")),
                        None => node.sites.join(", "),
                    })
                };
                if let Some(physical) = physical {
                    frame.fill_text(canvas::Text {
                        content: physical,
                        position: Point::new(position.x + 11.0, position.y + 17.0),
                        color: color(self.palette.text_muted),
                        size: iced::Pixels((self.label_size - 2.0).max(8.0)),
                        ..Default::default()
                    });
                }
            }
        })
    }
}

struct DawDeckProgram {
    playing: bool,
    recording: bool,
    position_ticks: u64,
    cycle_ticks: u64,
    tracks: Vec<DawTrack>,
    notes: Vec<DawNote>,
    palette: ThemePalette,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for DawDeckProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.tracks.iter().fold(
            Key::new()
                .add_u64(self.position_ticks)
                .add_u64(self.cycle_ticks)
                .add_usize(self.notes.len()),
            |key, track| key.add_str(&track.label).add_f32(track.level),
        );
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let ink = color(self.palette.text);
            let muted = color(self.palette.text_muted);
            let accent = color(self.palette.primary);
            let playhead = if self.recording {
                color(self.palette.danger)
            } else {
                accent
            };
            let phase = self.position_ticks as f32 / self.cycle_ticks.max(1) as f32;

            let reel_y = bounds.height * 0.23;
            let reel_r = (bounds.height * 0.13).clamp(26.0, 58.0);
            for center_x in [bounds.width * 0.18, bounds.width * 0.38] {
                let center = Point::new(center_x, reel_y);
                frame.stroke(
                    &Path::circle(center, reel_r),
                    canvas::Stroke::default().with_width(2.0).with_color(muted),
                );
                frame.stroke(
                    &Path::circle(center, reel_r * 0.32),
                    canvas::Stroke::default().with_width(2.0).with_color(accent),
                );
                for spoke in 0..3 {
                    let angle =
                        phase * std::f32::consts::TAU + spoke as f32 * std::f32::consts::TAU / 3.0;
                    let end = Point::new(
                        center.x + angle.cos() * reel_r * 0.82,
                        center.y + angle.sin() * reel_r * 0.82,
                    );
                    frame.stroke(
                        &Path::line(center, end),
                        canvas::Stroke::default()
                            .with_width(3.0)
                            .with_color(with_alpha(accent, if self.playing { 0.9 } else { 0.5 })),
                    );
                }
                frame.fill(&Path::circle(center, 4.0), accent);
            }
            frame.stroke(
                &Path::line(
                    Point::new(bounds.width * 0.18 + reel_r, reel_y),
                    Point::new(bounds.width * 0.38 - reel_r, reel_y),
                ),
                canvas::Stroke::default().with_width(1.0).with_color(muted),
            );

            let roll = Rectangle::new(
                Point::new(14.0, bounds.height * 0.48),
                Size::new(bounds.width * 0.65, bounds.height * 0.47),
            );
            for step in 0..=16 {
                let x = roll.x + roll.width * step as f32 / 16.0;
                frame.stroke(
                    &Path::line(Point::new(x, roll.y), Point::new(x, roll.y + roll.height)),
                    canvas::Stroke::default()
                        .with_width(if step % 4 == 0 { 1.0 } else { 0.5 })
                        .with_color(with_alpha(muted, if step % 4 == 0 { 0.38 } else { 0.16 })),
                );
            }
            for lane in 0..=12 {
                let y = roll.y + roll.height * lane as f32 / 12.0;
                frame.stroke(
                    &Path::line(Point::new(roll.x, y), Point::new(roll.x + roll.width, y)),
                    canvas::Stroke::default()
                        .with_width(0.5)
                        .with_color(with_alpha(muted, 0.14)),
                );
            }
            let min_note = self.notes.iter().map(|note| note.note).min().unwrap_or(36);
            let max_note = self
                .notes
                .iter()
                .map(|note| note.note)
                .max()
                .unwrap_or(84)
                .max(min_note + 1);
            for note in &self.notes {
                let cycle = self.cycle_ticks.max(1);
                let tick = note.tick % cycle;
                let x = roll.x + roll.width * tick as f32 / cycle as f32;
                let width = (roll.width * note.length_ticks.min(cycle) as f32 / cycle as f32)
                    .max(3.0);
                let pitch = (note.note - min_note) as f32 / (max_note - min_note) as f32;
                let y = roll.y + roll.height * (1.0 - pitch);
                let rect = Path::rounded_rectangle(
                    Point::new(x, y - 4.0),
                    Size::new(width, 7.0),
                    2.0.into(),
                );
                let note_color = match note.track.parse::<usize>().unwrap_or(0) % 4 {
                    0 => accent,
                    1 => color(self.palette.info),
                    2 => color(self.palette.success),
                    _ => color(self.palette.warning),
                };
                frame.fill(
                    &rect,
                    with_alpha(note_color, 0.35 + note.velocity as f32 / 127.0 * 0.65),
                );
                let overflow = x + width - (roll.x + roll.width);
                if overflow > 0.0 {
                    let wrapped = Path::rounded_rectangle(
                        Point::new(roll.x, y - 4.0),
                        Size::new(overflow, 7.0),
                        2.0.into(),
                    );
                    frame.fill(
                        &wrapped,
                        with_alpha(note_color, 0.35 + note.velocity as f32 / 127.0 * 0.65),
                    );
                }
            }
            let head_x = roll.x + roll.width * phase;
            frame.stroke(
                &Path::line(
                    Point::new(head_x, roll.y),
                    Point::new(head_x, roll.y + roll.height),
                ),
                canvas::Stroke::default()
                    .with_width(2.0)
                    .with_color(playhead),
            );

            let meter_x = bounds.width * 0.72;
            let meter_top = 24.0;
            let meter_height = bounds.height * 0.68;
            let shown = self.tracks.len().min(10);
            let slot = ((bounds.width - meter_x - 12.0) / shown.max(1) as f32).max(16.0);
            for (index, track) in self.tracks.iter().take(shown).enumerate() {
                let x = meter_x + index as f32 * slot;
                let h = meter_height * track.level.clamp(0.0, 1.0);
                let bar = Path::rounded_rectangle(
                    Point::new(x, meter_top + meter_height - h),
                    Size::new((slot - 6.0).max(4.0), h),
                    2.0.into(),
                );
                frame.fill(
                    &bar,
                    if track.clipping {
                        color(self.palette.danger)
                    } else if track.level > 0.82 {
                        color(self.palette.warning)
                    } else {
                        color(self.palette.success)
                    },
                );
                frame.fill_text(canvas::Text {
                    content: track.label.clone(),
                    position: Point::new(x, meter_top + meter_height + 15.0),
                    color: ink,
                    size: iced::Pixels((self.label_size - 1.0).max(8.0)),
                    ..Default::default()
                });
            }
        })
    }
}

struct NeuralGraphProgram {
    nodes: Vec<NeuralNode>,
    edges: Vec<NeuralEdge>,
    palette: ThemePalette,
    label_size: f32,
    dag: bool,
}

impl<Message> canvas::Program<Message> for NeuralGraphProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self
            .nodes
            .iter()
            .fold(Key::new().add_usize(self.edges.len()), |key, node| {
                key.add_str(&node.id)
                    .add_str(&node.label)
                    .add_f32(node.heat)
                    .add_u64(node.state as u64)
            });
        let key = self.edges.iter().fold(key, |key, edge| {
            key.add_str(&edge.from).add_str(&edge.to).add_f32(edge.heat)
        });
        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            if self.nodes.is_empty() {
                frame.fill_text(canvas::Text {
                    content: "Waiting for neural activity".into(),
                    position: Point::new(12.0, bounds.height * 0.5),
                    color: color(self.palette.text_muted),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                return;
            }

            let positions = if self.dag {
                dag_layout(&self.nodes, &self.edges, bounds.size())
            } else {
                neural_layout(&self.nodes, bounds.size())
            };
            let idle_edge = with_alpha(color(self.palette.border), 0.42);
            for edge in &self.edges {
                let (Some(from), Some(to)) = (positions.get(&edge.from), positions.get(&edge.to))
                else {
                    continue;
                };
                let active = lerp_oklch(
                    idle_edge,
                    color(self.palette.primary),
                    edge.heat.clamp(0.0, 1.0),
                );
                let path = if self.dag {
                    Path::new(|path| {
                        let mid = (from.y + to.y) * 0.5;
                        path.move_to(*from);
                        path.line_to(Point::new(from.x, mid));
                        path.line_to(Point::new(to.x, mid));
                        path.line_to(*to);
                    })
                } else {
                    Path::line(*from, *to)
                };
                frame.stroke(
                    &path,
                    canvas::Stroke::default()
                        .with_width(1.0 + edge.heat * 3.0)
                        .with_color(active),
                );
                if edge.heat > 0.02 {
                    let pulse = Point::new(
                        from.x + (to.x - from.x) * edge.heat,
                        from.y + (to.y - from.y) * edge.heat,
                    );
                    frame.fill(
                        &Path::circle(pulse, 2.5 + edge.heat * 3.5),
                        with_alpha(color(self.palette.primary), 0.7 + edge.heat * 0.3),
                    );
                }
            }

            for node in &self.nodes {
                let Some(position) = positions.get(&node.id) else {
                    continue;
                };
                let semantic = status_color(node.state, &self.palette);
                let heat = node.heat.clamp(0.0, 1.0);
                let radius = 5.0 + heat * 7.0;
                if self.dag {
                    let node_size = Size::new(108.0, 32.0);
                    let origin = Point::new(
                        position.x - node_size.width * 0.5,
                        position.y - node_size.height * 0.5,
                    );
                    let shape = Path::rounded_rectangle(origin, node_size, 3.0.into());
                    frame.fill(&shape, color(self.palette.surface_muted));
                    frame.stroke(
                        &shape,
                        canvas::Stroke::default()
                            .with_width(1.0 + heat * 2.0)
                            .with_color(semantic),
                    );
                    frame.fill_text(canvas::Text {
                        content: node.label.clone(),
                        position: Point::new(origin.x + 8.0, position.y + 4.0),
                        color: color(self.palette.text),
                        size: iced::Pixels(self.label_size),
                        ..Default::default()
                    });
                    continue;
                }
                if heat > 0.02 {
                    frame.fill(
                        &Path::circle(*position, radius + 7.0 * heat),
                        with_alpha(semantic, 0.08 + heat * 0.16),
                    );
                }
                if node.id.starts_with("site:") {
                    frame.stroke(
                        &Path::circle(*position, radius + 11.0),
                        canvas::Stroke::default()
                            .with_width(1.0)
                            .with_color(with_alpha(semantic, 0.38)),
                    );
                }
                frame.fill(
                    &Path::circle(*position, radius),
                    lerp_oklch(
                        color(self.palette.surface_muted),
                        semantic,
                        0.3 + heat * 0.7,
                    ),
                );
                frame.fill_text(canvas::Text {
                    content: node.label.clone(),
                    position: Point::new(
                        (position.x + radius + 5.0).min((bounds.width - 112.0).max(4.0)),
                        (position.y + 4.0).min(bounds.height - 4.0),
                    ),
                    color: color(self.palette.text),
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        })
    }
}

/// Deterministic grouped columns: stable input order within each first-seen
/// group, with ungrouped nodes sharing the final/default column.
fn neural_layout(nodes: &[NeuralNode], size: Size) -> std::collections::BTreeMap<String, Point> {
    if nodes.iter().any(|node| node.id.starts_with("site:")) {
        return topology_web_layout(nodes, size);
    }
    grouped_column_layout(nodes, size)
}

fn grouped_column_layout(
    nodes: &[NeuralNode],
    size: Size,
) -> std::collections::BTreeMap<String, Point> {
    let mut groups: Vec<(String, Vec<&NeuralNode>)> = Vec::new();
    for node in nodes {
        let group = node.group.as_deref().unwrap_or("nodes");
        if let Some((_, members)) = groups.iter_mut().find(|(name, _)| name == group) {
            members.push(node);
        } else {
            groups.push((group.to_owned(), vec![node]));
        }
    }
    let margin_x = 24.0;
    let margin_y = 22.0;
    let usable_w = (size.width - margin_x * 2.0).max(1.0);
    let usable_h = (size.height - margin_y * 2.0).max(1.0);
    let mut positions = std::collections::BTreeMap::new();
    for (column, (_, members)) in groups.iter().enumerate() {
        let x = if groups.len() == 1 {
            size.width * 0.38
        } else {
            margin_x + usable_w * column as f32 / (groups.len() - 1) as f32
        };
        for (row, node) in members.iter().enumerate() {
            let y = margin_y + usable_h * (row + 1) as f32 / (members.len() + 1) as f32;
            positions.insert(node.id.clone(), Point::new(x, y));
        }
    }
    positions
}

/// Deterministic hub-and-orbit layout for topology snapshots. A wide, shallow
/// wall surface needs real radial separation; grouped columns collapse into a
/// card row and make site-to-peer edges unreadable.
fn topology_web_layout(
    nodes: &[NeuralNode],
    size: Size,
) -> std::collections::BTreeMap<String, Point> {
    let sites = nodes
        .iter()
        .filter(|node| node.id.starts_with("site:"))
        .collect::<Vec<_>>();
    let mut positions = std::collections::BTreeMap::new();
    let cell_width = size.width / sites.len().max(1) as f32;
    for (site_index, site) in sites.iter().enumerate() {
        let center = Point::new(
            cell_width * (site_index as f32 + 0.5),
            size.height * 0.5,
        );
        positions.insert(site.id.clone(), center);
        let site_name = site.id.trim_start_matches("site:");
        let members = nodes
            .iter()
            .filter(|node| {
                !node.id.starts_with("site:")
                    && node.group.as_deref() == Some(site_name)
            })
            .collect::<Vec<_>>();
        let radius_x = (cell_width * 0.38).min(220.0).max(72.0);
        let radius_y = (size.height * 0.32).max(34.0);
        for (index, node) in members.iter().enumerate() {
            let angle = -std::f32::consts::FRAC_PI_2
                + std::f32::consts::TAU * index as f32 / members.len().max(1) as f32;
            positions.insert(
                node.id.clone(),
                Point::new(
                    center.x + angle.cos() * radius_x,
                    center.y + angle.sin() * radius_y,
                ),
            );
        }
    }
    let unplaced = nodes
        .iter()
        .filter(|node| !positions.contains_key(&node.id))
        .collect::<Vec<_>>();
    for (index, node) in unplaced.iter().enumerate() {
        positions.insert(
            node.id.clone(),
            Point::new(
                32.0 + index as f32 * 96.0,
                (size.height - 24.0).max(24.0),
            ),
        );
    }
    positions
}

/// Stable top-down lineage layout. Depth is the longest parent path so a node
/// with multiple parents never appears above one of its dependencies.
fn dag_layout(
    nodes: &[NeuralNode],
    edges: &[NeuralEdge],
    size: Size,
) -> std::collections::BTreeMap<String, Point> {
    let mut depth = nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect::<std::collections::BTreeMap<_, _>>();
    for _ in 0..nodes.len() {
        let mut changed = false;
        for edge in edges {
            let next = depth[edge.from.as_str()] + 1;
            let target = depth.get_mut(edge.to.as_str()).expect("validated DAG edge");
            if next > *target {
                *target = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let margin_x = 70.0;
    let margin_y = 28.0;
    let usable_w = (size.width - margin_x * 2.0).max(1.0);
    let usable_h = (size.height - margin_y * 2.0).max(1.0);
    let mut positions = std::collections::BTreeMap::new();
    for level in 0..=max_depth {
        let members = nodes
            .iter()
            .filter(|node| depth[node.id.as_str()] == level)
            .collect::<Vec<_>>();
        for (column, node) in members.iter().enumerate() {
            let x = margin_x + usable_w * (column + 1) as f32 / (members.len() + 1) as f32;
            let y = if max_depth == 0 {
                size.height * 0.5
            } else {
                margin_y + usable_h * level as f32 / max_depth as f32
            };
            positions.insert(node.id.clone(), Point::new(x, y));
        }
    }
    positions
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
    let accent = data_state_color(spec.data.state, &visual.palette);
    let compact = spec.effective_chrome() == WidgetChrome::Frameless;
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
    let mut layout = column![].spacing(if compact {
        metrics.space_1
    } else {
        metrics.space_2
    });
    // The badge sits above the body and is drawn in every state, including
    // Ready: a demo widget that has data is exactly the one that would
    // otherwise pass for real.
    if let Some(badge) = provenance_badge(spec, visual) {
        layout = layout.push(badge);
    }
    layout = layout.push(content);
    if !compact {
        if let Some(footer) = data_footer(spec, visual) {
            layout = layout.push(footer);
        }
    }
    let rail = container(space())
        .width(Length::Fill)
        .height(Length::Fixed(3.0))
        .style(move |_| container::Style {
            background: Some(
                Color {
                    a: if spec.data.state == WidgetDataState::Ready {
                        0.78
                    } else {
                        1.0
                    },
                    ..accent
                }
                .into(),
            ),
            border: iced::Border {
                radius: metrics.radius_sm.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    container(column![rail, layout].spacing(if compact {
        metrics.space_1
    } else {
        metrics.space_3
    }))
    .padding(if compact {
        metrics.space_2
    } else {
        metrics.space_4
    })
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

/// Opacity applied to stale values in addition to the chroma drain, so staleness
/// is visible at a glance from across a room, not just in the footer label.
const STALE_VALUE_ALPHA: f32 = 0.55;

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

/// Lay chart legends out from the width they actually receive.
///
/// A fixed horizontal row either steals plot height on a narrow surface or
/// leaves a tiny orphaned label beside a large plot. Greedy rows keep the
/// swatch and label together while using theme typography for the estimate.
fn responsive_series_legend<'a, Message: 'a>(
    labels: Vec<String>,
    colors: Vec<Color>,
    metrics: ThemeMetrics,
    muted: Color,
) -> Element<'a, Message> {
    responsive(move |bounds| {
        let mut rows: Vec<Vec<(String, Color)>> = vec![Vec::new()];
        let mut used = 0.0;
        for (index, label) in labels.iter().enumerate() {
            let estimated = 7.0
                + metrics.space_1
                + label.chars().count() as f32 * metrics.small * 0.58
                + metrics.space_3;
            if used > 0.0 && used + estimated > bounds.width {
                rows.push(Vec::new());
                used = 0.0;
            }
            rows.last_mut()
                .expect("legend always has one row")
                .push((label.clone(), colors[index % colors.len()]));
            used += estimated;
        }
        let mut layout = column![].spacing(metrics.space_1);
        for items in rows {
            let mut line = row![].spacing(metrics.space_3);
            for (label, swatch) in items {
                line = line.push(
                    row![
                        status_dot(swatch, 7.0),
                        text(label).size(metrics.small).color(muted)
                    ]
                    .spacing(metrics.space_1),
                );
            }
            layout = layout.push(line);
        }
        layout.into()
    })
    .height(Length::Shrink)
    .into()
}

fn metric_pair<'a, Message: 'a>(
    label: &'static str,
    value: String,
    metrics: ThemeMetrics,
    palette: &ThemePalette,
) -> Element<'a, Message> {
    column![
        text(label)
            .size(metrics.micro)
            .color(color(palette.text_muted)),
        text(value).size(metrics.small).color(color(palette.text)),
    ]
    .spacing(metrics.space_1)
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

/// Distinct colours a series is identified by, supplied by the resolved theme
/// or derived from its semantic palette for backward compatibility.
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
pub fn series_colors(visual: &VisualTheme) -> Vec<Color> {
    if let Some(colors) = visual.categorical {
        return colors.into_iter().map(color).collect();
    }
    let palette = &visual.palette;
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

struct SpectralWaterfallProgram {
    rows: Vec<Vec<f32>>,
    ramp: Vec<Color>,
    label: Color,
    scan: Color,
    label_size: f32,
    min_frequency: f64,
    max_frequency: f64,
    unit: String,
}

fn sample_spectrum(rows: &[Vec<f32>], x: f32, y: f32) -> f32 {
    let source_y = y.clamp(0.0, 1.0) * (rows.len().saturating_sub(1)) as f32;
    let y0 = source_y.floor() as usize;
    let y1 = (y0 + 1).min(rows.len().saturating_sub(1));
    let fy = source_y.fract();
    let sample_row = |row: &[f32]| {
        if row.is_empty() {
            return 0.0;
        }
        let source_x = x.clamp(0.0, 1.0) * (row.len().saturating_sub(1)) as f32;
        let x0 = source_x.floor() as usize;
        let x1 = (x0 + 1).min(row.len().saturating_sub(1));
        row[x0] + (row[x1] - row[x0]) * source_x.fract()
    };
    let top = sample_row(&rows[y0]);
    let bottom = sample_row(&rows[y1]);
    top + (bottom - top) * fy
}

impl<Message> canvas::Program<Message> for SpectralWaterfallProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.rows.iter().flatten().fold(
            Key::new()
                .add_usize(self.rows.len())
                .add_f64(self.min_frequency)
                .add_f64(self.max_frequency)
                .add_str(&self.unit)
                .add_color(self.label)
                .add_f32(self.label_size),
            |key, value| key.add_f32(*value),
        );
        let key = self
            .ramp
            .iter()
            .fold(key, |key, color| key.add_color(*color))
            .build();
        let mut output = state.draw_cached(renderer, bounds.size(), key, |frame| {
            if self.rows.is_empty() || self.ramp.len() < 2 {
                frame.fill_text(canvas::Text {
                    content: "Waiting for spectrum frames".into(),
                    position: Point::new(8.0, bounds.height * 0.5),
                    color: self.label,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                return;
            }
            let layout = PlotFrame::bottom_axis(bounds.size(), self.label_size);
            // Source frames are intentionally bounded and may be sparse. The
            // retained renderer resamples them into a dense field so a dozen
            // bins do not become a dozen giant boxes on a large wall.
            let columns = ((layout.plot.width / 3.0).ceil() as usize).clamp(24, 384);
            let display_rows = ((layout.plot.height / 2.5).ceil() as usize).clamp(16, 192);
            let cell_w = layout.plot.width / columns as f32;
            let cell_h = layout.plot.height / display_rows as f32;
            for row_index in 0..display_rows {
                let y = layout.plot.y + row_index as f32 * cell_h;
                for column in 0..columns {
                    let value = sample_spectrum(
                        &self.rows,
                        (column as f32 + 0.5) / columns as f32,
                        (row_index as f32 + 0.5) / display_rows as f32,
                    );
                    let scaled = value.clamp(0.0, 1.0) * (self.ramp.len() - 1) as f32;
                    let index = scaled.floor() as usize;
                    let next = (index + 1).min(self.ramp.len() - 1);
                    let fill = lerp_oklch(self.ramp[index], self.ramp[next], scaled.fract());
                    frame.fill(
                        &Path::rectangle(
                            Point::new(layout.plot.x + column as f32 * cell_w, y),
                            Size::new(cell_w + 0.35, cell_h + 0.35),
                        ),
                        fill,
                    );
                }
            }
            for (fraction, frequency) in [
                (0.0, self.min_frequency),
                (0.5, (self.min_frequency + self.max_frequency) * 0.5),
                (1.0, self.max_frequency),
            ] {
                let label = format!("{} {}", short_value(frequency), self.unit);
                let width = label.chars().count() as f32 * self.label_size * 0.52;
                frame.fill_text(canvas::Text {
                    content: label,
                    position: Point::new(
                        layout.plot.x + fraction * layout.plot.width - fraction * width,
                        layout.axis_baseline,
                    ),
                    color: self.label,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        });
        if !self.rows.is_empty() {
            let phase = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f32()
                .rem_euclid(3.0)
                / 3.0;
            let mut overlay = canvas::Frame::new(renderer, bounds.size());
            let layout = PlotFrame::bottom_axis(bounds.size(), self.label_size);
            let y = layout.plot.y + phase * layout.plot.height;
            overlay.stroke(
                &Path::line(
                    Point::new(layout.plot.x, y),
                    Point::new(layout.plot.x + layout.plot.width, y),
                ),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(with_alpha(self.scan, 0.28)),
            );
            output.push(overlay.into_geometry());
        }
        output
    }
}

struct HeatmapProgram {
    rows: Vec<String>,
    columns: Vec<String>,
    values: Vec<Vec<f64>>,
    style: HeatmapStyle,
    low: Color,
    high: Color,
    label: Color,
    /// Resolved from the theme at construction, like the colours above: a
    /// canvas draws its own text and has no access to the type scale.
    label_size: f32,
}

fn heatmap_value_key(values: &[Vec<f64>]) -> u64 {
    values
        .iter()
        .flatten()
        .map(|value| value.to_bits())
        .fold(0u64, u64::wrapping_add)
}

impl<Message> canvas::Program<Message> for HeatmapProgram {
    type State = CacheState;
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        // Float bit patterns are fingerprint material, not arithmetic. Normal
        // addition overflows quickly in debug builds for ordinary positive
        // values (the catalog's activity grid caught this live).
        let val_sum = heatmap_value_key(&self.values);
        let key = Key::new()
            .add_usize(self.columns.len())
            .add_usize(self.rows.len())
            .add_usize(self.values.iter().flatten().count())
            .add_u64(val_sum)
            .add_usize(self.style as usize)
            .add_color(self.low)
            .add_color(self.high)
            .add_color(self.label)
            .add_f32(self.label_size)
            .build();
        let mut output = state.draw_cached(renderer, bounds.size(), key, |frame| {
            if self.style == HeatmapStyle::Activity {
                self.draw_activity(frame, bounds);
                return;
            }
            let label_width = (bounds.width * 0.18).clamp(42.0, 110.0);
            let top = 18.0;
            // A color scale lives at the bottom so a cell's colour reads back
            // to a value — a heatmap with no ramp is a picture, not a reading.
            let scale_h: f32 = 20.0;
            let scale_gap: f32 = 4.0;
            let plot_bottom = (bounds.height - top - scale_h - scale_gap).max(top);
            let cell_w = ((bounds.width - label_width) / self.columns.len() as f32).max(1.0);
            let cell_h = ((plot_bottom - top) / self.rows.len() as f32).max(1.0);
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
                    let fill = lerp_oklch(self.low, self.high, t);
                    frame.fill(
                        &Path::rectangle(
                            Point::new(label_width + column as f32 * cell_w + 1.0, y + 1.0),
                            Size::new((cell_w - 2.0).max(1.0), (cell_h - 2.0).max(1.0)),
                        ),
                        fill,
                    );
                }
            }
            // Color scale: a perceptual ramp (same lerp as the cells, so the
            // strip is an honest map) with min/mid/max labels.
            let strip_x = label_width;
            let strip_y = bounds.height - scale_h;
            let strip_w = (bounds.width - label_width).max(1.0);
            let strip_h: f32 = 14.0;
            let stops = 16usize;
            let step = strip_w / stops as f32;
            for index in 0..stops {
                let t = index as f32 / (stops - 1) as f32;
                let color = lerp_oklch(self.low, self.high, t);
                frame.fill(
                    &Path::rectangle(
                        Point::new(
                            strip_x + index as f32 * step,
                            strip_y + (scale_h - strip_h) / 2.0,
                        ),
                        Size::new(step, strip_h),
                    ),
                    color,
                );
            }
            let mid = (min + max) / 2.0;
            let label_y = strip_y + scale_h + 2.0;
            for (value, justify) in [(min, 0.0f32), (mid, 0.5), (max, 1.0)] {
                let text = short_value(value);
                let width = text.chars().count() as f32 * 4.5;
                let x = strip_x + justify * strip_w - width * justify;
                frame.fill_text(canvas::Text {
                    content: text,
                    position: Point::new(x, label_y),
                    color: self.label,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
            }
        });
        if self.style == HeatmapStyle::Activity {
            return output;
        }
        let Some(position) = cursor.position_in(bounds) else {
            return output;
        };
        let label_width = (bounds.width * 0.18).clamp(42.0, 110.0);
        let top = 18.0;
        let plot_bottom = (bounds.height - top - 24.0).max(top);
        let cell_w = ((bounds.width - label_width) / self.columns.len() as f32).max(1.0);
        let cell_h = ((plot_bottom - top) / self.rows.len() as f32).max(1.0);
        if position.x < label_width || position.y < top || position.y >= plot_bottom {
            return output;
        }
        let column = ((position.x - label_width) / cell_w) as usize;
        let row = ((position.y - top) / cell_h) as usize;
        let (Some(row_label), Some(column_label), Some(value)) = (
            self.rows.get(row),
            self.columns.get(column),
            self.values.get(row).and_then(|values| values.get(column)),
        ) else {
            return output;
        };
        let mut overlay = canvas::Frame::new(renderer, bounds.size());
        let content = format!("{row_label} × {column_label}  {}", short_value(*value));
        let width = (content.chars().count() as f32 * self.label_size * 0.56 + 18.0)
            .clamp(120.0, bounds.width * 0.75);
        let origin = Point::new(
            (position.x + 10.0).min((bounds.width - width).max(0.0)),
            (position.y - self.label_size * 2.4).max(0.0),
        );
        overlay.fill(
            &Path::rounded_rectangle(origin, Size::new(width, self.label_size * 2.0), 5.0.into()),
            self.low,
        );
        overlay.fill_text(canvas::Text {
            content,
            position: Point::new(origin.x + 9.0, origin.y + self.label_size * 1.35),
            color: self.label,
            size: iced::Pixels(self.label_size),
            ..Default::default()
        });
        output.push(overlay.into_geometry());
        output
    }
}

impl HeatmapProgram {
    fn draw_activity(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let rows = self.rows.len().max(1);
        let columns = self.columns.len().max(1);
        let label_width = (bounds.width * 0.12).clamp(28.0, 62.0);
        let gap = 3.0;
        let cell = ((bounds.width - label_width - gap * columns.saturating_sub(1) as f32)
            / columns as f32)
            .min((bounds.height - 22.0 - gap * rows.saturating_sub(1) as f32) / rows as f32)
            .max(2.0);
        let min = self
            .values
            .iter()
            .flatten()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let max = self
            .values
            .iter()
            .flatten()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        for (row, (label, values)) in self.rows.iter().zip(&self.values).enumerate() {
            let y = 18.0 + row as f32 * (cell + gap);
            frame.fill_text(canvas::Text {
                content: label.clone(),
                position: Point::new(0.0, y + cell * 0.72),
                color: self.label,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            for (column, value) in values.iter().enumerate() {
                let raw = ((*value - min) / (max - min).max(f64::EPSILON)) as f32;
                let level = (raw.clamp(0.0, 1.0) * 4.0).ceil() / 4.0;
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(label_width + column as f32 * (cell + gap), y),
                        Size::new(cell, cell),
                        (cell * 0.16).into(),
                    ),
                    lerp_oklch(self.low, self.high, level),
                );
            }
        }
        let stride = (columns / 5).max(1);
        for (column, label) in self.columns.iter().enumerate().step_by(stride) {
            frame.fill_text(canvas::Text {
                content: label.clone(),
                position: Point::new(label_width + column as f32 * (cell + gap), 10.0),
                color: self.label,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
        }
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

struct SankeyProgram {
    nodes: Vec<(String, String, Option<Color>)>,
    links: Vec<(String, String, f32)>,
    colors: Vec<Color>,
    label: Color,
    surface: Color,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for SankeyProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self
            .links
            .iter()
            .fold(
                self.colors.iter().fold(
                    Key::new()
                        .add_usize(self.nodes.len())
                        .add_color(self.label)
                        .add_color(self.surface),
                    |key, color| key.add_color(*color),
                ),
                |key, (from, to, value)| key.add_str(from).add_str(to).add_f32(*value),
            )
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let count = self.nodes.len().max(1);
            let max_link = self
                .links
                .iter()
                .map(|link| link.2)
                .fold(0.0, f32::max)
                .max(f32::EPSILON);
            let mut positions = std::collections::HashMap::new();
            for (index, (id, _, _)) in self.nodes.iter().enumerate() {
                let incoming = self.links.iter().any(|(_, to, _)| to == id);
                let outgoing = self.links.iter().any(|(from, _, _)| from == id);
                let x = match (incoming, outgoing) {
                    (false, true) => bounds.width * 0.08,
                    (true, false) => bounds.width * 0.92,
                    _ => bounds.width * 0.5,
                };
                let y = bounds.height * (index as f32 + 0.5) / count as f32;
                positions.insert(id.as_str(), Point::new(x, y));
            }
            for (index, (from, to, value)) in self.links.iter().enumerate() {
                let (Some(start), Some(end)) =
                    (positions.get(from.as_str()), positions.get(to.as_str()))
                else {
                    continue;
                };
                frame.stroke(
                    &Path::line(*start, *end),
                    canvas::Stroke::default()
                        .with_width(2.0 + 14.0 * (*value / max_link).sqrt())
                        .with_color(with_alpha(self.colors[index % self.colors.len()], 0.46)),
                );
            }
            for (index, (id, label, semantic)) in self.nodes.iter().enumerate() {
                let position = positions[id.as_str()];
                let fill = semantic.unwrap_or(self.colors[index % self.colors.len()]);
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(position.x - 5.0, position.y - 15.0),
                        Size::new(10.0, 30.0),
                        3.0.into(),
                    ),
                    fill,
                );
                let right = position.x < bounds.width * 0.7;
                frame.fill_text(canvas::Text {
                    content: label.clone(),
                    position: Point::new(
                        if right {
                            position.x + 10.0
                        } else {
                            position.x - 10.0
                        },
                        position.y + self.label_size * 0.35,
                    ),
                    color: self.label,
                    size: iced::Pixels(self.label_size),
                    align_x: if right {
                        iced::alignment::Horizontal::Left
                    } else {
                        iced::alignment::Horizontal::Right
                    }
                    .into(),
                    ..Default::default()
                });
            }
        })
    }
}

struct HierarchyProgram {
    style: HierarchyStyle,
    nodes: Vec<HierarchyNode>,
    focus: Option<String>,
    colors: Vec<Color>,
    text: Color,
    border: Color,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for HierarchyProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.nodes.iter().fold(
            Key::new()
                .add_usize(self.style as usize)
                .add_str(self.focus.as_deref().unwrap_or_default())
                .add_color(self.text)
                .add_color(self.border)
                .add_f32(self.label_size),
            |key, node| {
                key.add_str(&node.id)
                    .add_str(&node.label)
                    .add_str(node.parent.as_deref().unwrap_or_default())
                    .add_f64(node.value)
            },
        );
        let key = self
            .colors
            .iter()
            .fold(key, |key, color| key.add_color(*color))
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            if self.nodes.is_empty() || self.colors.is_empty() {
                return;
            }
            let roots = if let Some(focus) = self.focus.as_deref() {
                vec![focus]
            } else {
                self.nodes
                    .iter()
                    .filter(|node| node.parent.is_none())
                    .map(|node| node.id.as_str())
                    .collect()
            };
            match self.style {
                HierarchyStyle::Treemap => {
                    let rect = Rectangle::new(Point::ORIGIN, bounds.size());
                    draw_treemap_level(frame, &self.nodes, &roots, rect, 0, self);
                }
                HierarchyStyle::Sunburst => {
                    let center = Point::new(bounds.width * 0.5, bounds.height * 0.5);
                    let max_depth = roots
                        .iter()
                        .map(|root| hierarchy_depth(&self.nodes, root))
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    let ring = bounds.width.min(bounds.height) * 0.46 / max_depth as f32;
                    let total = roots
                        .iter()
                        .map(|id| hierarchy_weight(&self.nodes, id))
                        .sum::<f64>()
                        .max(f64::EPSILON);
                    let mut start = -std::f32::consts::FRAC_PI_2;
                    for root in roots {
                        let sweep = (hierarchy_weight(&self.nodes, root) / total) as f32
                            * std::f32::consts::TAU;
                        draw_sunburst_node(
                            frame,
                            &self.nodes,
                            root,
                            center,
                            0,
                            ring,
                            start,
                            sweep,
                            self,
                        );
                        start += sweep;
                    }
                }
            }
        })
    }
}

fn hierarchy_weight(nodes: &[HierarchyNode], id: &str) -> f64 {
    let children = nodes
        .iter()
        .filter(|node| node.parent.as_deref() == Some(id));
    let sum = children
        .map(|child| hierarchy_weight(nodes, &child.id))
        .sum::<f64>();
    if sum > 0.0 {
        sum
    } else {
        nodes
            .iter()
            .find(|node| node.id == id)
            .map_or(0.0, |node| node.value)
    }
}

fn hierarchy_depth(nodes: &[HierarchyNode], id: &str) -> usize {
    1 + nodes
        .iter()
        .filter(|node| node.parent.as_deref() == Some(id))
        .map(|child| hierarchy_depth(nodes, &child.id))
        .max()
        .unwrap_or(0)
}

fn draw_treemap_level(
    frame: &mut canvas::Frame,
    nodes: &[HierarchyNode],
    ids: &[&str],
    rect: Rectangle,
    depth: usize,
    program: &HierarchyProgram,
) {
    let total = ids
        .iter()
        .map(|id| hierarchy_weight(nodes, id))
        .sum::<f64>()
        .max(f64::EPSILON);
    let mut offset = 0.0;
    for (index, id) in ids.iter().enumerate() {
        let share = (hierarchy_weight(nodes, id) / total) as f32;
        let child_rect = if depth % 2 == 0 {
            Rectangle::new(
                Point::new(rect.x + rect.width * offset, rect.y),
                Size::new(rect.width * share, rect.height),
            )
        } else {
            Rectangle::new(
                Point::new(rect.x, rect.y + rect.height * offset),
                Size::new(rect.width, rect.height * share),
            )
        };
        offset += share;
        let inset = 1.5;
        let drawn = Rectangle::new(
            Point::new(child_rect.x + inset, child_rect.y + inset),
            Size::new(
                (child_rect.width - inset * 2.0).max(1.0),
                (child_rect.height - inset * 2.0).max(1.0),
            ),
        );
        frame.fill(
            &Path::rectangle(drawn.position(), drawn.size()),
            with_alpha(program.colors[(depth + index) % program.colors.len()], 0.72),
        );
        frame.stroke(
            &Path::rectangle(drawn.position(), drawn.size()),
            canvas::Stroke::default()
                .with_width(1.0)
                .with_color(program.border),
        );
        if let Some(node) = nodes.iter().find(|node| node.id == *id) {
            if drawn.width > 48.0 && drawn.height > 24.0 {
                frame.fill_text(canvas::Text {
                    content: node.label.clone(),
                    position: Point::new(drawn.x + 6.0, drawn.y + program.label_size + 5.0),
                    color: program.text,
                    size: iced::Pixels(program.label_size),
                    ..Default::default()
                });
            }
        }
        let children = nodes
            .iter()
            .filter(|node| node.parent.as_deref() == Some(*id))
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        if !children.is_empty() {
            let inner = Rectangle::new(
                Point::new(drawn.x + 4.0, drawn.y + program.label_size + 8.0),
                Size::new(
                    (drawn.width - 8.0).max(1.0),
                    (drawn.height - program.label_size - 12.0).max(1.0),
                ),
            );
            draw_treemap_level(frame, nodes, &children, inner, depth + 1, program);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sunburst_node(
    frame: &mut canvas::Frame,
    nodes: &[HierarchyNode],
    id: &str,
    center: Point,
    depth: usize,
    ring: f32,
    start: f32,
    sweep: f32,
    program: &HierarchyProgram,
) {
    let inner = depth as f32 * ring;
    let outer = inner + ring - 1.5;
    let steps = ((sweep.abs() * 28.0).ceil() as usize).max(3);
    let path = Path::new(|builder| {
        for step in 0..=steps {
            let angle = start + sweep * step as f32 / steps as f32;
            let point = Point::new(
                center.x + outer * angle.cos(),
                center.y + outer * angle.sin(),
            );
            if step == 0 {
                builder.move_to(point)
            } else {
                builder.line_to(point)
            }
        }
        for step in (0..=steps).rev() {
            let angle = start + sweep * step as f32 / steps as f32;
            builder.line_to(Point::new(
                center.x + inner * angle.cos(),
                center.y + inner * angle.sin(),
            ));
        }
        builder.close();
    });
    frame.fill(
        &path,
        with_alpha(program.colors[depth % program.colors.len()], 0.78),
    );
    frame.stroke(
        &path,
        canvas::Stroke::default()
            .with_width(1.0)
            .with_color(program.border),
    );
    if sweep > 0.34 && outer > 20.0 {
        if let Some(node) = nodes.iter().find(|node| node.id == id) {
            let angle = start + sweep * 0.5;
            let radius = (inner + outer) * 0.5;
            frame.fill_text(canvas::Text {
                content: node.label.clone(),
                position: Point::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                ),
                color: program.text,
                size: iced::Pixels(program.label_size),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..Default::default()
            });
        }
    }
    let children = nodes
        .iter()
        .filter(|node| node.parent.as_deref() == Some(id))
        .collect::<Vec<_>>();
    let total = children
        .iter()
        .map(|node| hierarchy_weight(nodes, &node.id))
        .sum::<f64>()
        .max(f64::EPSILON);
    let mut child_start = start;
    for child in children {
        let child_sweep = sweep * (hierarchy_weight(nodes, &child.id) / total) as f32;
        draw_sunburst_node(
            frame,
            nodes,
            &child.id,
            center,
            depth + 1,
            ring,
            child_start,
            child_sweep,
            program,
        );
        child_start += child_sweep;
    }
}

struct MetricBoardProgram {
    style: MetricBoardStyle,
    metrics: Vec<TargetMetric>,
    palette: ThemePalette,
    colors: Vec<Color>,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for MetricBoardProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.metrics.iter().fold(
            Key::new()
                .add_usize(self.style as usize)
                .add_f32(self.label_size)
                .add_color(color(self.palette.text))
                .add_color(color(self.palette.surface_muted)),
            |key, metric| {
                key.add_str(&metric.label)
                    .add_f64(metric.value)
                    .add_f64(metric.max)
                    .add_f64(metric.target.unwrap_or(-1.0))
                    .add_f64(metric.forecast.unwrap_or(-1.0))
                    .add_usize(metric.state.map_or(usize::MAX, |state| state as usize))
            },
        );
        let key = self
            .colors
            .iter()
            .fold(key, |key, value| key.add_color(*value))
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| match self.style {
            MetricBoardStyle::Radial => self.draw_radial(frame, bounds),
            MetricBoardStyle::Bullet => self.draw_bullets(frame, bounds),
        })
    }
}

impl MetricBoardProgram {
    fn draw_radial(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let columns = (self.metrics.len() as f32).sqrt().ceil() as usize;
        let rows = self.metrics.len().div_ceil(columns.max(1));
        let cell_w = bounds.width / columns.max(1) as f32;
        let cell_h = bounds.height / rows.max(1) as f32;
        for (index, metric) in self.metrics.iter().enumerate() {
            let center = Point::new(
                (index % columns) as f32 * cell_w + cell_w * 0.5,
                (index / columns) as f32 * cell_h + cell_h * 0.48,
            );
            let radius = cell_w.min(cell_h) * 0.31;
            let fraction = (metric.value / metric.max).clamp(0.0, 1.0) as f32;
            let start = std::f32::consts::PI * 0.75;
            let sweep = std::f32::consts::PI * 1.5;
            let stroke_arc = |frame: &mut canvas::Frame, end: f32, color: Color, width: f32| {
                let steps = 40usize;
                let path = Path::new(|builder| {
                    for step in 0..=steps {
                        let angle = start + end * step as f32 / steps as f32;
                        let point = Point::new(
                            center.x + radius * angle.cos(),
                            center.y + radius * angle.sin(),
                        );
                        if step == 0 {
                            builder.move_to(point)
                        } else {
                            builder.line_to(point)
                        }
                    }
                });
                frame.stroke(
                    &path,
                    canvas::Stroke::default()
                        .with_width(width)
                        .with_color(color),
                );
            };
            stroke_arc(frame, sweep, color(self.palette.surface_muted), 7.0);
            let fill = metric
                .state
                .map_or(self.colors[index % self.colors.len()], |state| {
                    status_color(state, &self.palette)
                });
            stroke_arc(frame, sweep * fraction, fill, 7.0);
            if let Some(target) = metric.target {
                let angle = start + sweep * (target / metric.max) as f32;
                frame.stroke(
                    &Path::line(
                        Point::new(
                            center.x + (radius - 7.0) * angle.cos(),
                            center.y + (radius - 7.0) * angle.sin(),
                        ),
                        Point::new(
                            center.x + (radius + 7.0) * angle.cos(),
                            center.y + (radius + 7.0) * angle.sin(),
                        ),
                    ),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(color(self.palette.text)),
                );
            }
            frame.fill_text(canvas::Text {
                content: format_metric(metric.value),
                position: Point::new(center.x, center.y + 4.0),
                color: color(self.palette.text),
                size: iced::Pixels(self.label_size * 1.45),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..Default::default()
            });
            frame.fill_text(canvas::Text {
                content: metric.label.clone(),
                position: Point::new(center.x, center.y + radius + self.label_size + 5.0),
                color: color(self.palette.text_muted),
                size: iced::Pixels(self.label_size),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..Default::default()
            });
        }
    }

    fn draw_bullets(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let label_w = (bounds.width * 0.23).clamp(54.0, 120.0);
        let row_h = bounds.height / self.metrics.len().max(1) as f32;
        for (index, metric) in self.metrics.iter().enumerate() {
            let y = index as f32 * row_h;
            let plot_x = label_w;
            let plot_w = (bounds.width - label_w - 8.0).max(1.0);
            frame.fill_text(canvas::Text {
                content: metric.label.clone(),
                position: Point::new(0.0, y + row_h * 0.58),
                color: color(self.palette.text_muted),
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            for (share, fill) in [
                (1.0, self.palette.surface_muted),
                (0.75, self.palette.border),
                (0.5, self.palette.border_strong),
            ] {
                frame.fill(
                    &Path::rectangle(
                        Point::new(plot_x, y + row_h * 0.23),
                        Size::new(plot_w * share, row_h * 0.54),
                    ),
                    with_alpha(color(fill), 0.55),
                );
            }
            let actual = (metric.value / metric.max).clamp(0.0, 1.0) as f32;
            let fill = metric
                .state
                .map_or(self.colors[index % self.colors.len()], |state| {
                    status_color(state, &self.palette)
                });
            frame.fill(
                &Path::rectangle(
                    Point::new(plot_x, y + row_h * 0.38),
                    Size::new(plot_w * actual, row_h * 0.24),
                ),
                fill,
            );
            if let Some(forecast) = metric.forecast {
                let x = plot_x + plot_w * (forecast / metric.max) as f32;
                frame.stroke(
                    &Path::line(
                        Point::new(x, y + row_h * 0.31),
                        Point::new(x, y + row_h * 0.69),
                    ),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(color(self.palette.info)),
                );
            }
            if let Some(target) = metric.target {
                let x = plot_x + plot_w * (target / metric.max) as f32;
                frame.stroke(
                    &Path::line(
                        Point::new(x, y + row_h * 0.18),
                        Point::new(x, y + row_h * 0.82),
                    ),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(color(self.palette.text)),
                );
            }
        }
    }
}

struct DistributionProgram {
    style: DistributionStyle,
    groups: Vec<DistributionGroup>,
    colors: Vec<Color>,
    text: Color,
    grid: Color,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for DistributionProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self.groups.iter().fold(
            Key::new()
                .add_usize(self.style as usize)
                .add_color(self.text)
                .add_color(self.grid)
                .add_f32(self.label_size),
            |key, group| {
                group
                    .samples
                    .iter()
                    .fold(key.add_str(&group.label), |key, value| key.add_f64(*value))
            },
        );
        let key = self
            .colors
            .iter()
            .fold(key, |key, value| key.add_color(*value))
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let min = self
                .groups
                .iter()
                .flat_map(|group| &group.samples)
                .copied()
                .fold(f64::INFINITY, f64::min);
            let max = self
                .groups
                .iter()
                .flat_map(|group| &group.samples)
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let span = (max - min).max(f64::EPSILON);
            let label_w = (bounds.width * 0.18).clamp(48.0, 110.0);
            let plot_w = (bounds.width - label_w - 8.0).max(1.0);
            let row_h = bounds.height / self.groups.len().max(1) as f32;
            let x = |value: f64| label_w + ((value - min) / span) as f32 * plot_w;
            for tick in 0..=4 {
                let tx = label_w + plot_w * tick as f32 / 4.0;
                frame.stroke(
                    &Path::line(Point::new(tx, 0.0), Point::new(tx, bounds.height)),
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(self.grid),
                );
            }
            for (index, group) in self.groups.iter().enumerate() {
                let center = index as f32 * row_h + row_h * 0.5;
                frame.fill_text(canvas::Text {
                    content: group.label.clone(),
                    position: Point::new(0.0, center + self.label_size * 0.35),
                    color: self.text,
                    size: iced::Pixels(self.label_size),
                    ..Default::default()
                });
                let mut samples = group.samples.clone();
                samples.sort_by(f64::total_cmp);
                let quantile = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
                let color = self.colors[index % self.colors.len()];
                match self.style {
                    DistributionStyle::Box => {
                        let (lo, q1, median, q3, hi) = (
                            samples[0],
                            quantile(0.25),
                            quantile(0.5),
                            quantile(0.75),
                            samples[samples.len() - 1],
                        );
                        frame.stroke(
                            &Path::line(Point::new(x(lo), center), Point::new(x(hi), center)),
                            canvas::Stroke::default().with_width(2.0).with_color(color),
                        );
                        let box_h = row_h * 0.48;
                        let rect = Path::rectangle(
                            Point::new(x(q1), center - box_h * 0.5),
                            Size::new((x(q3) - x(q1)).max(1.0), box_h),
                        );
                        frame.fill(&rect, with_alpha(color, 0.42));
                        frame.stroke(
                            &rect,
                            canvas::Stroke::default().with_width(1.5).with_color(color),
                        );
                        frame.stroke(
                            &Path::line(
                                Point::new(x(median), center - box_h * 0.5),
                                Point::new(x(median), center + box_h * 0.5),
                            ),
                            canvas::Stroke::default().with_width(2.0).with_color(color),
                        );
                    }
                    DistributionStyle::Violin => {
                        const BINS: usize = 20;
                        let mut density = [0usize; BINS];
                        for value in &samples {
                            let bin =
                                (((*value - min) / span) * (BINS - 1) as f64).round() as usize;
                            density[bin.min(BINS - 1)] += 1;
                        }
                        let peak = density.iter().copied().max().unwrap_or(1).max(1) as f32;
                        let half = row_h * 0.36;
                        let path = Path::new(|builder| {
                            for (bin, count) in density.iter().enumerate() {
                                let px = label_w + plot_w * bin as f32 / (BINS - 1) as f32;
                                let py = center - half * *count as f32 / peak;
                                if bin == 0 {
                                    builder.move_to(Point::new(px, py))
                                } else {
                                    builder.line_to(Point::new(px, py))
                                }
                            }
                            for (bin, count) in density.iter().enumerate().rev() {
                                let px = label_w + plot_w * bin as f32 / (BINS - 1) as f32;
                                builder
                                    .line_to(Point::new(px, center + half * *count as f32 / peak));
                            }
                            builder.close();
                        });
                        frame.fill(&path, with_alpha(color, 0.45));
                        frame.stroke(
                            &path,
                            canvas::Stroke::default().with_width(1.5).with_color(color),
                        );
                        frame.stroke(
                            &Path::line(
                                Point::new(x(quantile(0.5)), center - half),
                                Point::new(x(quantile(0.5)), center + half),
                            ),
                            canvas::Stroke::default().with_width(1.5).with_color(color),
                        );
                    }
                }
            }
        })
    }
}

struct SpanTimelineProgram {
    style: SpanTimelineStyle,
    spans: Vec<TimedSpan>,
    colors: Vec<Color>,
    palette: ThemePalette,
    label_size: f32,
}

impl<Message> canvas::Program<Message> for SpanTimelineProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = self
            .spans
            .iter()
            .fold(
                self.colors.iter().fold(
                    Key::new()
                        .add_usize(self.style as usize)
                        .add_color(color(self.palette.border))
                        .add_color(color(self.palette.text)),
                    |key, value| key.add_color(*value),
                ),
                |key, span| {
                    key.add_str(&span.id)
                        .add_str(&span.label)
                        .add_f64(span.start)
                        .add_f64(span.end)
                        .add_str(span.lane.as_deref().unwrap_or(""))
                        .add_str(span.parent.as_deref().unwrap_or(""))
                        .add_str(span.detail.as_deref().unwrap_or(""))
                        .add_usize(span.state.map_or(usize::MAX, |state| state as usize))
                },
            )
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let min = self
                .spans
                .iter()
                .map(|span| span.start)
                .fold(f64::INFINITY, f64::min);
            let max = self
                .spans
                .iter()
                .map(|span| span.end)
                .fold(f64::NEG_INFINITY, f64::max);
            let duration = (max - min).max(f64::EPSILON);
            let label_width = if self.style == SpanTimelineStyle::Flame {
                8.0
            } else {
                (bounds.width * 0.20).clamp(56.0, 140.0)
            };
            let plot_width = (bounds.width - label_width - 8.0).max(1.0);
            let mut lane_names = Vec::<String>::new();
            for span in &self.spans {
                let lane = span.lane.clone().unwrap_or_else(|| span.label.clone());
                if !lane_names.contains(&lane) {
                    lane_names.push(lane);
                }
            }
            let depths: std::collections::HashMap<&str, usize> = self
                .spans
                .iter()
                .map(|span| {
                    let mut depth = 0usize;
                    let mut parent = span.parent.as_deref();
                    while let Some(id) = parent {
                        depth += 1;
                        if depth > self.spans.len() {
                            break;
                        }
                        parent = self
                            .spans
                            .iter()
                            .find(|candidate| candidate.id == id)
                            .and_then(|candidate| candidate.parent.as_deref());
                    }
                    (span.id.as_str(), depth)
                })
                .collect();
            let row_count = match self.style {
                SpanTimelineStyle::Flame => depths.values().copied().max().unwrap_or(0) + 1,
                SpanTimelineStyle::Waterfall => self.spans.len(),
                SpanTimelineStyle::Gantt | SpanTimelineStyle::EventRiver => lane_names.len(),
            }
            .max(1);
            let row_height = bounds.height / row_count as f32;
            for (index, span) in self.spans.iter().enumerate() {
                let row = match self.style {
                    SpanTimelineStyle::Flame => row_count - 1 - depths[span.id.as_str()],
                    SpanTimelineStyle::Waterfall => index,
                    SpanTimelineStyle::Gantt | SpanTimelineStyle::EventRiver => {
                        let lane = span.lane.as_deref().unwrap_or(&span.label);
                        lane_names
                            .iter()
                            .position(|value| value == lane)
                            .unwrap_or(0)
                    }
                };
                let x = label_width + ((span.start - min) / duration) as f32 * plot_width;
                let width = (((span.end - span.start) / duration) as f32 * plot_width).max(
                    if self.style == SpanTimelineStyle::EventRiver {
                        4.0
                    } else {
                        2.0
                    },
                );
                let y = row as f32 * row_height + row_height * 0.16;
                let height = row_height * 0.68;
                let fill = span
                    .state
                    .map_or(self.colors[index % self.colors.len()], |value| {
                        status_color(value, &self.palette)
                    });
                if self.style == SpanTimelineStyle::EventRiver
                    && (span.end - span.start).abs() <= f64::EPSILON
                {
                    frame.fill(
                        &Path::circle(
                            Point::new(x, y + height * 0.5),
                            (height * 0.28).clamp(2.0, 7.0),
                        ),
                        fill,
                    );
                } else {
                    frame.fill(
                        &Path::rounded_rectangle(
                            Point::new(x, y),
                            Size::new(width, height),
                            (height * 0.16).into(),
                        ),
                        with_alpha(fill, 0.88),
                    );
                }
                if self.style != SpanTimelineStyle::Flame {
                    frame.fill_text(canvas::Text {
                        content: if matches!(
                            self.style,
                            SpanTimelineStyle::Gantt | SpanTimelineStyle::EventRiver
                        ) {
                            span.lane.clone().unwrap_or_else(|| span.label.clone())
                        } else {
                            span.label.clone()
                        },
                        position: Point::new(0.0, y + height * 0.72),
                        color: color(self.palette.text_muted),
                        size: iced::Pixels(self.label_size),
                        ..Default::default()
                    });
                } else if width > self.label_size * 4.0 {
                    frame.fill_text(canvas::Text {
                        content: span.label.clone(),
                        position: Point::new(x + 4.0, y + height * 0.72),
                        color: color(self.palette.background),
                        size: iced::Pixels(self.label_size),
                        ..Default::default()
                    });
                }
            }
            for tick in 0..=4 {
                let x = label_width + plot_width * tick as f32 / 4.0;
                frame.stroke(
                    &Path::line(Point::new(x, 0.0), Point::new(x, bounds.height)),
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(with_alpha(color(self.palette.border), 0.24)),
                );
            }
        })
    }
}

struct PieChartProgram {
    values: Vec<(String, f64)>,
    colors: Vec<Color>,
    text: Color,
    surface: Color,
    label_size: f32,
    unit: Option<String>,
}

impl<Message> canvas::Program<Message> for PieChartProgram {
    type State = CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let total = self
            .values
            .iter()
            .map(|(_, value)| value.max(0.0))
            .sum::<f64>();
        if total <= f64::EPSILON || self.colors.is_empty() {
            return Vec::new();
        }
        let center = Point::new(bounds.width * 0.5, bounds.height * 0.5);
        let radius = bounds.width.min(bounds.height) * 0.42;
        let inner = radius * 0.55;
        let key = self
            .values
            .iter()
            .fold(Key::new(), |key, (label, value)| {
                key.add_str(label).add_f64(*value)
            })
            .add_color(self.text)
            .add_color(self.surface)
            .add_f32(self.label_size)
            .add_usize(self.colors.len());
        let key = self
            .colors
            .iter()
            .fold(key, |key, color| key.add_color(*color))
            .build();
        let mut output = state.draw_cached(renderer, bounds.size(), key, |frame| {
            let mut start = -std::f32::consts::FRAC_PI_2;
            for (index, (_, value)) in self.values.iter().enumerate() {
                let sweep = (*value / total) as f32 * std::f32::consts::TAU;
                let steps = ((sweep.abs() * 24.0).ceil() as usize).max(2);
                let path = Path::new(|path| {
                    path.move_to(center);
                    for step in 0..=steps {
                        let angle = start + sweep * step as f32 / steps as f32;
                        path.line_to(Point::new(
                            center.x + radius * angle.cos(),
                            center.y + radius * angle.sin(),
                        ));
                    }
                    path.close();
                });
                frame.fill(&path, self.colors[index % self.colors.len()]);
                start += sweep;
            }
            frame.fill(&Path::circle(center, inner), self.surface);
            frame.fill_text(canvas::Text {
                content: format_quantity(total, self.unit.as_deref()),
                position: Point::new(center.x, center.y + self.label_size * 0.25),
                color: self.text,
                size: iced::Pixels(self.label_size * 1.15),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..Default::default()
            });
        });

        let Some(position) = cursor.position_in(bounds) else {
            return output;
        };
        let dx = position.x - center.x;
        let dy = position.y - center.y;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < inner || distance > radius {
            return output;
        }
        let angle = (dy.atan2(dx) + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::TAU);
        let mut start_angle = 0.0f32;
        let hovered = self
            .values
            .iter()
            .enumerate()
            .find_map(|(index, (label, value))| {
                let sweep = (*value / total) as f32 * std::f32::consts::TAU;
                let end = start_angle + sweep;
                let hit = angle <= end;
                let midpoint = start_angle + sweep * 0.5 - std::f32::consts::FRAC_PI_2;
                start_angle = end;
                hit.then_some((index, label, *value, midpoint))
            });
        if let Some((index, label, value, midpoint)) = hovered {
            let mut overlay = canvas::Frame::new(renderer, bounds.size());
            let detail = format!(
                "{} · {:.1}%",
                format_quantity(value, self.unit.as_deref()),
                value / total * 100.0
            );
            let label = if label.chars().count() > 34 {
                format!("{}…", label.chars().take(33).collect::<String>())
            } else {
                label.clone()
            };
            let characters = label.chars().count().max(detail.chars().count()) as f32;
            let width =
                (characters * self.label_size * 0.56 + 24.0).clamp(128.0, bounds.width * 0.72);
            // Two baselines plus real descender room; the prior 3.35× box
            // clipped the quantity line by a few pixels on high-DPI displays.
            let height = self.label_size * 3.8;
            let anchor = Point::new(
                center.x + midpoint.cos() * (radius + 10.0),
                center.y + midpoint.sin() * (radius + 10.0),
            );
            let preferred_x = if midpoint.cos() >= 0.0 {
                anchor.x + 8.0
            } else {
                anchor.x - width - 8.0
            };
            let x = preferred_x.clamp(4.0, bounds.width - width - 4.0);
            let y = (anchor.y - height * 0.5).clamp(4.0, bounds.height - height - 4.0);
            let box_path =
                Path::rounded_rectangle(Point::new(x, y), Size::new(width, height), 6.0.into());
            overlay.fill(
                &box_path,
                Color {
                    a: 0.98,
                    ..self.surface
                },
            );
            overlay.stroke(
                &box_path,
                canvas::Stroke::default().with_width(1.0).with_color(Color {
                    a: 0.55,
                    ..self.colors[index % self.colors.len()]
                }),
            );
            overlay.fill(
                &Path::rounded_rectangle(
                    Point::new(x + 6.0, y + 8.0),
                    Size::new(4.0, height - 16.0),
                    2.0.into(),
                ),
                self.colors[index % self.colors.len()],
            );
            overlay.fill_text(canvas::Text {
                content: label,
                position: Point::new(x + 18.0, y + self.label_size * 1.25),
                color: self.text,
                size: iced::Pixels(self.label_size),
                ..Default::default()
            });
            overlay.fill_text(canvas::Text {
                content: detail,
                position: Point::new(x + 18.0, y + self.label_size * 2.55),
                color: Color {
                    a: 0.78,
                    ..self.text
                },
                size: iced::Pixels(self.label_size * 0.86),
                ..Default::default()
            });
            output.push(overlay.into_geometry());
        }
        output
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
        let seg_sum: f64 = self
            .lanes
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.start + s.end)
            .sum();
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
                    let x =
                        label_w + ((segment.start - min) / span) as f32 * (bounds.width - label_w);
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

/// Interpolate in OKLCH so the ramp is perceptually uniform — a color scale
/// drawn from this must read back the same gradient the cells use. Hue takes
/// the shortest arc across 0/360 so a ramp that crosses the red seam does not
/// sweep the whole color wheel.
fn lerp_oklch(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let a = to_oklch(a);
    let b = to_oklch(b);
    let hue = |lo: f32, hi: f32| {
        let d = (hi - lo).rem_euclid(360.0);
        let d = if d > 180.0 { d - 360.0 } else { d };
        (lo + d * t).rem_euclid(360.0)
    };
    from_oklch(Oklch {
        l: a.l + (b.l - a.l) * t,
        c: a.c + (b.c - a.c) * t,
        h: hue(a.h, b.h),
    })
}

/// Compact label for the ends of a color scale, so min/mid/max stay readable
/// on a narrow strip across the wall.
fn short_value(value: f64) -> String {
    if value.abs() >= 1_000.0 {
        format!("{:.1e}", value)
    } else if value == value.trunc() {
        format!("{}", value as i64)
    } else {
        format!("{:.1}", value)
    }
}

/// 1/2/5 × 10ⁿ tick ladder (Cleveland's "nice numbers"), producing roughly
/// `target` ticks spanning [min, max]. Returns only ticks within range, so a
/// blank axis labels nothing rather than inventing values past the data.
pub fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    if target == 0 {
        return Vec::new();
    }
    let target = target.max(1);
    let span = (max - min).max(0.0);
    if span < 1e-12 || !span.is_finite() {
        return vec![min];
    }
    let raw = span / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm < 1.5 {
        mag
    } else if norm < 3.0 {
        2.0 * mag
    } else if norm < 7.0 {
        5.0 * mag
    } else {
        10.0 * mag
    };
    let mut ticks = Vec::new();
    let mut t = (min / step).floor() * step;
    while t <= max + step {
        if t >= min && t <= max {
            ticks.push(t);
        }
        t += step;
    }
    if ticks.is_empty() {
        ticks.push(min);
    }
    ticks
}

/// Unit-aware value formatting for axis ticks and legends, reusing metric
/// abbreviation (`format_metric`) and appending the declared unit when present.
pub fn format_quantity(value: f64, unit: Option<&str>) -> String {
    if unit == Some("$") {
        return format!("${value:.2}");
    }
    let mut text = format_metric(value);
    if let Some(unit) = unit.filter(|unit| !unit.is_empty()) {
        text = format!("{text} {unit}");
    }
    text
}

/// Draw min/mid/max tick labels along the left edge of a plot area and a
/// current-value legend at its top-right, so a chart can be read back to a
/// number across the wall (docs/widget-quality.md). `value` is the reading to
/// surface; for multi-series plots it is the most relevant one (e.g. the max).
#[allow(clippy::too_many_arguments)]
fn annotate_plot(
    frame: &mut canvas::Frame,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min: f64,
    max: f64,
    unit: Option<&str>,
    value: f64,
    color: Color,
    size: f32,
) {
    let span = (max - min).max(1e-9);
    for tick in nice_ticks(min, max, 3) {
        let frac = ((tick - min) / span) as f32;
        let label = format_quantity(tick, unit);
        let label_width = text_width(&label, size);
        frame.fill_text(canvas::Text {
            content: label,
            position: Point::new(
                (x - label_width - size * 0.55).max(0.0),
                y + height * (1.0 - frac) - size * 0.4,
            ),
            color,
            size: iced::Pixels(size),
            ..Default::default()
        });
    }
    let legend = format_quantity(value, unit);
    let legend_w = text_width(&legend, size);
    frame.fill_text(canvas::Text {
        content: legend,
        position: Point::new(x + width - legend_w, y + 1.0),
        color,
        size: iced::Pixels(size),
        ..Default::default()
    });
}

fn text_width(value: &str, size: f32) -> f32 {
    value.chars().count() as f32 * size * 0.58
}

/// Shared placement for plot marks and their typography.
///
/// Renderers ask this primitive for a plot rectangle instead of independently
/// guessing gutters, which keeps labels, legends, grids, and data marks from
/// occupying the same pixels as surfaces resize.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlotFrame {
    plot: Rectangle,
    axis_baseline: f32,
}

impl PlotFrame {
    fn cartesian(bounds: Size, label_size: f32, min: f64, max: f64, unit: Option<&str>) -> Self {
        let widest_tick = nice_ticks(min, max, 3)
            .into_iter()
            .map(|tick| text_width(&format_quantity(tick, unit), label_size))
            .fold(0.0, f32::max);
        let left = widest_tick + label_size * 0.75;
        let top = label_size * 1.65;
        let right = label_size * 0.4;
        let bottom = label_size * 0.5;
        Self {
            plot: Rectangle {
                x: left,
                y: top,
                width: (bounds.width - left - right).max(1.0),
                height: (bounds.height - top - bottom).max(1.0),
            },
            axis_baseline: bounds.height - 1.0,
        }
    }

    fn bottom_axis(bounds: Size, label_size: f32) -> Self {
        let bottom = label_size * 1.55;
        Self {
            plot: Rectangle {
                x: 0.0,
                y: 0.0,
                width: bounds.width.max(1.0),
                height: (bounds.height - bottom).max(1.0),
            },
            axis_baseline: bounds.height - 1.0,
        }
    }
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
    label: Color,
    label_size: f32,
    unit: Option<String>,
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
            if let Some(last) = self.values.last() {
                annotate_plot(
                    frame,
                    area.x,
                    area.y,
                    area.width,
                    area.height,
                    range.min_y as f64,
                    range.max_y as f64,
                    self.unit.as_deref(),
                    *last as f64,
                    self.label,
                    self.label_size,
                );
            }
        })
    }
}

struct GaugeProgram {
    value: f32,
    track: Color,
    fill: Color,
    radius: f32,
    label: Color,
    label_size: f32,
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
            annotate_plot(
                frame,
                0.0,
                0.0,
                bounds.width,
                bounds.height,
                0.0,
                100.0,
                None,
                self.value as f64,
                self.label,
                self.label_size,
            );
        })
    }
}

struct HistogramProgram {
    values: Vec<f32>,
    color: Color,
    grid: Color,
    label: Color,
    label_size: f32,
    unit: Option<String>,
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
            annotate_plot(
                frame,
                0.0,
                0.0,
                bounds.width,
                bounds.height,
                0.0,
                max as f64,
                self.unit.as_deref(),
                max as f64,
                self.label,
                self.label_size,
            );
        })
    }
}

struct TimeSeriesProgram {
    series: Vec<Vec<(f32, f32)>>,
    colors: Vec<Color>,
    grid: Color,
    label: Color,
    label_size: f32,
    unit: Option<String>,
}

#[derive(Default)]
struct TimeSeriesState {
    cache: Cache,
    visual_key: Cell<u64>,
    motion: RefCell<SeriesMotion>,
}

#[derive(Default)]
struct SeriesMotion {
    target_key: u64,
    from: Vec<Vec<(f32, f32)>>,
    target: Vec<Vec<(f32, f32)>>,
    started: Option<Instant>,
}

const SERIES_TRANSITION: Duration = Duration::from_millis(520);

fn series_key(series: &[Vec<(f32, f32)>]) -> u64 {
    series
        .iter()
        .fold(Key::new().add_usize(series.len()), |key, line| {
            line.iter().fold(key.add_usize(line.len()), |key, (x, y)| {
                key.add_f32(*x).add_f32(*y)
            })
        })
        .build()
}

fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

fn interpolate_line(from: &[(f32, f32)], target: &[(f32, f32)], progress: f32) -> Vec<(f32, f32)> {
    if target.is_empty() {
        return Vec::new();
    }
    let fallback = from.last().copied().unwrap_or(target[0]);
    target
        .iter()
        .enumerate()
        .map(|(index, &(tx, ty))| {
            let (fx, fy) = from.get(index).copied().unwrap_or(fallback);
            (fx + (tx - fx) * progress, fy + (ty - fy) * progress)
        })
        .collect()
}

fn interpolate_series(
    from: &[Vec<(f32, f32)>],
    target: &[Vec<(f32, f32)>],
    progress: f32,
) -> Vec<Vec<(f32, f32)>> {
    target
        .iter()
        .enumerate()
        .map(|(index, line)| {
            interpolate_line(from.get(index).map_or(&[], Vec::as_slice), line, progress)
        })
        .collect()
}

impl<Message> canvas::Program<Message> for TimeSeriesProgram {
    type State = TimeSeriesState;
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
        let target_key = series_key(&self.series);
        let now = Instant::now();
        let (displayed, animating) = {
            let mut motion = state.motion.borrow_mut();
            if motion.target_key != target_key {
                let current = motion.started.map_or_else(
                    || motion.target.clone(),
                    |started| {
                        let progress = ease_out_cubic(
                            now.saturating_duration_since(started).as_secs_f32()
                                / SERIES_TRANSITION.as_secs_f32(),
                        );
                        interpolate_series(&motion.from, &motion.target, progress)
                    },
                );
                motion.from = if current.is_empty() {
                    self.series.clone()
                } else {
                    current
                };
                motion.target = self.series.clone();
                motion.target_key = target_key;
                motion.started = Some(now);
                state.cache.clear();
            }
            let elapsed = motion.started.map_or(SERIES_TRANSITION, |started| {
                now.saturating_duration_since(started)
            });
            if elapsed >= SERIES_TRANSITION {
                motion.started = None;
                (motion.target.clone(), false)
            } else {
                let progress =
                    ease_out_cubic(elapsed.as_secs_f32() / SERIES_TRANSITION.as_secs_f32());
                (
                    interpolate_series(&motion.from, &motion.target, progress),
                    true,
                )
            }
        };
        let visual_key = self
            .colors
            .iter()
            .fold(
                Key::new()
                    .add_u64(target_key)
                    .add_color(self.grid)
                    .add_color(self.label),
                |key, color| key.add_color(*color),
            )
            .build();
        if state.visual_key.replace(visual_key) != visual_key {
            state.cache.clear();
        }
        let draw = |frame: &mut canvas::Frame| {
            // One range over every series: two series on their own scales share an
            // axis that means nothing.
            let range = PlotRange::over(displayed.iter().flatten());
            let layout = PlotFrame::cartesian(
                bounds.size(),
                self.label_size,
                range.min_y as f64,
                range.max_y as f64,
                self.unit.as_deref(),
            );
            let area = layout.plot;
            for row in 0..=4 {
                let y = area.y + area.height * row as f32 / 4.0;
                frame.stroke(
                    &Path::line(Point::new(area.x, y), Point::new(area.x + area.width, y)),
                    canvas::Stroke::default()
                        .with_width(1.0)
                        .with_color(self.grid),
                );
            }
            // The fills stack, so each is worth a share of the budget rather than
            // all of it.
            let area_alpha = AREA_ALPHA / displayed.len().max(1) as f32;
            for (index, series) in displayed.iter().enumerate() {
                let points: Vec<Point> = series
                    .iter()
                    .map(|value| range.project(*value, area))
                    .collect();
                draw_series_line(
                    frame,
                    &points,
                    area.y + area.height,
                    self.colors[index % self.colors.len()],
                    area_alpha,
                );
            }
            annotate_plot(
                frame,
                area.x,
                area.y,
                area.width,
                area.height,
                range.min_y as f64,
                range.max_y as f64,
                self.unit.as_deref(),
                range.max_y as f64,
                self.label,
                self.label_size,
            );
        };
        if animating {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            draw(&mut frame);
            vec![frame.into_geometry()]
        } else {
            vec![state.cache.draw(renderer, bounds.size(), draw)]
        }
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
fn format_media_time(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

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
    pub epoch_millis: i64,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
    pub millisecond: u16,
}

pub fn chrono_now() -> EpochParts {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    chrono_now_at_millis(now.as_millis() as i64)
}

pub fn chrono_now_at_millis(epoch_millis: i64) -> EpochParts {
    let mut parts = chrono_now_at(epoch_millis.div_euclid(1_000));
    parts.epoch_millis = epoch_millis;
    parts.millisecond = epoch_millis.rem_euclid(1_000) as u16;
    parts
}

/// Broken-down *local* time for a given epoch second. The widget is the
/// documented exception that reads the host clock (a wall clock is presentation
/// metadata, not a measurement); to agree with Canvas's diagnostics HUD it must
/// read the same offset the HUD uses rather than the UTC-from-epoch math it did
/// before, which left the two clocks disagreeing by the host's UTC offset.
pub fn chrono_now_at(epoch: i64) -> EpochParts {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&epoch, &mut tm);
    }
    EpochParts {
        epoch_millis: epoch.saturating_mul(1_000),
        hour: tm.tm_hour as u64,
        minute: tm.tm_min as u64,
        second: tm.tm_sec as u64,
        millisecond: 0,
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
            let sy =
                |y: f32| bounds.height - margin - (y - min_y) / (max_y - min_y).max(1e-6) * plot_h;

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
                annotate_plot(
                    frame,
                    margin,
                    margin,
                    plot_w,
                    plot_h,
                    min_y as f64,
                    max_y as f64,
                    None,
                    max_y as f64,
                    self.axis_color,
                    11.0,
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
    fn spectral_sampling_interpolates_between_bins_and_frames() {
        let rows = vec![vec![0.0, 1.0], vec![1.0, 0.0]];
        assert!((sample_spectrum(&rows, 0.5, 0.0) - 0.5).abs() < 0.001);
        assert!((sample_spectrum(&rows, 0.0, 0.5) - 0.5).abs() < 0.001);
        assert!((sample_spectrum(&rows, 0.5, 0.5) - 0.5).abs() < 0.001);
    }

    #[test]
    fn plot_frame_reserves_typography_outside_the_data_marks() {
        let layout = PlotFrame::cartesian(Size::new(420.0, 220.0), 11.0, 0.0, 10_000.0, Some("ms"));
        assert!(layout.plot.x > 40.0, "wide labels need a real gutter");
        assert!(
            layout.plot.y >= 11.0,
            "the current-value legend needs a top row"
        );
        assert!(layout.plot.x + layout.plot.width <= 420.0);
        assert!(layout.plot.y + layout.plot.height <= 220.0);
    }

    #[test]
    fn series_interpolation_eases_values_and_new_points_from_the_previous_head() {
        let from = vec![(0.0, 2.0), (1.0, 4.0)];
        let target = vec![(0.0, 4.0), (1.0, 8.0), (2.0, 10.0)];
        let halfway = interpolate_line(&from, &target, 0.5);
        assert_eq!(halfway, vec![(0.0, 3.0), (1.0, 6.0), (1.5, 7.0)]);
    }

    #[test]
    fn neural_layout_is_stable_and_groups_columns() {
        let nodes = vec![
            NeuralNode {
                id: "a".into(),
                label: "A".into(),
                group: Some("client".into()),
                heat: 0.0,
                state: StatusLevel::Neutral,
            },
            NeuralNode {
                id: "b".into(),
                label: "B".into(),
                group: Some("gateway".into()),
                heat: 1.0,
                state: StatusLevel::Good,
            },
            NeuralNode {
                id: "c".into(),
                label: "C".into(),
                group: Some("gateway".into()),
                heat: 0.5,
                state: StatusLevel::Warning,
            },
        ];
        let first = neural_layout(&nodes, Size::new(600.0, 300.0));
        let second = neural_layout(&nodes, Size::new(600.0, 300.0));
        assert_eq!(first, second);
        assert!(first["a"].x < first["b"].x);
        assert_eq!(first["b"].x, first["c"].x);
        assert!(first["b"].y < first["c"].y);
    }

    #[test]
    fn topology_web_orbits_site_members_in_a_wide_band() {
        let node = |id: &str, group: &str| NeuralNode {
            id: id.into(),
            label: id.into(),
            group: Some(group.into()),
            heat: 0.5,
            state: StatusLevel::Good,
        };
        let nodes = vec![
            node("site:home", "sites"),
            node("home-pi", "home"),
            node("agora-one", "home"),
            node("segment:lan", "home"),
        ];
        let layout = neural_layout(&nodes, Size::new(1200.0, 280.0));
        let hub = layout["site:home"];
        assert_eq!(hub, Point::new(600.0, 140.0));
        assert_ne!(layout["home-pi"].x, layout["agora-one"].x);
        assert!(layout.values().all(|point| point.y >= 24.0 && point.y <= 256.0));
    }

    #[test]
    fn dag_layout_is_stable_and_places_parents_before_children() {
        let node = |id: &str| NeuralNode {
            id: id.into(),
            label: id.into(),
            group: None,
            heat: 0.0,
            state: StatusLevel::Neutral,
        };
        let nodes = vec![node("root"), node("child-a"), node("child-b"), node("leaf")];
        let edge = |from: &str, to: &str| NeuralEdge {
            from: from.into(),
            to: to.into(),
            heat: 0.0,
        };
        let edges = vec![
            edge("root", "child-a"),
            edge("root", "child-b"),
            edge("child-a", "leaf"),
        ];
        let first = dag_layout(&nodes, &edges, Size::new(600.0, 400.0));
        assert_eq!(first, dag_layout(&nodes, &edges, Size::new(600.0, 400.0)));
        assert!(first["root"].y < first["child-a"].y);
        assert_eq!(first["child-a"].y, first["child-b"].y);
        assert!(first["child-a"].y < first["leaf"].y);
    }

    #[test]
    fn clock_formats_hours_minutes_seconds() {
        let now = EpochParts {
            epoch_millis: 0,
            hour: 14,
            minute: 3,
            second: 7,
            millisecond: 0,
        };
        assert_eq!(strftime(&now, "%H:%M:%S"), "14:03:07");
        assert_eq!(strftime(&now, "100%%"), "100%");
        assert_eq!(strftime(&now, "%Q"), "%Q");
    }

    #[test]
    fn clock_reports_local_time_not_utc() {
        // Acceptance for dock-96j.47: the widget and the diagnostics HUD must
        // agree. The only tz-INDEPENDENT way to assert that is to require the
        // widget's broken-down time to equal localtime_r's for the same instant
        // — which holds in every timezone because both use the host offset. The
        // old UTC-from-epoch math fails this for any non-UTC host.
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let resolved = unsafe { libc::localtime_r(&epoch, &mut tm) };
        assert!(!resolved.is_null());
        let parts = chrono_now_at(epoch);
        assert_eq!(parts.hour, tm.tm_hour as u64);
        assert_eq!(parts.minute, tm.tm_min as u64);
        assert_eq!(parts.second, tm.tm_sec as u64);
    }

    #[test]
    fn oklch_ramp_endpoints_and_mid_stay_in_range() {
        // A honest scale maps the endpoints back to the declared low/high, so a
        // viewer can read a value from a cell colour: lerp at 0 and 1 must
        // return the inputs, and the midpoint must sit between them.
        let low = Color::from_rgb(0.2, 0.2, 0.8);
        let high = Color::from_rgb(0.9, 0.1, 0.1);
        let mid = lerp_oklch(low, high, 0.5);
        let near = |a: Color, b: Color| {
            (a.r - b.r).abs() < 1e-4
                && (a.g - b.g).abs() < 1e-4
                && (a.b - b.b).abs() < 1e-4
                && (a.a - b.a).abs() < 1e-4
        };
        assert!(near(lerp_oklch(low, high, 0.0), low));
        assert!(near(lerp_oklch(low, high, 1.0), high));
        assert!(mid.r > low.r.min(high.r) && mid.r < low.r.max(high.r));
    }

    #[test]
    fn short_value_formats_integers_without_trailing_dot() {
        assert_eq!(short_value(0.0), "0");
        assert_eq!(short_value(-7.0), "-7");
        assert_eq!(short_value(12.5), "12.5");
        assert_eq!(short_value(1000.0), "1.0e3");
    }

    #[test]
    fn nice_ticks_pick_a_1_2_5_ramp_inside_the_range() {
        // A color/spatial scale only helps if the step is a round 1/2/5 × 10ⁿ
        // value and every label lands inside the data range.
        let ticks = nice_ticks(0.0, 10.0, 4);
        assert!(!ticks.is_empty());
        assert!(ticks[0] >= 0.0);
        assert!(*ticks.last().unwrap() <= 10.0);
        let step = ticks[1] - ticks[0];
        let mantissa = step / 10f64.powf(step.log10().floor());
        assert!(
            [1.0, 2.0, 5.0, 10.0].contains(&mantissa.round()),
            "step {step} is not a 1/2/5 ramp"
        );
    }

    #[test]
    fn format_quantity_appends_units() {
        assert_eq!(format_quantity(0.0, None), "0");
        assert_eq!(format_quantity(1234.0, Some("ms")), "1.2k ms");
        assert_eq!(format_quantity(0.5, Some("°C")), "0.50 °C");
        assert_eq!(format_quantity(28.27, Some("$")), "$28.27");
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
    fn heatmap_fingerprint_wraps_instead_of_panicking() {
        let values = vec![vec![1.0, 2.0, 3.0, 4.0], vec![8.0, 16.0, 32.0, 64.0]];
        let first = heatmap_value_key(&values);
        assert_eq!(first, heatmap_value_key(&values));
        assert_ne!(first, heatmap_value_key(&[vec![1.0, 2.0]]));
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
            let colors = series_colors(&visual(drain(palette, STALE_CHROMA)));
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
    fn the_gamut_map_holds_hue_while_clip_shifts_it() {
        // A deliberately out-of-gamut Oklch: high chroma at a hue near blue, the
        // worst case for channel-clipping (the doc comment cites ~14° shift here).
        let extreme = Oklch {
            l: 0.70,
            c: 0.40,
            h: 280.0,
        };
        // Confirm the starting point is actually out of gamut, so the test is
        // exercising the search rather than a no-op.
        assert!(
            !in_srgb_gamut(oklch_to_linear(extreme)),
            "test color must be out of sRGB gamut"
        );

        let mapped = from_oklch(extreme);
        let mapped_oklch = to_oklch(mapped);
        let hue_diff = angular_diff(mapped_oklch.h, extreme.h);
        assert!(
            hue_diff < 1.0,
            "gamut map held hue drift (target ~0, got {hue_diff:.2}°)"
        );
        assert!(
            mapped_oklch.c < extreme.c,
            "gamut map must reduce chroma (was {}, now {})",
            extreme.c,
            mapped_oklch.c
        );

        // Naive channel clip: clamp each linear channel to [0,1] and encode.
        let linear = oklch_to_linear(extreme);
        let clipped_color = Color::from_rgb(
            srgb_encode(linear[0].clamp(0.0, 1.0)),
            srgb_encode(linear[1].clamp(0.0, 1.0)),
            srgb_encode(linear[2].clamp(0.0, 1.0)),
        );
        let clipped_oklch = to_oklch(clipped_color);
        let clip_hue_diff = angular_diff(clipped_oklch.h, extreme.h);
        assert!(
            clip_hue_diff > 5.0,
            "naive clip must shift hue measurably (got {clip_hue_diff:.2}°, expected >5°)"
        );
    }

    fn angular_diff(a: f32, b: f32) -> f32 {
        let diff = (a - b).rem_euclid(360.0);
        diff.min(360.0 - diff)
    }

    #[test]
    fn a_stale_stat_value_carries_an_opacity_mark_beyond_the_chroma_drain() {
        // A stale stat must not just drain chroma — at a glance across a room
        // the drain alone is too subtle. The value text is additionally alpha-
        // faded so staleness is legible before anyone leans in to read the
        // footer. Only the value text gets this; the chrome (label, unit,
        // change) stays at full opacity for contrast.
        const {
            assert!(
                STALE_VALUE_ALPHA < 1.0,
                "stale value alpha must reduce visibility"
            )
        };
        for (name, palette) in palettes() {
            let ready_text = color(palette.text);
            let stale_text = with_alpha(color(palette.text), STALE_VALUE_ALPHA);
            // The stale text must differ from the live text — not just in chroma
            // (the drain handles that) but in alpha, so the value itself fades.
            assert_ne!(
                ready_text, stale_text,
                "{name} stale value is not visually faded"
            );
        }
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
            for (index, series) in series_colors(&visual(palette)).into_iter().enumerate() {
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
            let colors = series_colors(&visual(palette));
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
            let colors = series_colors(&visual(*palette));
            colors.iter().map(|c| to_oklch(*c).l).sum::<f32>() / colors.len() as f32
        };
        use canvas_core::{Accent, ThemeMode, ThemeSelection};
        let dark = ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).palette();
        let light = ThemeSelection::coherence(ThemeMode::Light, Accent::Brand).palette();
        assert!(lightness(&dark) > lightness(&light));
        // And the first entry still carries the brand: identity starts where
        // the theme's primary already points.
        let brand = to_oklch(color(light.primary)).h;
        assert!(hue_gap(to_oklch(series_colors(&visual(light))[0]).h, brand) < 1.0);
    }

    #[test]
    fn charts_honor_an_explicit_theme_palette_in_order() {
        let expected = [
            [1, 2, 3],
            [4, 5, 6],
            [7, 8, 9],
            [10, 11, 12],
            [13, 14, 15],
            [16, 17, 18],
        ];
        let mut theme = visual(palettes()[0].1);
        theme.categorical = Some(expected);
        assert_eq!(
            series_colors(&theme),
            expected.into_iter().map(color).collect::<Vec<_>>()
        );
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
