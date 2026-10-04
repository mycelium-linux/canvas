//! Deterministic geometry and mutations for Canvas's durable layout tree.
//!
//! The workspace document stores intent (`Split`, `Stack`, or freeform
//! `Canvas`).  Pixel rectangles are derived here from the current output
//! extent; they are never persisted, so moving a workspace between displays
//! cannot retain stale dimensions from the previous output.

use crate::{Axis, Layout, NormalizedRect, Placement};
use canvas_core::layout::Rect;
use std::fmt;

pub const MIN_SPLIT_RATIO: f32 = 0.05;
pub const MAX_SPLIT_RATIO: f32 = 0.95;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError(String);

impl LayoutError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for LayoutError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolvedPlacement {
    pub surface_id: String,
    pub frame: Rect,
    pub layer: i32,
    /// False for parked members of a stack. They retain their allocation and
    /// stream state but should not be presented until selected.
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSide {
    Before,
    After,
}

/// Flatten a durable tree into normalized presentation rectangles.
///
/// Stack members intentionally share a rectangle. Consumers that paint every
/// member preserve document order as z-order; consumers that only need the
/// visible member can use [`solve`].
pub fn placements(layout: &Layout) -> Vec<Placement> {
    fn visit(layout: &Layout, bounds: NormalizedRect, output: &mut Vec<Placement>) {
        match layout {
            Layout::Empty => {}
            Layout::Surface { surface_id } => output.push(Placement {
                surface_id: surface_id.clone(),
                bounds,
                layer: output.len() as i32,
            }),
            Layout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_bounds, second_bounds) = split_normalized(bounds, *axis, *ratio);
                visit(first, first_bounds, output);
                visit(second, second_bounds, output);
            }
            Layout::Stack { surface_ids, .. } => {
                for surface_id in surface_ids {
                    output.push(Placement {
                        surface_id: surface_id.clone(),
                        bounds,
                        layer: output.len() as i32,
                    });
                }
            }
            Layout::Canvas {
                placements: existing,
            } => output.extend(existing.iter().cloned()),
        }
    }

    let mut output = Vec::new();
    visit(
        layout,
        NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        &mut output,
    );
    output
}

/// Resolve layout intent into exact integer pixel rectangles.
///
/// Split edges are rounded once and shared by both children. This is the
/// invariant that prevents diagonal scanline/shape artifacts: adjacent panes
/// always agree on the same physical boundary, at every output resolution.
pub fn solve(layout: &Layout, area: Rect) -> Result<Vec<SolvedPlacement>, LayoutError> {
    validate_area(area)?;

    fn visit(
        layout: &Layout,
        frame: Rect,
        output: &mut Vec<SolvedPlacement>,
    ) -> Result<(), LayoutError> {
        match layout {
            Layout::Empty => Ok(()),
            Layout::Surface { surface_id } => {
                output.push(SolvedPlacement {
                    surface_id: surface_id.clone(),
                    frame,
                    layer: output.len() as i32,
                    visible: true,
                });
                Ok(())
            }
            Layout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_frame, second_frame) = split_pixels(frame, *axis, *ratio)?;
                visit(first, first_frame, output)?;
                visit(second, second_frame, output)
            }
            Layout::Stack {
                surface_ids,
                active_surface_id,
            } => {
                for surface_id in surface_ids {
                    output.push(SolvedPlacement {
                        surface_id: surface_id.clone(),
                        frame,
                        layer: output.len() as i32,
                        visible: surface_id == active_surface_id,
                    });
                }
                Ok(())
            }
            Layout::Canvas { placements } => {
                for placement in placements {
                    output.push(SolvedPlacement {
                        surface_id: placement.surface_id.clone(),
                        frame: normalized_to_pixels(placement.bounds, frame)?,
                        layer: placement.layer,
                        visible: true,
                    });
                }
                Ok(())
            }
        }
    }

    let mut output = Vec::new();
    visit(layout, area, &mut output)?;
    Ok(output)
}

