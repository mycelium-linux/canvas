//! Themeable GPU rendering for the canonical FPL vector mark.

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, Vector};
use std::sync::LazyLock;
use svgtypes::{PathParser, PathSegment, TransformListParser, TransformListToken};

const SOURCE: &str = include_str!("../assets/fpl-mark.svg");
const VIEWBOX_WIDTH: f32 = 210.0;
const VIEWBOX_HEIGHT: f32 = 100.0;

static PATHS: LazyLock<Vec<Path>> = LazyLock::new(|| {
    let document = roxmltree::Document::parse(SOURCE).expect("bundled FPL mark must be valid SVG");
    document
        .descendants()
        .filter(|node| node.has_tag_name("path"))
        .map(|node| {
            let data = node
                .attribute("d")
                .expect("every FPL mark path must have path data");
            let transform = node
                .attribute("transform")
                .map(parse_transform)
                .unwrap_or_default();
            parse_path(data, transform)
        })
        .collect()
});

#[derive(Debug, Clone, Copy)]
struct Mark {
    accent: Color,
    field: Color,
    ink: Color,
}

pub(crate) fn view<'a, Message: 'a>(
    accent: Color,
    field: Color,
    ink: Color,
) -> Element<'a, Message> {
    Canvas::new(Mark { accent, field, ink })
        .width(Length::Fixed(44.0))
        .height(Length::Fixed(24.0))
        .into()
}

impl<Message> canvas::Program<Message> for Mark {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let scale = (bounds.width / VIEWBOX_WIDTH).min(bounds.height / VIEWBOX_HEIGHT);
        let offset = Vector::new(
            (bounds.width - VIEWBOX_WIDTH * scale) / 2.0,
            (bounds.height - VIEWBOX_HEIGHT * scale) / 2.0,
        );
        frame.with_save(|frame| {
            frame.translate(offset);
            frame.scale(scale);
            for (path, fill) in PATHS.iter().zip([self.accent, self.field, self.ink]) {
                frame.fill(path, fill);
            }
        });
        vec![frame.into_geometry()]
    }
}

#[derive(Debug, Clone, Copy)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }
}

impl Affine {
    fn point(self, point: Point) -> Point {
        Point::new(
            self.a * point.x + self.c * point.y + self.e,
            self.b * point.x + self.d * point.y + self.f,
        )
    }
}

fn parse_transform(value: &str) -> Affine {
    let mut tokens = TransformListParser::from(value);
    let token = tokens
        .next()
        .expect("FPL path transform must not be empty")
        .expect("FPL path transform must be valid");
    assert!(tokens.next().is_none(), "FPL path uses one transform");
    let TransformListToken::Matrix { a, b, c, d, e, f } = token else {
        panic!("FPL path transform must be a matrix")
    };
    Affine {
        a: a as f32,
        b: b as f32,
        c: c as f32,
        d: d as f32,
        e: e as f32,
        f: f as f32,
    }
}

fn parse_path(data: &str, transform: Affine) -> Path {
    let mut current = Point::ORIGIN;
    let mut subpath = Point::ORIGIN;
    let mut last_cubic_control = None;
    let mut last_quadratic_control = None;

    Path::new(|builder| {
        for segment in PathParser::from(data) {
            let segment = segment.expect("FPL mark path data must be valid");
            match segment {
                PathSegment::MoveTo { abs, x, y } => {
                    current = resolve(current, abs, x, y);
                    subpath = current;
                    builder.move_to(transform.point(current));
                    last_cubic_control = None;
                    last_quadratic_control = None;
                }
                PathSegment::LineTo { abs, x, y } => {
                    current = resolve(current, abs, x, y);
                    builder.line_to(transform.point(current));
                    last_cubic_control = None;
                    last_quadratic_control = None;
                }
                PathSegment::HorizontalLineTo { abs, x } => {
                    current.x = if abs { x as f32 } else { current.x + x as f32 };
                    builder.line_to(transform.point(current));
                    last_cubic_control = None;
                    last_quadratic_control = None;
                }
                PathSegment::VerticalLineTo { abs, y } => {
                    current.y = if abs { y as f32 } else { current.y + y as f32 };
                    builder.line_to(transform.point(current));
                    last_cubic_control = None;
                    last_quadratic_control = None;
                }
                PathSegment::CurveTo {
                    abs,
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => {
                    let control_a = resolve(current, abs, x1, y1);
                    let control_b = resolve(current, abs, x2, y2);
                    current = resolve(current, abs, x, y);
                    builder.bezier_curve_to(
                        transform.point(control_a),
                        transform.point(control_b),
                        transform.point(current),
                    );
                    last_cubic_control = Some(control_b);
                    last_quadratic_control = None;
                }
                PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                    let control_a = reflect(last_cubic_control.unwrap_or(current), current);
                    let control_b = resolve(current, abs, x2, y2);
                    current = resolve(current, abs, x, y);
                    builder.bezier_curve_to(
                        transform.point(control_a),
                        transform.point(control_b),
                        transform.point(current),
                    );
                    last_cubic_control = Some(control_b);
                    last_quadratic_control = None;
                }
                PathSegment::Quadratic { abs, x1, y1, x, y } => {
                    let control = resolve(current, abs, x1, y1);
                    current = resolve(current, abs, x, y);
                    builder.quadratic_curve_to(transform.point(control), transform.point(current));
                    last_quadratic_control = Some(control);
                    last_cubic_control = None;
                }
                PathSegment::SmoothQuadratic { abs, x, y } => {
                    let control = reflect(last_quadratic_control.unwrap_or(current), current);
                    current = resolve(current, abs, x, y);
                    builder.quadratic_curve_to(transform.point(control), transform.point(current));
                    last_quadratic_control = Some(control);
                    last_cubic_control = None;
                }
                PathSegment::ClosePath { .. } => {
                    builder.close();
                    current = subpath;
                    last_cubic_control = None;
                    last_quadratic_control = None;
                }
                PathSegment::EllipticalArc { .. } => {
                    panic!("FPL mark arcs must be converted to cubic paths")
                }
            }
        }
    })
}

fn resolve(current: Point, absolute: bool, x: f64, y: f64) -> Point {
    let point = Point::new(x as f32, y as f32);
    if absolute {
        point
    } else {
        Point::new(current.x + point.x, current.y + point.y)
    }
}

fn reflect(control: Point, around: Point) -> Point {
    Point::new(2.0 * around.x - control.x, 2.0 * around.y - control.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_mark_is_three_font_independent_paths() {
        assert_eq!(SOURCE.matches("<path").count(), 3);
        assert!(!SOURCE.contains("<text"));
        assert!(!SOURCE.contains("<ellipse"));
        assert_eq!(PATHS.len(), 3);
    }
}
