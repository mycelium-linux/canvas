use ::canvas::chart::{ChartDocument, ChartKind};
use ::canvas::view::ViewRect;
use canvas_core::ThemeMetrics;
use iced::mouse;
use iced::widget::canvas::{self, Geometry};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};
use unibus_media::Detection;

pub(crate) fn view_crop_pixels(viewport: ViewRect, source: [u32; 2]) -> iced::Rectangle<u32> {
    let width = source[0].max(1);
    let height = source[1].max(1);
    let x = (viewport.x * width as f32).floor().max(0.0) as u32;
    let y = (viewport.y * height as f32).floor().max(0.0) as u32;
    let right = ((viewport.x + viewport.width) * width as f32)
        .ceil()
        .max((x + 1) as f32) as u32;
    let bottom = ((viewport.y + viewport.height) * height as f32)
        .ceil()
        .max((y + 1) as f32) as u32;
    iced::Rectangle {
        x: x.min(width - 1),
        y: y.min(height - 1),
        width: right.min(width).saturating_sub(x).max(1),
        height: bottom.min(height).saturating_sub(y).max(1),
    }
}

pub(crate) fn attention_geometry(
    canvas: Size,
    source: [u32; 2],
    viewport: ViewRect,
    attention: ViewRect,
) -> Option<Rectangle> {
    let left = attention.x.max(viewport.x);
    let top = attention.y.max(viewport.y);
    let right = (attention.x + attention.width).min(viewport.x + viewport.width);
    let bottom = (attention.y + attention.height).min(viewport.y + viewport.height);
    if right <= left || bottom <= top {
        return None;
    }

    let crop_width = source[0].max(1) as f32 * viewport.width;
    let crop_height = source[1].max(1) as f32 * viewport.height;
    let scale = (canvas.width / crop_width).min(canvas.height / crop_height);
    let drawn_width = crop_width * scale;
    let drawn_height = crop_height * scale;
    let offset_x = (canvas.width - drawn_width) / 2.0;
    let offset_y = (canvas.height - drawn_height) / 2.0;
    Some(Rectangle {
        x: offset_x + ((left - viewport.x) / viewport.width) * drawn_width,
        y: offset_y + ((top - viewport.y) / viewport.height) * drawn_height,
        width: ((right - left) / viewport.width) * drawn_width,
        height: ((bottom - top) / viewport.height) * drawn_height,
    })
}

pub(crate) struct DetectionOverlayProgram {
    pub(crate) detections: Vec<Detection>,
    pub(crate) source_pixels: [u32; 2],
    pub(crate) color: Color,
}

impl<Message> canvas::Program<Message> for DetectionOverlayProgram {
    type State = canvas_widgets::live::CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut key = canvas_widgets::live::Key::new()
            .add_u64(self.source_pixels[0] as u64)
            .add_u64(self.source_pixels[1] as u64)
            .add_color(self.color);
        for detection in &self.detections {
            key = key
                .add_str(&detection.tracking_id)
                .add_str(&detection.label)
                .add_f32(detection.confidence)
                .add_f32(detection.bounds.x)
                .add_f32(detection.bounds.y)
                .add_f32(detection.bounds.width)
                .add_f32(detection.bounds.height);
        }
        let key = key.build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let source_width = self.source_pixels[0].max(1) as f32;
            let source_height = self.source_pixels[1].max(1) as f32;
            let scale = (bounds.width / source_width).min(bounds.height / source_height);
            let drawn_width = source_width * scale;
            let drawn_height = source_height * scale;
            let offset_x = (bounds.width - drawn_width) * 0.5;
            let offset_y = (bounds.height - drawn_height) * 0.5;
            for detection in &self.detections {
                let region = Rectangle {
                    x: offset_x + detection.bounds.x * drawn_width,
                    y: offset_y + detection.bounds.y * drawn_height,
                    width: detection.bounds.width * drawn_width,
                    height: detection.bounds.height * drawn_height,
                };
                let path = canvas::Path::rectangle(region.position(), region.size());
                frame.stroke(
                    &path,
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(self.color),
                );
                chart_text(
                    frame,
                    &format!("{} {:.0}%", detection.label, detection.confidence * 100.0),
                    Point::new(region.x + 4.0, (region.y - 17.0).max(3.0)),
                    12.0,
                    self.color,
                    iced::alignment::Horizontal::Left,
                );
            }
        })
    }
}

pub(crate) struct AttentionProgram {
    pub(crate) viewport: ViewRect,
    pub(crate) attention: ViewRect,
    pub(crate) source_pixels: [u32; 2],
    pub(crate) color: Color,
    pub(crate) label: Option<String>,
}

