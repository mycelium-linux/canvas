//! SPIKE: can plotters draw Canvas-quality charts on iced 0.14, driven from Lua?
//!
//! Run:   cargo run --bin spike -- plots/fleet.lua
//! Check: cargo run --bin spike -- --check plots/fleet.lua   (headless, no window)

mod lua;
mod palette;
mod spec;

use iced::widget::{Column, container, text};
use iced::{Element, Font, Length, Size, Theme};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use plotters_backend::DrawingBackend;
use plotters_iced2::{Chart, ChartWidget};

use palette::{DARK, METRICS, Metrics, Palette, Role};
use spec::{PlotSpec, SeriesKind};

const FONT: &str = "sans-serif";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (check_only, path) = match args.as_slice() {
        [flag, p] if flag == "--check" => (true, p.clone()),
        [p] => (false, p.clone()),
        _ => (false, "plots/fleet.lua".to_string()),
    };

    let source = std::fs::read_to_string(&path)?;
    // Fail loud: a bad plot script is an error at load, not a blank widget.
    let plot = lua::load_plot(&source).map_err(|e| format!("{path}: {e}"))?;
    let (xr, yr) = plot.ranges()?;

    println!("loaded {path}");
    println!("  title   {:?}", plot.title);
    println!("  x       {:?} -> {:.2}..{:.2}  ({} ticks)", plot.x, xr.0, xr.1, plot.x_ticks);
    println!("  y       {:?} -> {:.2}..{:.2}  ({} ticks)", plot.y, yr.0, yr.1, plot.y_ticks);
    println!("  legend  {}", plot.legend);
    for s in &plot.series {
        println!(
            "  series  {:<12} {:?} {:?} w{} {} pts",
            s.name,
            s.color,
            s.kind,
            s.width,
            s.points.len()
        );
    }

    if check_only {
        return Ok(());
    }

    iced::application(
        move || App {
            chart: SpecChart {
                plot: plot.clone(),
                palette: DARK,
                metrics: METRICS,
            },
        },
        App::update,
        App::view,
    )
    .title("plotters-iced spike")
    .antialiasing(true)
    .theme(|_| Theme::Dark)
    .default_font(Font::with_name(FONT))
    .window_size(Size::new(1100.0, 620.0))
    .run()?;

    Ok(())
}

#[derive(Debug, Clone)]
enum Message {}

struct App {
    chart: SpecChart,
}

impl App {
    fn update(&mut self, _m: Message) {}

    fn view(&self) -> Element<'_, Message> {
        let body = Column::new()
            .push(
                ChartWidget::new(&self.chart)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .push(
                text("chart geometry, axes, legend and labels all drawn by plotters")
                    .size(METRICS.micro)
                    .color(DARK.iced(Role::TextMuted)),
            )
            .spacing(METRICS.space_3);

        container(body)
            .padding(METRICS.space_3)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(DARK.iced(Role::Background).into()),
                ..Default::default()
            })
            .into()
    }
}

struct SpecChart {
    plot: PlotSpec,
    palette: Palette,
    metrics: Metrics,
}

impl Chart<Message> for SpecChart {
    type State = ();