/// Remove structurally empty branches and degenerate stacks.
///
/// This is deliberately conservative: a binary split remains binary and its
/// ratio remains authored intent. We do not rewrite consecutive same-axis
/// splits into a subtly different weighted layout.
pub fn normalize(layout: &mut Layout) {
    match layout {
        Layout::Split { first, second, .. } => {
            normalize(first);
            normalize(second);
            let replacement = match (&**first, &**second) {
                (Layout::Empty, _) => Some((**second).clone()),
                (_, Layout::Empty) => Some((**first).clone()),
                _ => None,
            };
            if let Some(replacement) = replacement {
                *layout = replacement;
            }
        }
        Layout::Stack {
            surface_ids,
            active_surface_id,
        } => match surface_ids.as_slice() {
            [] => *layout = Layout::Empty,
            [surface_id] => {
                *layout = Layout::Surface {
                    surface_id: surface_id.clone(),
                };
            }
            _ if !surface_ids.contains(active_surface_id) => {
                *active_surface_id = surface_ids[0].clone();
            }
            _ => {}
        },
        Layout::Empty | Layout::Surface { .. } | Layout::Canvas { .. } => {}
    }
}

/// Replace one leaf with a binary split containing it and a new surface.
pub fn split_surface(
    layout: &mut Layout,
    target_surface_id: &str,
    new_surface_id: impl Into<String>,
    axis: Axis,
    ratio: f32,
    side: SplitSide,
) -> Result<(), LayoutError> {
    if !ratio.is_finite() || !(MIN_SPLIT_RATIO..=MAX_SPLIT_RATIO).contains(&ratio) {
        return Err(LayoutError::new(format!(
            "split ratio must be between {MIN_SPLIT_RATIO} and {MAX_SPLIT_RATIO}"
        )));
    }
    let new_surface_id = new_surface_id.into();
    if new_surface_id.is_empty() || contains_surface(layout, &new_surface_id) {
        return Err(LayoutError::new(
            "new surface ID must be nonempty and unique",
        ));
    }

    fn replace(
        node: &mut Layout,
        target: &str,
        new_surface: &str,
        axis: Axis,
        ratio: f32,
        side: SplitSide,
    ) -> bool {
        match node {
            Layout::Surface { surface_id } if surface_id == target => {
                let old = std::mem::replace(node, Layout::Empty);
                let new = Layout::Surface {
                    surface_id: new_surface.to_owned(),
                };
                let (first, second) = match side {
                    SplitSide::Before => (new, old),
                    SplitSide::After => (old, new),
                };
                *node = Layout::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Layout::Split { first, second, .. } => {
                replace(first, target, new_surface, axis, ratio, side)
                    || replace(second, target, new_surface, axis, ratio, side)
            }
            Layout::Empty
            | Layout::Surface { .. }
            | Layout::Stack { .. }
            | Layout::Canvas { .. } => false,
        }
    }

    if replace(
        layout,
        target_surface_id,
        &new_surface_id,
        axis,
        ratio,
        side,
    ) {
        Ok(())
    } else {
        Err(LayoutError::new(format!(
            "surface {target_surface_id:?} is not a tiled leaf"
        )))
    }
}

/// Reset every split to an even division without changing tree topology.
pub fn balance(layout: &mut Layout) {
    if let Layout::Split {
        ratio,
        first,
        second,
        ..
    } = layout
    {
        *ratio = 0.5;
        balance(first);
        balance(second);
    }
}

/// Grow or shrink the focused surface at its nearest split boundary.
///
/// Positive deltas grow the target regardless of which side of the split it
/// occupies. Ratios remain inside the same bounds enforced by the document
/// validator.
pub fn resize_nearest(
    layout: &mut Layout,
    target_surface_id: &str,
    delta: f32,
) -> Result<(), LayoutError> {
    if !delta.is_finite() || delta == 0.0 {
        return Err(LayoutError::new("resize delta must be finite and nonzero"));
    }

    #[derive(Clone, Copy)]
    enum Search {
        Missing,
        NeedsParent,
        Adjusted,
    }

    fn resize(node: &mut Layout, target: &str, delta: f32) -> Search {
        match node {
            Layout::Surface { surface_id } if surface_id == target => Search::NeedsParent,
            Layout::Stack { surface_ids, .. }
                if surface_ids.iter().any(|surface_id| surface_id == target) =>
            {
                Search::NeedsParent
            }
            Layout::Split {
                ratio,
                first,
                second,
                ..
            } => match resize(first, target, delta) {
                Search::Adjusted => Search::Adjusted,
                Search::NeedsParent => {
                    *ratio = (*ratio + delta).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                    Search::Adjusted
                }
                Search::Missing => match resize(second, target, delta) {
                    Search::Adjusted => Search::Adjusted,
                    Search::NeedsParent => {
                        *ratio = (*ratio - delta).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                        Search::Adjusted
                    }
                    Search::Missing => Search::Missing,
                },
            },
            Layout::Empty
            | Layout::Surface { .. }
            | Layout::Stack { .. }
            | Layout::Canvas { .. } => Search::Missing,
        }
    }

    match resize(layout, target_surface_id, delta) {
        Search::Adjusted => Ok(()),
        Search::NeedsParent => Err(LayoutError::new("surface has no split boundary to resize")),
        Search::Missing => Err(LayoutError::new(format!(
            "surface {target_surface_id:?} is not in a tiled branch"
        ))),
    }
}

/// Toggle the axis of the target's nearest split container.
pub fn rotate_nearest(layout: &mut Layout, target_surface_id: &str) -> Result<(), LayoutError> {
    #[derive(Clone, Copy)]
    enum Search {
        Missing,
        NeedsParent,
        Rotated,
    }

    fn rotate(node: &mut Layout, target: &str) -> Search {
        match node {
            Layout::Surface { surface_id } if surface_id == target => Search::NeedsParent,
            Layout::Stack { surface_ids, .. }
                if surface_ids.iter().any(|surface_id| surface_id == target) =>
            {
                Search::NeedsParent
            }
            Layout::Split {
                axis,
                first,
                second,
                ..
            } => match rotate(first, target) {
                Search::Rotated => Search::Rotated,
                Search::NeedsParent => {
                    *axis = opposite(*axis);
                    Search::Rotated
                }
                Search::Missing => match rotate(second, target) {
                    Search::Rotated => Search::Rotated,
                    Search::NeedsParent => {
                        *axis = opposite(*axis);
                        Search::Rotated
                    }
                    Search::Missing => Search::Missing,
                },
            },
            Layout::Empty
            | Layout::Surface { .. }
            | Layout::Stack { .. }
            | Layout::Canvas { .. } => Search::Missing,
        }
    }

    match rotate(layout, target_surface_id) {
        Search::Rotated => Ok(()),
        Search::NeedsParent => Err(LayoutError::new("surface has no split container to rotate")),
        Search::Missing => Err(LayoutError::new(format!(
            "surface {target_surface_id:?} is not in a tiled branch"
        ))),
    }
}

pub fn contains_surface(layout: &Layout, target: &str) -> bool {
    match layout {
        Layout::Empty => false,
        Layout::Surface { surface_id } => surface_id == target,
        Layout::Split { first, second, .. } => {
            contains_surface(first, target) || contains_surface(second, target)
        }
        Layout::Stack { surface_ids, .. } => surface_ids.iter().any(|id| id == target),
        Layout::Canvas { placements } => placements.iter().any(|p| p.surface_id == target),
    }
}

fn split_normalized(
    bounds: NormalizedRect,
    axis: Axis,
    ratio: f32,
) -> (NormalizedRect, NormalizedRect) {
    let ratio = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
    match axis {
        Axis::Horizontal => (
            NormalizedRect {
                width: bounds.width * ratio,
                ..bounds
            },
            NormalizedRect {
                x: bounds.x + bounds.width * ratio,
                width: bounds.width * (1.0 - ratio),
                ..bounds
            },
        ),
        Axis::Vertical => (
            NormalizedRect {
                height: bounds.height * ratio,
                ..bounds
            },
            NormalizedRect {
                y: bounds.y + bounds.height * ratio,
                height: bounds.height * (1.0 - ratio),
                ..bounds
            },
        ),
    }
}

const fn opposite(axis: Axis) -> Axis {
    match axis {
        Axis::Horizontal => Axis::Vertical,
        Axis::Vertical => Axis::Horizontal,
    }
}

fn split_pixels(frame: Rect, axis: Axis, ratio: f32) -> Result<(Rect, Rect), LayoutError> {
    if !ratio.is_finite() || !(MIN_SPLIT_RATIO..=MAX_SPLIT_RATIO).contains(&ratio) {
        return Err(LayoutError::new("invalid split ratio"));
    }
    let extent = match axis {
        Axis::Horizontal => frame.width,
        Axis::Vertical => frame.height,
    };
    if extent < 2 {
        return Err(LayoutError::new(
            "split extent must contain at least two pixels",
        ));
    }
    let first_extent = ((f64::from(extent) * f64::from(ratio)).round() as u32).clamp(1, extent - 1);
    let second_extent = extent - first_extent;
    match axis {
        Axis::Horizontal => {
            let edge = frame
                .x
                .checked_add(
                    i32::try_from(first_extent).map_err(|_| LayoutError::new("width overflow"))?,
                )
                .ok_or_else(|| LayoutError::new("horizontal edge overflow"))?;
            Ok((
                Rect {
                    width: first_extent,
                    ..frame
                },
                Rect {
                    x: edge,
                    width: second_extent,
                    ..frame
                },
            ))
        }
        Axis::Vertical => {
            let edge = frame
                .y
                .checked_add(
                    i32::try_from(first_extent).map_err(|_| LayoutError::new("height overflow"))?,
                )
                .ok_or_else(|| LayoutError::new("vertical edge overflow"))?;
            Ok((
                Rect {
                    height: first_extent,
                    ..frame
                },
                Rect {
                    y: edge,
                    height: second_extent,
                    ..frame
                },
            ))
        }
    }
}

fn normalized_to_pixels(bounds: NormalizedRect, area: Rect) -> Result<Rect, LayoutError> {
    if ![bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite())
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(LayoutError::new("invalid normalized rectangle"));
    }
    let left = f64::from(area.x) + f64::from(area.width) * f64::from(bounds.x);
    let top = f64::from(area.y) + f64::from(area.height) * f64::from(bounds.y);
    let right = left + f64::from(area.width) * f64::from(bounds.width);
    let bottom = top + f64::from(area.height) * f64::from(bounds.height);
    let left = left.round();
    let top = top.round();
    let right = right.round();
    let bottom = bottom.round();
    Ok(Rect {
        x: left as i32,
        y: top as i32,
        width: (right - left).max(1.0) as u32,
        height: (bottom - top).max(1.0) as u32,
    })
}