impl<Message> canvas::Program<Message> for AttentionProgram {
    type State = canvas_widgets::live::CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = canvas_widgets::live::Key::new()
            .add_f32(self.viewport.x)
            .add_f32(self.viewport.y)
            .add_f32(self.viewport.width)
            .add_f32(self.viewport.height)
            .add_f32(self.attention.x)
            .add_f32(self.attention.y)
            .add_f32(self.attention.width)
            .add_f32(self.attention.height)
            .add_u64(self.source_pixels[0] as u64)
            .add_u64(self.source_pixels[1] as u64)
            .add_color(self.color)
            .add_str(self.label.as_deref().unwrap_or(""))
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let Some(region) = attention_geometry(
                bounds.size(),
                self.source_pixels,
                self.viewport,
                self.attention,
            ) else {
                return;
            };
            let path = canvas::Path::rectangle(region.position(), region.size());
            frame.fill(
                &path,
                Color {
                    a: 0.08,
                    ..self.color
                },
            );
            frame.stroke(
                &path,
                canvas::Stroke::default().with_width(2.0).with_color(Color {
                    a: 0.95,
                    ..self.color
                }),
            );
            if let Some(label) = &self.label {
                chart_text(
                    frame,
                    label,
                    Point::new(region.x + 5.0, (region.y - 18.0).max(4.0)),
                    12.0,
                    self.color,
                    iced::alignment::Horizontal::Left,
                );
            }
        })
    }
}

pub(crate) struct ChartProgram {
    pub(crate) chart: ChartDocument,
    pub(crate) axis_color: Color,
    pub(crate) muted_color: Color,
    pub(crate) surface_color: Color,
    pub(crate) series_colors: Vec<Color>,
    /// The chart draws its own labels, so it has to be handed the type scale
    /// rather than inheriting one from a widget it does not use.
    pub(crate) metrics: ThemeMetrics,
}

impl<Message> canvas::Program<Message> for ChartProgram {
    type State = canvas_widgets::live::CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let total_points: usize = self.chart.series.iter().map(|s| s.points.len()).sum();
        let val_sum: f32 = self
            .chart
            .series
            .iter()
            .flat_map(|s| &s.points)
            .map(|p| p.y)
            .sum();
        let key = canvas_widgets::live::Key::new()
            .add_usize(total_points)
            .add_f32(val_sum)
            .add_usize(self.chart.kind as usize)
            .add_str(&self.chart.title)
            .add_f32(bounds.width)
            .add_f32(bounds.height)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let Some((min_x, max_x, min_y, max_y)) = chart_extents(&self.chart) else {
                chart_text(
                    frame,
                    "No chart data",
                    Point::new(20.0, 24.0),
                    self.metrics.body,
                    self.muted_color,
                    iced::alignment::Horizontal::Left,
                );
                return;
            };

            chart_text(
                frame,
                &self.chart.title,
                Point::new(18.0, 12.0),
                self.metrics.body,
                self.axis_color,
                iced::alignment::Horizontal::Left,
            );
            let legend = legend_layout(
                self.chart.series.iter().map(|series| series.name.as_str()),
                bounds.width.max(1.0),
            );
            for (index, item) in legend.items.iter().enumerate() {
                let color = self.series_colors[index % self.series_colors.len()];
                frame.fill(
                    &canvas::Path::circle(Point::new(item.x + 4.0, item.y + 6.0), 3.5),
                    color,
                );
                chart_text(
                    frame,
                    &item.label,
                    Point::new(item.x + 13.0, item.y),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Left,
                );
            }

            let left = if bounds.width < 360.0 { 44.0 } else { 58.0 };
            let right = (bounds.width - 18.0).max(left + 1.0);
            let top = (48.0 + legend.rows as f32 * 18.0).min(bounds.height * 0.45);
            let bottom = (bounds.height - 36.0).max(top + 1.0);
            let plot_width = right - left;
            let plot_height = bottom - top;
            if plot_width <= 1.0 || plot_height <= 1.0 {
                return;
            }

            let x_ticks = chart_ticks(min_x, max_x, if bounds.width < 420.0 { 3 } else { 5 });
            let y_ticks = chart_ticks(min_y, max_y, if bounds.height < 300.0 { 3 } else { 5 });
            let sx = |x: f32| left + (x - min_x) / (max_x - min_x) * plot_width;
            let sy = |y: f32| bottom - (y - min_y) / (max_y - min_y) * plot_height;
            let grid_stroke = canvas::Stroke::default().with_width(1.0).with_color(Color {
                a: 0.14,
                ..self.axis_color
            });

            for tick in &y_ticks {
                let y = sy(*tick);
                frame.stroke(
                    &canvas::Path::line(Point::new(left, y), Point::new(right, y)),
                    grid_stroke,
                );
                chart_text(
                    frame,
                    &format_chart_value(*tick),
                    Point::new(left - 7.0, y),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Right,
                );
            }
            for tick in &x_ticks {
                let x = sx(*tick);
                frame.stroke(
                    &canvas::Path::line(Point::new(x, top), Point::new(x, bottom)),
                    grid_stroke,
                );
                chart_text(
                    frame,
                    &format_chart_value(*tick),
                    Point::new(x, bottom + 7.0),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Center,
                );
            }

