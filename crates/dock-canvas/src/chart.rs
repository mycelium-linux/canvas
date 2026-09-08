//! Data visualization chart rendering using iced canvas widget.
//! Extracted from main.rs for module clarity (dock-q26.14).

use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use iced::widget::canvas::{Geometry, Path};
use iced::mouse;

pub struct ChartProgram {
    pub data: Vec<(f32, f32)>,
    pub chart_type: String,
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
        let stroke = canvas::Stroke::default()
            .with_width(1.0)
            .with_color(Color { a: 0.15, ..self.axis_color });

        let min_x = self.data.iter().map(|(x, _)| *x).fold(f32::INFINITY, f32::min);
        let max_x = self.data.iter().map(|(x, _)| *x).fold(f32::NEG_INFINITY, f32::max);
        let min_y = self.data.iter().map(|(_, y)| *y).fold(f32::INFINITY, f32::min);
        let max_y = self.data.iter().map(|(_, y)| *y).fold(f32::NEG_INFINITY, f32::max);

        let plot_w = bounds.width - 2.0 * margin;
        let plot_h = bounds.height - 2.0 * margin;

        let sx = |x: f32| margin + (x - min_x) / (max_x - min_x).max(1e-6) * plot_w;
        let sy = |y: f32| bounds.height - margin - (y - min_y) / (max_y - min_y).max(1e-6) * plot_h;

        if plot_w > 0.0 && plot_h > 0.0 {
            let grid = Path::line(
                Point::new(margin, bounds.height - margin),
                Point::new(bounds.width - margin, bounds.height - margin),
            );
            frame.stroke(&grid, stroke);
            let grid_y = Path::line(
                Point::new(margin, margin),
                Point::new(margin, bounds.height - margin),
            );
            frame.stroke(&grid_y, stroke);
        }

        match self.chart_type.as_str() {
            "bar" => {
                let n = self.data.len() as f32;
                let bar_w = plot_w / (n + 1.0);
                for (i, (_, y)) in self.data.iter().enumerate() {
                    let cx = margin + bar_w * (i as f32 + 0.5);
                    let bh = (*y - min_y) / (max_y - min_y).max(1e-6) * plot_h;
                    let rect = Path::rectangle(
                        Point::new(cx - bar_w * 0.4, bounds.height - margin - bh),
                        Size::new(bar_w * 0.8, bh.max(1.0)),
                    );
                    frame.fill(&rect, self.bar_color);
                }
            }
            "line" => {
                let line_stroke = canvas::Stroke::default()
                    .with_width(2.0)
                    .with_color(self.line_color);
                for window in self.data.windows(2) {
                    let path = Path::line(
                        Point::new(sx(window[0].0), sy(window[0].1)),
                        Point::new(sx(window[1].0), sy(window[1].1)),
                    );
                    frame.stroke(&path, line_stroke);
                }
                for (x, y) in &self.data {
                    let circle = Path::circle(Point::new(sx(*x), sy(*y)), 2.5);
                    frame.fill(&circle, self.line_color);
                }
            }
            "scatter" => {
                for (x, y) in &self.data {
                    let circle = Path::circle(Point::new(sx(*x), sy(*y)), 3.0);
                    frame.fill(&circle, self.point_color);
                }
            }
            _ => {
                for (x, y) in &self.data {
                    let circle = Path::circle(Point::new(sx(*x), sy(*y)), 2.0);
                    frame.fill(&circle, self.point_color);
                }
            }
        }

        vec![frame.into_geometry()]
    }
}
