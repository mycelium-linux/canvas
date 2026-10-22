//! A projected, pickable orientation cube.
//!
//! This is deliberately geometry, not a grid of controls styled to resemble
//! geometry. Faces, edge bevels and cut corners are the same polygons for
//! painting and hit testing, so hover can never disagree with click.

use canvas_core::VisualTheme;
use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

const HALF: f32 = 1.0;
const BEVEL: f32 = 0.66;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionKind {
    Face,
    Edge,
    Corner,
}

#[derive(Debug, Clone)]
struct Region {
    view: &'static str,
    kind: RegionKind,
    vertices: Vec<[f32; 3]>,
}

#[derive(Debug, Clone)]
struct ProjectedRegion {
    view: &'static str,
    kind: RegionKind,
    points: Vec<Point>,
    depth: f32,
}

pub(super) fn view<'a, Message: Clone + 'a>(
    visual: VisualTheme,
    yaw: f32,
    pitch: f32,
    selected: Option<&'static str>,
    on_pick: impl Fn(&'static str) -> Message + 'a,
) -> Element<'a, Message> {
    Canvas::new(ViewCube {
        visual,
        yaw,
        pitch,
        selected,
        on_pick: Box::new(on_pick),
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct ViewCube<'a, Message> {
    visual: VisualTheme,
    yaw: f32,
    pitch: f32,
    selected: Option<&'static str>,
    on_pick: Box<dyn Fn(&'static str) -> Message + 'a>,
}

impl<Message> canvas::Program<Message> for ViewCube<'_, Message> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        if !matches!(
            event,
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) {
            return None;
        }
        let pointer = cursor.position_in(bounds)?;
        let regions = projected_regions(bounds, self.yaw, self.pitch);
        let region = pick(&regions, pointer)?;
        Some(iced::widget::Action::publish((self.on_pick)(region.view)))
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let regions = projected_regions(bounds, self.yaw, self.pitch);
        let hovered = cursor
            .position_in(bounds)
            .and_then(|pointer| pick(&regions, pointer))
            .map(|region| region.view);
        let palette = self.visual.palette;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        for region in &regions {
            let path = polygon(&region.points);
            let active = hovered == Some(region.view) || self.selected == Some(region.view);
            let base = match region.kind {
                RegionKind::Face => palette.surface,
                RegionKind::Edge => palette.surface_muted,
                RegionKind::Corner => palette.background_strong,
            };
            frame.fill(
                &path,
                if active {
                    rgb(palette.primary, 0.96)
                } else {
                    rgb(base, 0.92)
                },
            );
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_width(if active { 2.0 } else { 1.0 })
                    .with_color(rgb(
                        if active {
                            palette.text
                        } else {
                            palette.border_strong
                        },
                        if active { 0.95 } else { 0.72 },
                    )),
            );
            if region.kind == RegionKind::Face
                && face_label_visible(region.view, self.yaw, self.pitch)
            {
                frame.fill_text(Text {
                    content: region.view.to_ascii_uppercase(),
                    position: centroid(&region.points),
                    color: rgb(
                        if active {
                            palette.background
                        } else {
                            palette.text
                        },
                        0.9,
                    ),
                    size: iced::Pixels(11.0),
                    align_x: iced::alignment::Horizontal::Center.into(),
                    align_y: iced::alignment::Vertical::Center,
                    ..Text::default()
                });
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let regions = projected_regions(bounds, self.yaw, self.pitch);
        let pickable = cursor
            .position_in(bounds)
            .is_some_and(|pointer| pick(&regions, pointer).is_some());
        if pickable {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

fn centroid(points: &[Point]) -> Point {
    let (x, y) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), point| (x + point.x, y + point.y));
    let count = points.len().max(1) as f32;
    Point::new(x / count, y / count)
}

/// Text is not clipped by iced Canvas paths, so painter ordering alone cannot
/// prevent a rear-face label bleeding through the solid. Gate labels by the
/// rotated outward normal and suppress them near edge-on, where the face is too
/// narrow to carry readable type anyway.
fn face_label_visible(view: &str, yaw: f32, pitch: f32) -> bool {
    let normal = match view {
        "left" => [-1.0, 0.0, 0.0],
        "right" => [1.0, 0.0, 0.0],
        "bottom" => [0.0, -1.0, 0.0],
        "top" => [0.0, 1.0, 0.0],
        "back" => [0.0, 0.0, -1.0],
        "front" => [0.0, 0.0, 1.0],
        _ => return false,
    };
    rotate(normal, yaw, pitch)[2] > 0.16
}

fn rgb(value: [u8; 3], alpha: f32) -> Color {
    Color::from_rgba8(value[0], value[1], value[2], alpha)
}

fn polygon(points: &[Point]) -> Path {
    Path::new(|builder| {
        if let Some(first) = points.first() {
            builder.move_to(*first);
            for point in &points[1..] {
                builder.line_to(*point);
            }
            builder.close();
        }
    })
}

fn projected_regions(bounds: Rectangle, yaw: f32, pitch: f32) -> Vec<ProjectedRegion> {
    let radius = bounds.width.min(bounds.height) * 0.36;
    let center = Point::new(bounds.width * 0.5, bounds.height * 0.5);
    let mut projected = regions()
        .into_iter()
        .map(|region| {
            let rotated: Vec<_> = region
                .vertices
                .iter()
                .copied()
                .map(|vertex| rotate(vertex, yaw, pitch))
                .collect();
            let depth = rotated.iter().map(|point| point[2]).sum::<f32>() / rotated.len() as f32;
            let points = rotated
                .into_iter()
                .map(|point| Point::new(center.x + point[0] * radius, center.y - point[1] * radius))
                .collect();
            ProjectedRegion {
                view: region.view,
                kind: region.kind,
                points,
                depth,
            }
        })
        .collect::<Vec<_>>();
    // Painter's algorithm: rear surfaces first. Picking walks this in reverse.
    projected.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    projected
}

fn rotate([x, y, z]: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let x1 = cy * x - sy * z;
    let z1 = sy * x + cy * z;
    [x1, cp * y - sp * z1, sp * y + cp * z1]
}

fn pick(regions: &[ProjectedRegion], point: Point) -> Option<&ProjectedRegion> {
    regions
        .iter()
        .rev()
        .find(|region| contains(&region.points, point))
}

fn contains(polygon: &[Point], point: Point) -> bool {
    let mut inside = false;
    let mut previous = polygon.last().copied().unwrap_or(Point::ORIGIN);
    for current in polygon.iter().copied() {
        if ((current.y > point.y) != (previous.y > point.y))
            && point.x
                < (previous.x - current.x) * (point.y - current.y) / (previous.y - current.y)
                    + current.x
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn regions() -> Vec<Region> {
    let mut result = Vec::with_capacity(26);
    for (axis, negative, positive) in [
        (0, "left", "right"),
        (1, "bottom", "top"),
        (2, "back", "front"),
    ] {
        for (sign, view) in [(-1.0, negative), (1.0, positive)] {
            let others: Vec<_> = (0..3).filter(|candidate| *candidate != axis).collect();
            let mut vertices = Vec::with_capacity(4);
            for (a, b) in [
                (-BEVEL, -BEVEL),
                (BEVEL, -BEVEL),
                (BEVEL, BEVEL),
                (-BEVEL, BEVEL),
            ] {
                let mut vertex = [0.0; 3];
                vertex[axis] = sign * HALF;
                vertex[others[0]] = a;
                vertex[others[1]] = b;
                vertices.push(vertex);
            }
            result.push(Region {
                view,
                kind: RegionKind::Face,
                vertices,
            });
        }
    }
    for free in 0..3 {
        let fixed: Vec<_> = (0..3).filter(|axis| *axis != free).collect();
        for sa in [-1.0, 1.0] {
            for sb in [-1.0, 1.0] {
                let mut vertices = Vec::with_capacity(4);
                for (along, outer_a, outer_b) in [
                    (-BEVEL, HALF, BEVEL),
                    (BEVEL, HALF, BEVEL),
                    (BEVEL, BEVEL, HALF),
                    (-BEVEL, BEVEL, HALF),
                ] {
                    let mut vertex = [0.0; 3];
                    vertex[free] = along;
                    vertex[fixed[0]] = sa * outer_a;
                    vertex[fixed[1]] = sb * outer_b;
                    vertices.push(vertex);
                }
                result.push(Region {
                    view: edge_view(free, sa, sb),
                    kind: RegionKind::Edge,
                    vertices,
                });
            }
        }
    }
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                result.push(Region {
                    view: corner_view(sx, sy, sz),
                    kind: RegionKind::Corner,
                    vertices: vec![
                        [sx * HALF, sy * BEVEL, sz * BEVEL],
                        [sx * BEVEL, sy * HALF, sz * BEVEL],
                        [sx * BEVEL, sy * BEVEL, sz * HALF],
                    ],
                });
            }
        }
    }
    result
}

fn edge_view(free: usize, a: f32, b: f32) -> &'static str {
    match (free, a > 0.0, b > 0.0) {
        (0, false, false) => "bottom_back",
        (0, false, true) => "bottom_front",
        (0, true, false) => "top_back",
        (0, true, true) => "top_front",
        (1, false, false) => "back_left",
        (1, false, true) => "front_left",
        (1, true, false) => "back_right",
        (1, true, true) => "front_right",
        (2, false, false) => "bottom_left",
        (2, false, true) => "bottom_right",
        (2, true, false) => "top_left",
        (2, true, true) => "top_right",
        _ => unreachable!("axis is generated in 0..3"),
    }
}