            let y_label = axis_title(&self.chart.y_axis);
            chart_text(
                frame,
                &y_label,
                Point::new(12.0, top - 20.0),
                self.metrics.micro,
                self.muted_color,
                iced::alignment::Horizontal::Left,
            );
            let x_label = axis_title(&self.chart.x_axis);
            chart_text(
                frame,
                &x_label,
                Point::new((left + right) / 2.0, bounds.height - 15.0),
                self.metrics.micro,
                self.muted_color,
                iced::alignment::Horizontal::Center,
            );

            let total_points = self
                .chart
                .series
                .iter()
                .map(|series| series.points.len())
                .sum::<usize>()
                .max(1);
            let bar_width = (plot_width / total_points as f32 * 0.72).clamp(2.0, 28.0);
            for (series_index, series) in self.chart.series.iter().enumerate() {
                let series_color = self.series_colors[series_index % self.series_colors.len()];
                match self.chart.kind {
                    ChartKind::Line => {
                        let points: Vec<Point> = series
                            .points
                            .iter()
                            .map(|point| Point::new(sx(point.x), sy(point.y)))
                            .collect();
                        draw_chart_series_line(
                            frame,
                            &points,
                            bottom,
                            series_color,
                            CHART_AREA_ALPHA / self.chart.series.len().max(1) as f32,
                        );
                    }
                    ChartKind::Scatter => {
                        for point in &series.points {
                            frame.fill(
                                &canvas::Path::circle(Point::new(sx(point.x), sy(point.y)), 3.4),
                                series_color,
                            );
                        }
                    }
                    ChartKind::Bar => {
                        let group_offset = (series_index as f32
                            - (self.chart.series.len() - 1) as f32 / 2.0)
                            * bar_width;
                        for point in &series.points {
                            let x = sx(point.x) + group_offset;
                            let y = sy(point.y);
                            let base = sy(min_y.max(0.0));
                            frame.fill(
                                &canvas::Path::rectangle(
                                    Point::new(x - bar_width * 0.42, y.min(base)),
                                    Size::new(bar_width * 0.84, (base - y).abs().max(1.0)),
                                ),
                                series_color,
                            );
                        }
                    }
                }
            }

            if let Some(cursor) = cursor.position_in(bounds)
                && cursor.x >= left
                && cursor.x <= right
                && cursor.y >= top
                && cursor.y <= bottom
                && let Some((series_index, point, position)) =
                    nearest_chart_point(&self.chart, cursor, sx, sy, 20.0)
            {
                let series = &self.chart.series[series_index];
                let series_color = self.series_colors[series_index % self.series_colors.len()];
                frame.fill(&canvas::Path::circle(position, 5.0), self.surface_color);
                frame.stroke(
                    &canvas::Path::circle(position, 5.0),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(series_color),
                );
                let value = format!(
                    "{}  {} · {}",
                    series.name,
                    format_axis_value(point.x, self.chart.x_axis.unit.as_deref()),
                    format_axis_value(point.y, self.chart.y_axis.unit.as_deref())
                );
                let tooltip_width = (value.chars().count() as f32 * 6.5 + 18.0).min(plot_width);
                let tooltip_x = (position.x + 12.0).min(right - tooltip_width);
                let tooltip_y = (position.y - 32.0).max(top + 4.0);
                frame.fill(
                    &canvas::Path::rounded_rectangle(
                        Point::new(tooltip_x, tooltip_y),
                        Size::new(tooltip_width, 24.0),
                        6.0.into(),
                    ),
                    Color {
                        a: 0.94,
                        ..self.surface_color
                    },
                );
                chart_text(
                    frame,
                    &value,
                    Point::new(tooltip_x + 9.0, tooltip_y + 5.0),
                    self.metrics.micro,
                    self.axis_color,
                    iced::alignment::Horizontal::Left,
                );
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegendItem {
    pub(crate) label: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegendLayout {
    pub(crate) items: Vec<LegendItem>,
    pub(crate) rows: usize,
}

pub(crate) fn legend_layout<'a>(labels: impl Iterator<Item = &'a str>, width: f32) -> LegendLayout {
    let available = (width - 36.0).max(80.0);
    let mut x = 18.0;
    let mut row = 0usize;
    let mut items = Vec::new();
    for label in labels {
        let item_width = label.chars().count() as f32 * 6.5 + 30.0;
        if x > 18.0 && x + item_width > available + 18.0 {
            row += 1;
            x = 18.0;
        }
        items.push(LegendItem {
            label: label.to_owned(),
            x,
            y: 34.0 + row as f32 * 18.0,
        });
        x += item_width;
    }
    LegendLayout {
        rows: row + usize::from(!items.is_empty()),
        items,
    }
}

pub(crate) fn chart_extents(chart: &ChartDocument) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for point in chart.series.iter().flat_map(|series| &series.points) {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    if !min_x.is_finite() {
        return None;
    }
    if min_x == max_x {
        min_x -= 0.5;
        max_x += 0.5;
    }
    if min_y == max_y {
        let padding = min_y.abs().max(1.0) * 0.1;
        min_y -= padding;
        max_y += padding;
    }
    Some((min_x, max_x, min_y, max_y))
}

pub(crate) fn chart_ticks(min: f32, max: f32, count: usize) -> Vec<f32> {
    let count = count.max(2);
    let step = (max - min) / (count - 1) as f32;
    (0..count).map(|index| min + step * index as f32).collect()
}

pub(crate) fn nearest_chart_point<F, G>(
    chart: &ChartDocument,
    cursor: Point,
    sx: F,
    sy: G,
    maximum_distance: f32,
) -> Option<(usize, ::canvas::chart::ChartPoint, Point)>
where
    F: Fn(f32) -> f32,
    G: Fn(f32) -> f32,
{
    chart
        .series
        .iter()
        .enumerate()
        .flat_map(|(series_index, series)| {
            let sx = &sx;
            let sy = &sy;
            series.points.iter().map(move |point| {
                let position = Point::new(sx(point.x), sy(point.y));
                let distance = (position.x - cursor.x).hypot(position.y - cursor.y);
                (distance, series_index, point.clone(), position)
            })
        })
        .filter(|(distance, _, _, _)| *distance <= maximum_distance)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, series_index, point, position)| (series_index, point, position))
}