fn validate_area(area: Rect) -> Result<(), LayoutError> {
    if area.width == 0 || area.height == 0 {
        return Err(LayoutError::new("layout area must be nonempty"));
    }
    let width = i32::try_from(area.width).map_err(|_| LayoutError::new("area width overflow"))?;
    let height =
        i32::try_from(area.height).map_err(|_| LayoutError::new("area height overflow"))?;
    area.x
        .checked_add(width)
        .ok_or_else(|| LayoutError::new("area right edge overflow"))?;
    area.y
        .checked_add(height)
        .ok_or_else(|| LayoutError::new("area bottom edge overflow"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: &str) -> Layout {
        Layout::Surface {
            surface_id: id.into(),
        }
    }

    fn sample() -> Layout {
        Layout::Split {
            axis: Axis::Horizontal,
            ratio: 0.68,
            first: Box::new(surface("surf")),
            second: Box::new(Layout::Split {
                axis: Axis::Vertical,
                ratio: 0.55,
                first: Box::new(surface("hud")),
                second: Box::new(surface("stdout")),
            }),
        }
    }

    #[test]
    fn shared_edges_cover_every_pixel_at_tv_and_phone_aspects() {
        for area in [
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            Rect {
                x: 0,
                y: 0,
                width: 3840,
                height: 2160,
            },
            Rect {
                x: -430,
                y: 20,
                width: 430,
                height: 932,
            },
        ] {
            let solved = solve(&sample(), area).unwrap();
            let surf = solved
                .iter()
                .find(|p| p.surface_id == "surf")
                .unwrap()
                .frame;
            let hud = solved.iter().find(|p| p.surface_id == "hud").unwrap().frame;
            let stdout = solved
                .iter()
                .find(|p| p.surface_id == "stdout")
                .unwrap()
                .frame;
            assert_eq!(surf.x + surf.width as i32, hud.x);
            assert_eq!(hud.x, stdout.x);
            assert_eq!(hud.y + hud.height as i32, stdout.y);
            assert_eq!(surf.width + hud.width, area.width);
            assert_eq!(hud.height + stdout.height, area.height);
        }
    }

    #[test]
    fn stack_allocates_all_members_but_presents_only_active() {
        let layout = Layout::Stack {
            surface_ids: vec!["a".into(), "b".into()],
            active_surface_id: "b".into(),
        };
        let solved = solve(
            &layout,
            Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        )
        .unwrap();
        assert_eq!(solved[0].frame, solved[1].frame);
        assert!(!solved[0].visible);
        assert!(solved[1].visible);
    }

    #[test]
    fn normalization_eliminates_empty_branches_and_degenerate_stacks() {
        let mut layout = Layout::Split {
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(Layout::Empty),
            second: Box::new(Layout::Stack {
                surface_ids: vec!["surf".into()],
                active_surface_id: "surf".into(),
            }),
        };
        normalize(&mut layout);
        assert_eq!(layout, surface("surf"));
    }

    #[test]
    fn splitting_and_balancing_preserve_binary_topology() {
        let mut layout = surface("surf");
        split_surface(
            &mut layout,
            "surf",
            "hud",
            Axis::Vertical,
            0.7,
            SplitSide::After,
        )
        .unwrap();
        assert!(contains_surface(&layout, "surf"));
        assert!(contains_surface(&layout, "hud"));
        balance(&mut layout);
        assert!(matches!(layout, Layout::Split { ratio: 0.5, .. }));
        assert!(
            split_surface(
                &mut layout,
                "surf",
                "hud",
                Axis::Horizontal,
                0.5,
                SplitSide::Before,
            )
            .is_err()
        );
    }

    #[test]
    fn resize_and_rotate_choose_the_nearest_split() {
        let mut layout = sample();
        resize_nearest(&mut layout, "stdout", 0.1).unwrap();
        assert!(matches!(
            &layout,
            Layout::Split { second, .. }
                if matches!(&**second, Layout::Split { ratio, .. } if (*ratio - 0.45).abs() < f32::EPSILON)
        ));
        rotate_nearest(&mut layout, "stdout").unwrap();
        assert!(matches!(
            &layout,
            Layout::Split { second, .. }
                if matches!(&**second, Layout::Split { axis: Axis::Horizontal, .. })
        ));
    }

    #[test]
    fn invalid_geometry_fails_loudly() {
        assert!(
            solve(
                &sample(),
                Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 1080
                }
            )
            .is_err()
        );
        let invalid = Layout::Split {
            axis: Axis::Horizontal,
            ratio: f32::NAN,
            first: Box::new(surface("a")),
            second: Box::new(surface("b")),
        };
        assert!(
            solve(
                &invalid,
                Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080
                }
            )
            .is_err()
        );
    }
}