fn corner_view(x: f32, y: f32, z: f32) -> &'static str {
    match (x > 0.0, y > 0.0, z > 0.0) {
        (true, true, true) => "iso",
        (false, true, true) => "iso_front_left",
        (true, true, false) => "iso_back_right",
        (false, true, false) => "iso_rear",
        (true, false, true) => "iso_bottom",
        (false, false, true) => "iso_bottom_left",
        (true, false, false) => "iso_bottom_back_right",
        (false, false, false) => "iso_bottom_rear",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_has_six_faces_twelve_edges_and_eight_cut_corners() {
        let model = regions();
        assert_eq!(model.len(), 26);
        assert_eq!(
            model.iter().filter(|r| r.kind == RegionKind::Face).count(),
            6
        );
        assert_eq!(
            model.iter().filter(|r| r.kind == RegionKind::Edge).count(),
            12
        );
        assert_eq!(
            model
                .iter()
                .filter(|r| r.kind == RegionKind::Corner)
                .count(),
            8
        );
    }

    #[test]
    fn center_pick_tracks_camera_orientation() {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(132.0, 132.0));
        let center = Point::new(66.0, 66.0);
        assert_eq!(
            pick(&projected_regions(bounds, 0.0, 0.0), center).map(|r| r.view),
            Some("front")
        );
        assert_eq!(
            pick(&projected_regions(bounds, 90.0, 0.0), center).map(|r| r.view),
            Some("right")
        );
        assert_eq!(
            pick(&projected_regions(bounds, -90.0, 0.0), center).map(|r| r.view),
            Some("left")
        );
    }

    #[test]
    fn opposite_face_labels_are_mutually_occluded() {
        assert!(face_label_visible("front", 0.0, 0.0));
        assert!(!face_label_visible("back", 0.0, 0.0));
        assert!(face_label_visible("back", 180.0, 0.0));
        assert!(!face_label_visible("front", 180.0, 0.0));
        assert!(face_label_visible("right", 90.0, 0.0));
        assert!(!face_label_visible("left", 90.0, 0.0));
    }

    #[test]
    fn edge_on_faces_do_not_float_a_label() {
        assert!(!face_label_visible("front", 90.0, 0.0));
        assert!(!face_label_visible("top", 0.0, 0.0));
    }

    #[test]
    fn every_pick_target_is_a_core_named_view() {
        for region in regions() {
            assert!(
                canvas_core::model_camera::named_view(region.view).is_some(),
                "{}",
                region.view
            );
        }
    }

    #[test]
    fn hover_hit_test_includes_bevel_regions() {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(132.0, 132.0));
        let projected = projected_regions(bounds, 32.0, 24.0);
        for kind in [RegionKind::Face, RegionKind::Edge, RegionKind::Corner] {
            let independently_pickable =
                projected
                    .iter()
                    .filter(|region| region.kind == kind)
                    .any(|region| {
                        let centroid = Point::new(
                            region.points.iter().map(|p| p.x).sum::<f32>()
                                / region.points.len() as f32,
                            region.points.iter().map(|p| p.y).sum::<f32>()
                                / region.points.len() as f32,
                        );
                        pick(&projected, centroid).is_some_and(|picked| picked.view == region.view)
                    });
            assert!(independently_pickable, "no visible {kind:?} was pickable");
        }
    }
}