    fn build_chart<DB: DrawingBackend>(&self, _state: &(), mut builder: ChartBuilder<DB>) {
        let p = &self.palette;
        let m = &self.metrics;

        // Every mark below resolves through the supplied palette. plotters'
        // own defaults (BLACK axes, Palette99 series colours) are never used.
        let ink = p.color(Role::Text);
        let muted = p.color(Role::TextMuted);
        let axis = p.color(Role::BorderStrong);
        let grid_bold = p.faded(Role::Border, 0.85);
        let grid_light = p.faded(Role::Border, 0.40);

        let title_style = TextStyle::from((FONT, m.title as i32).into_font())
            .color(&ink)
            .pos(Pos::new(HPos::Left, VPos::Top));
        let label_style = TextStyle::from((FONT, m.micro as i32).into_font()).color(&muted);
        let desc_style = TextStyle::from((FONT, m.small as i32).into_font()).color(&muted);

        let ((x0, x1), (y0, y1)) = match self.plot.ranges() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("plot range error: {e}");
                return;
            }
        };

        let mut chart = match builder
            .margin(m.space_3 as i32)
            .caption(&self.plot.title, title_style)
            // Room for real tick text plus the axis description.
            .x_label_area_size((m.micro + m.small + m.space_3) as i32)
            .y_label_area_size((m.micro * 3.4 + m.space_3) as i32)
            .build_cartesian_2d(x0..x1, y0..y1)
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("build_cartesian_2d failed: {e}");
                return;
            }
        };

        if let Err(e) = chart
            .configure_mesh()
            .x_labels(self.plot.x_ticks)
            .y_labels(self.plot.y_ticks)
            .x_desc(&self.plot.x_label)
            .y_desc(&self.plot.y_label)
            .axis_desc_style(desc_style)
            .label_style(label_style)
            .axis_style(ShapeStyle::from(&axis).stroke_width(1))
            .bold_line_style(ShapeStyle::from(&grid_bold).stroke_width(1))
            .light_line_style(ShapeStyle::from(&grid_light).stroke_width(1))
            .x_label_formatter(&|v| format!("{v:.0}"))
            .y_label_formatter(&|v| format!("{v:.0}"))
            .draw()
        {
            eprintln!("mesh draw failed: {e}");
            return;
        }

        for s in &self.plot.series {
            let color = p.color(s.color);
            let stroke = ShapeStyle::from(&color).stroke_width(s.width);
            let pts = s.points.iter().copied();

            let drawn = match s.kind {
                SeriesKind::Line => chart.draw_series(LineSeries::new(pts, stroke)),
                SeriesKind::Area => chart.draw_series(
                    AreaSeries::new(pts, y0, color.mix(0.25)).border_style(stroke),
                ),
                SeriesKind::Points => chart.draw_series(
                    s.points
                        .iter()
                        .map(|&(x, y)| Circle::new((x, y), s.width as i32 + 1, color.filled())),
                ),
            };

            match drawn {
                Ok(series) => {
                    let legend_style = ShapeStyle::from(&color).stroke_width(3);
                    series.label(&s.name).legend(move |(x, y)| {
                        PathElement::new(vec![(x, y), (x + 18, y)], legend_style)
                    });
                }
                Err(e) => eprintln!("series {:?} failed: {e}", s.name),
            }
        }

        if self.plot.legend {
            if let Err(e) = chart
                .configure_series_labels()
                .position(SeriesLabelPosition::UpperRight)
                .margin(m.space_3 as i32)
                .legend_area_size((m.body * 1.6) as i32)
                .background_style(p.faded(Role::Surface, 0.92))
                .border_style(ShapeStyle::from(&p.color(Role::Border)).stroke_width(1))
                .label_font(TextStyle::from((FONT, m.small as i32).into_font()).color(&ink))
                .draw()
            {
                eprintln!("legend draw failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spec::Axis;

    fn fleet() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plots/fleet.lua")).unwrap()
    }

    #[test]
    fn lua_defines_a_whole_plot() {
        let p = lua::load_plot(&fleet()).expect("fleet.lua should load");
        assert_eq!(p.title, "Fleet throughput");
        assert_eq!(p.y_label, "requests / s");
        assert_eq!(p.x, Axis::Fixed(0.0, 60.0));
        assert_eq!(p.y, Axis::Auto);
        assert_eq!(p.x_ticks, 7);
        assert_eq!(p.series.len(), 3);
        assert_eq!(p.series[0].color, Role::Primary);
        assert_eq!(p.series[2].kind, SeriesKind::Area);
        assert_eq!(p.series[0].points.len(), 121);
    }

    #[test]
    fn auto_range_is_inferred_from_the_data() {
        let p = lua::load_plot(&fleet()).unwrap();
        let (x, y) = p.ranges().unwrap();
        assert_eq!((x.0, x.1), (0.0, 60.0), "fixed x survives verbatim");
        assert!(y.0 < 14.0 && y.1 > 80.0, "auto y covers the data: {y:?}");
    }

    #[test]
    fn off_palette_colour_is_rejected() {
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plots/bad_color.lua"))
                .unwrap();
        let err = lua::load_plot(&src).unwrap_err();
        assert!(err.contains("not a palette role"), "{err}");
    }

    #[test]
    fn source_budget_is_enforced() {
        let big = format!("-- {}\nreturn {{}}", "x".repeat(spec::MAX_SOURCE_BYTES));
        let err = lua::load_plot(&big).unwrap_err();
        assert!(err.contains("byte budget"), "{err}");
    }

    #[test]
    fn point_budget_is_enforced() {
        let src = format!(
            r#"
            local pts = {{}}
            for i = 1, {} do pts[i] = {{ i, i }} end
            return {{ series = {{ {{ name = "big", color = "primary", points = pts }} }} }}
            "#,
            spec::MAX_SERIES_POINTS + 10
        );
        let err = lua::load_plot(&src).unwrap_err();
        assert!(err.contains("more than"), "{err}");
    }

    #[test]
    fn runaway_script_is_interrupted() {
        let err = lua::load_plot("while true do end").unwrap_err();
        assert!(err.contains("step budget"), "{err}");
    }

    #[test]
    fn sandbox_denies_io() {
        let err = lua::load_plot(r#"return { series = io.open("/etc/passwd") }"#).unwrap_err();
        assert!(!err.is_empty(), "io must not be reachable");
    }
}