pub(crate) fn axis_title(axis: &::canvas::chart::ChartAxis) -> String {
    axis.unit.as_ref().map_or_else(
        || axis.label.clone(),
        |unit| format!("{} ({unit})", axis.label),
    )
}

pub(crate) fn format_chart_value(value: f32) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else if value.abs() >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

pub(crate) fn format_axis_value(value: f32, unit: Option<&str>) -> String {
    let value = format_chart_value(value);
    unit.map_or(value.clone(), |unit| {
        if unit == "$" || unit == "£" || unit == "€" {
            format!("{unit}{value}")
        } else {
            format!("{value} {unit}")
        }
    })
}

const CHART_LINE_WIDTH: f32 = 2.0;
/// Opacity of the area under a line at the line itself, falling to nothing at
/// the axis. Divided among overlaid series so a four-series chart does not silt
/// up.
const CHART_AREA_ALPHA: f32 = 0.22;
const CHART_HEAD_RADIUS: f32 = CHART_LINE_WIDTH * 1.6;

/// Draw one series: the area beneath it, the line as a single path, and a
/// filled head on the newest point.
///
/// Every vertex used to be the meeting of two separately stroked `Path::line`
/// segments, which on a 2px butt-capped stroke leaves a visible notch at each
/// one. One path with round joins removes them, and hands the tessellator one
/// shape instead of n.
///
/// This mirrors `canvas_widgets::live`'s private `draw_series_line`; the two
/// should become one call once that crate exports it.
pub(crate) fn draw_chart_series_line(
    frame: &mut canvas::Frame,
    points: &[Point],
    baseline: f32,
    stroke: Color,
    area_alpha: f32,
) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    if points.len() < 2 {
        return;
    }
    let crest = points.iter().map(|point| point.y).fold(baseline, f32::min);
    let area = canvas::Path::new(|path| {
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
            .add_stop(
                0.0,
                Color {
                    a: area_alpha,
                    ..stroke
                },
            )
            .add_stop(1.0, Color { a: 0.0, ..stroke }),
    );
    let line = canvas::Path::new(|path| {
        path.move_to(*first);
        for point in &points[1..] {
            path.line_to(*point);
        }
    });
    frame.stroke(
        &line,
        canvas::Stroke {
            style: canvas::Style::Solid(stroke),
            width: CHART_LINE_WIDTH,
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..Default::default()
        },
    );
    // The newest point is what a live series is read for; without it the eye
    // has to work out which end of the line is now.
    frame.fill(&canvas::Path::circle(*last, CHART_HEAD_RADIUS), stroke);
}

pub(crate) fn chart_text(
    frame: &mut canvas::Frame,
    content: &str,
    position: Point,
    size: f32,
    color: Color,
    align_x: iced::alignment::Horizontal,
) {
    frame.fill_text(canvas::Text {
        content: content.to_owned(),
        position,
        color,
        size: iced::Pixels(size),
        align_x: align_x.into(),
        ..Default::default()
    });
}
