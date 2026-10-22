//! The one and only drawing routine. Generic over `DrawingBackend`, so the
//! iced/wgpu path and the instrumented audit path execute identical code.

use crate::palette::Palette;
use crate::spec::PlotSpec;
use plotters::chart::{ChartBuilder, SeriesLabelPosition};
use plotters::prelude::*;
use plotters_backend::DrawingBackend;

/// Canvas would take these from ThemeMetrics. Overridable here so the spike can
/// test wall-scale type, where plotters-iced2's text-measurement bug bites.
fn env_size(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
pub fn label_size() -> u32 {
    env_size("SPIKE_LABEL_SIZE", 14)
}
pub fn title_size() -> u32 {
    env_size("SPIKE_TITLE_SIZE", 18)
}

/// Draw `spec`, taking every colour from `palette`.
///
/// `rotate_y_desc` selects between the two ways to label a y axis:
///   * `true`  -- plotters' own `y_desc`, which requests rotated text.
///   * `false` -- a horizontal unit caption, which is what actually renders.
/// The audit mode runs both to show the difference.
pub fn draw_spec<DB: DrawingBackend>(
    spec: &PlotSpec,
    palette: &Palette,
    mut builder: ChartBuilder<DB>,
    rotate_y_desc: bool,
) {
    let text = palette.rgb("text");
    let muted = palette.rgb("text_muted");
    let border = palette.rgb("border");
    let border_strong = palette.rgb("border_strong");
    let surface = palette.rgb("surface");

    let caption = if rotate_y_desc {
        spec.title.clone()
    } else {
        // Horizontal unit label, dataviz-conventional and actually renderable.
        format!("{}  —  {}", spec.title, spec.y_label)
    };

    let mut chart = builder
        .caption(caption, ("sans-serif", title_size()).into_font().color(&text))
        .margin(16)
        .x_label_area_size(38)
        .y_label_area_size(58)
        .build_cartesian_2d(
            spec.x_range.0..spec.x_range.1,
            spec.y_range.0..spec.y_range.1,
        )
        .expect("cartesian range proven non-degenerate by PlotSpec validation");

    let mut mesh = chart.configure_mesh();
    mesh.bold_line_style(border_strong.mix(0.55))
        .light_line_style(border.mix(0.30))
        .axis_style(ShapeStyle::from(border_strong).stroke_width(1))
        .x_labels(8)
        .y_labels(6)
        .x_label_style(("sans-serif", label_size()).into_font().color(&muted))
        .y_label_style(("sans-serif", label_size()).into_font().color(&muted))
        .x_label_formatter(&|v: &f64| format!("{v:.0}"))
        .y_label_formatter(&|v: &f64| format!("{v:.1}"))
        .x_desc(spec.x_label.clone());
    if rotate_y_desc {
        mesh.y_desc(spec.y_label.clone());
    }
    mesh.axis_desc_style(("sans-serif", label_size()).into_font().color(&muted))
        .draw()
        .expect("mesh draw");

    for entry in &spec.series {
        let colour = palette.rgb(&entry.role);
        let style = ShapeStyle::from(colour).stroke_width(2);
        chart
            .draw_series(LineSeries::new(entry.points.iter().copied(), style))
            .expect("series draw")
            .label(entry.name.clone())
            .legend(move |(x, y)| {
                PathElement::new([(x, y), (x + 18, y)], ShapeStyle::from(colour).stroke_width(2))
            });
    }

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(surface.mix(0.92))
        .border_style(ShapeStyle::from(border_strong))
        .label_font(("sans-serif", label_size()).into_font().color(&text))
        .draw()
        .expect("legend draw");
}
