//! Pure interactive geometry. Canvas owns the resulting rectangle; a gesture
//! retains only its starting rectangle and pointer position, not a new layout.
use canvas_core::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    Low,
    High,
}

#[derive(Clone, Copy, Debug)]
pub struct ResizeEdges {
    pub horizontal: Edge,
    pub vertical: Edge,
}

#[derive(Clone, Copy, Debug)]
pub struct WindowDrag {
    pub frame: Rect,
    pub start: (f64, f64),
    pub edges: Option<ResizeEdges>,
}

impl WindowDrag {
    pub fn frame_at(self, location: (f64, f64), min: (i32, i32), max: (i32, i32)) -> Rect {
        let delta = |value: f64, start: f64| {
            if value.is_finite() && start.is_finite() {
                (value - start)
                    .round()
                    .clamp(i32::MIN as f64, i32::MAX as f64) as i64
            } else {
                0
            }
        };
        let dx = delta(location.0, self.start.0);
        let dy = delta(location.1, self.start.1);
        let mut frame = self.frame;
        if let Some(edges) = self.edges {
            let (x, width) = resize_axis(frame.x, frame.width, dx, edges.horizontal, min.0, max.0);
            let (y, height) = resize_axis(frame.y, frame.height, dy, edges.vertical, min.1, max.1);
            frame = Rect {
                x,
                y,
                width,
                height,
            };
        } else {
            frame.x = bounded_position(i64::from(frame.x) + dx);
            frame.y = bounded_position(i64::from(frame.y) + dy);
        }
        frame
    }
}

fn bounded_position(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn resize_axis(origin: i32, size: u32, delta: i64, edge: Edge, min: i32, max: i32) -> (i32, u32) {
    if edge == Edge::None {
        return (origin, size);
    }
    let minimum = i64::from(min.max(1));
    let maximum = if max > 0 {
        i64::from(max).max(minimum)
    } else {
        i64::from(i32::MAX)
    };
    let desired = i64::from(size) + if edge == Edge::Low { -delta } else { delta };
    let resized = desired.clamp(minimum, maximum);
    let origin = if edge == Edge::Low {
        bounded_position(i64::from(origin) + i64::from(size) - resized)
    } else {
        origin
    };
    (origin, resized as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drag(edges: Option<ResizeEdges>) -> WindowDrag {
        WindowDrag {
            frame: Rect {
                x: 100,
                y: 200,
                width: 400,
                height: 300,
            },
            start: (10.0, 20.0),
            edges,
        }
    }
    #[test]
    fn moving_preserves_size_and_uses_original_not_accumulated_delta() {
        let drag = drag(None);
        let frame = drag.frame_at((50.0, 70.0), (0, 0), (0, 0));
        assert_eq!(
            frame,
            Rect {
                x: 140,
                y: 250,
                width: 400,
                height: 300
            }
        );
        assert_eq!(drag.frame_at((10.0, 20.0), (0, 0), (0, 0)), drag.frame);
    }
    #[test]
    fn every_resize_edge_preserves_the_opposite_anchor() {
        for horizontal in [Edge::None, Edge::Low, Edge::High] {
            for vertical in [Edge::None, Edge::Low, Edge::High] {
                let drag = drag(Some(ResizeEdges {
                    horizontal,
                    vertical,
                }));
                let frame = drag.frame_at((50.0, 70.0), (0, 0), (0, 0));
                assert_eq!(
                    frame.width,
                    match horizontal {
                        Edge::None => 400,
                        Edge::Low => 360,
                        Edge::High => 440,
                    }
                );
                assert_eq!(
                    frame.height,
                    match vertical {
                        Edge::None => 300,
                        Edge::Low => 250,
                        Edge::High => 350,
                    }
                );
                assert_eq!(frame.x, if horizontal == Edge::Low { 140 } else { 100 });
                assert_eq!(frame.y, if vertical == Edge::Low { 250 } else { 200 });
            }
        }
    }
    #[test]
    fn limits_clamp_size_without_moving_the_opposite_edge() {
        let drag = drag(Some(ResizeEdges {
            horizontal: Edge::Low,
            vertical: Edge::High,
        }));
        let frame = drag.frame_at((1000.0, 1000.0), (120, 80), (600, 400));
        assert_eq!(
            frame,
            Rect {
                x: 380,
                y: 200,
                width: 120,
                height: 400
            }
        );
        let frame = drag.frame_at((-1000.0, -1000.0), (120, 80), (600, 400));
        assert_eq!(
            frame,
            Rect {
                x: -100,
                y: 200,
                width: 600,
                height: 80
            }
        );
    }
    #[test]
    fn unconstrained_zero_limits_still_require_positive_bounded_dimensions() {
        let drag = drag(Some(ResizeEdges {
            horizontal: Edge::High,
            vertical: Edge::Low,
        }));
        let frame = drag.frame_at((-1e20, 1e20), (0, 0), (0, 0));
        assert_eq!((frame.width, frame.height), (1, 1));
        let frame = drag.frame_at((1e20, -1e20), (0, 0), (0, 0));
        assert_eq!(
            (frame.width, frame.height),
            (i32::MAX as u32, i32::MAX as u32)
        );
        let frame = drag.frame_at((f64::NAN, f64::INFINITY), (0, 0), (0, 0));
        assert_eq!(frame, drag.frame);
    }
}
