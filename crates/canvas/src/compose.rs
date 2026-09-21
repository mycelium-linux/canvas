//! Deterministic whole-wall composition presets.
//!
//! Both presets walk the same slicing floorplan: take the open cell with the
//! largest area, give it to the next surface, and split what remains along the
//! cell's physically longer axis. Only the ratio differs — half for `Tile`,
//! the golden ratio conjugate for `Golden` — so the two modes share one
//! implementation and one body of guarantees: the cells partition the unit
//! square, exactly, in the order the surfaces were given. No randomness, no
//! history, no wall clock: same inputs, same arrangement, on every Canvas.

use crate::{NormalizedRect, Placement};
use serde::{Deserialize, Serialize};

/// The larger half of a golden two-part division (1/φ).
pub const GOLDEN_PART: f32 = 0.618_034;

/// How to arrange a document's surfaces across the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum LayoutMode {
    /// Balanced grid: repeatedly halve the largest cell along its long axis.
    Tile,
    /// Golden mosaic: repeatedly carve the largest cell at φ:1.
    Golden,
}

/// Place surfaces in a deterministic row-major grid filling the wall.
///
/// Callers choose the column count; rows grow when necessary so no surface is
/// silently omitted. Keeping this here gives catalog paging, window restore,
/// and future layout controls one geometry implementation.
pub fn grid(ids: &[String], minimum_rows: u32, columns: u32) -> Vec<Placement> {
    let columns = columns.max(1);
    let rows = (ids.len() as u32)
        .div_ceil(columns)
        .max(minimum_rows)
        .max(1);
    let width = 1.0 / columns as f32;
    let height = 1.0 / rows as f32;
    ids.iter()
        .enumerate()
        .map(|(index, surface_id)| {
            let row = index as u32 / columns;
            let column = index as u32 % columns;
            Placement {
                surface_id: surface_id.clone(),
                bounds: NormalizedRect {
                    x: column as f32 * width,
                    y: row as f32 * height,
                    width,
                    height,
                },
                layer: index as i32,
            }
        })
        .collect()
}

/// Compose surfaces, in order, into placements filling the unit square.
///
/// `aspect` is the wall's physical width/height ratio. Splitting chooses the
/// physically longer axis, so a mosaic reads the same on a 16:9 laptop and a
/// 5:3 desktop instead of distorting in normalized space. The largest open
/// cell is repeatedly split (count − 1 times) to make room; cells are then
/// assigned to surfaces biggest-first, so the first surface claims the prime
/// region and every cell is a distinct, non-overlapping part of the wall.
pub fn compose(mode: LayoutMode, ids: &[String], aspect: f32) -> Vec<Placement> {
    let ratio = match mode {
        LayoutMode::Tile => 0.5,
        LayoutMode::Golden => GOLDEN_PART,
    };
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let mut cells = vec![NormalizedRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }];
    for _ in 1..ids.len() {
        let largest = cells
            .iter()
            .enumerate()
            .fold(0usize, |best, (candidate, cell)| {
                if cell.width * cell.height > cells[best].width * cells[best].height {
                    candidate
                } else {
                    best
                }
            });
        let cell = cells.remove(largest);
        if cell.width * aspect >= cell.height {
            let split = cell.width * ratio;
            cells.push(NormalizedRect {
                x: cell.x,
                y: cell.y,
                width: split,
                height: cell.height,
            });
            cells.push(NormalizedRect {
                x: cell.x + split,
                y: cell.y,
                width: cell.width - split,
                height: cell.height,
            });
        } else {
            let split = cell.height * ratio;
            cells.push(NormalizedRect {
                x: cell.x,
                y: cell.y,
                width: cell.width,
                height: split,
            });
            cells.push(NormalizedRect {
                x: cell.x,
                y: cell.y + split,
                width: cell.width,
                height: cell.height - split,
            });
        }
    }
    // Prime region first: biggest cell, ties broken top-then-left, so the
    // layout is stable and the surface most worth reading earns the largest
    // rectangle rather than the one that happened to be sliced first.
    cells.sort_by(|a, b| {
        let area_a = a.width * a.height;
        let area_b = b.width * b.height;
        area_b
            .total_cmp(&area_a)
            .then(a.x.total_cmp(&b.x))
            .then(a.y.total_cmp(&b.y))
    });
    cells
        .into_iter()
        .zip(ids)
        .enumerate()
        .map(|(index, (bounds, surface_id))| Placement {
            surface_id: surface_id.clone(),
            bounds,
            layer: index as i32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("s{index}")).collect()
    }

    fn total_area(placements: &[Placement]) -> f32 {
        placements
            .iter()
            .map(|p| p.bounds.width * p.bounds.height)
            .sum()
    }

    fn overlap_area(a: &Placement, b: &Placement) -> f32 {
        let a_right = a.bounds.x + a.bounds.width;
        let b_right = b.bounds.x + b.bounds.width;
        let a_bottom = a.bounds.y + a.bounds.height;
        let b_bottom = b.bounds.y + b.bounds.height;
        let width = (a_right.min(b_right) - a.bounds.x.max(b.bounds.x)).max(0.0);
        let height = (a_bottom.min(b_bottom) - a.bounds.y.max(b.bounds.y)).max(0.0);
        width * height
    }

    fn bounds(p: &Placement) -> NormalizedRect {
        p.bounds
    }

    #[test]
    fn golden_mosaics_partition_the_wall_in_order() {
        for count in 1..=7 {
            let placements = compose(LayoutMode::Golden, &ids(count), 16.0 / 9.0);
            assert_eq!(placements.len(), count);
            for (index, placement) in placements.iter().enumerate() {
                assert_eq!(placement.surface_id, format!("s{index}"));
            }
            assert!(
                (total_area(&placements) - 1.0).abs() < 1e-4,
                "{count} cells must tile the unit square"
            );
            for (i, a) in placements.iter().enumerate() {
                for b in placements.iter().skip(i + 1) {
                    assert!(
                        overlap_area(a, b) < 1e-4,
                        "{} and {} overlap",
                        a.surface_id,
                        b.surface_id
                    );
                }
            }
        }
    }

    #[test]
    fn golden_first_split_takes_the_larger_phi_part() {
        let placements = compose(LayoutMode::Golden, &ids(2), 16.0 / 9.0);
        assert!(
            (placements[0].bounds.width - GOLDEN_PART).abs() < 1e-5,
            "the first surface earns the 0.618 column on a wide wall"
        );
        assert!((placements[1].bounds.width - (1.0 - GOLDEN_PART)).abs() < 1e-5);
    }

    #[test]
    fn grid_is_row_major_and_honors_a_minimum_row_count() {
        let ids = (0..5).map(|index| format!("s{index}")).collect::<Vec<_>>();
        let placements = grid(&ids, 2, 3);
        assert_eq!(placements.len(), ids.len());
        assert_eq!(placements[0].bounds.x, 0.0);
        assert_eq!(placements[2].bounds.x, 2.0 / 3.0);
        assert_eq!(placements[3].bounds.y, 0.5);
        assert!(placements.iter().all(|placement| {
            placement.bounds.width == 1.0 / 3.0 && placement.bounds.height == 0.5
        }));
    }

    #[test]
    fn tiling_four_surfaces_makes_a_two_by_two_grid() {
        let placements = compose(LayoutMode::Tile, &ids(4), 1.0);
        let mut corners: Vec<(f32, f32)> = placements
            .iter()
            .map(|p| (p.bounds.x, p.bounds.y))
            .collect();
        corners.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        assert_eq!(
            corners,
            vec![(0.0, 0.0), (0.0, 0.5), (0.5, 0.0), (0.5, 0.5)]
        );
        for placement in &placements {
            assert!((placement.bounds.width - 0.5).abs() < 1e-6);
            assert!((placement.bounds.height - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn compose_is_deterministic_and_empty_safe() {
        assert!(compose(LayoutMode::Golden, &[], 1.0).is_empty());
        let a = compose(LayoutMode::Golden, &ids(5), 2.0);
        let b = compose(LayoutMode::Golden, &ids(5), 2.0);
        assert_eq!(
            a.iter().map(bounds).collect::<Vec<_>>(),
            b.iter().map(bounds).collect::<Vec<_>>()
        );
    }

    #[test]
    fn bad_aspect_falls_back_instead_of_poisoning_geometry() {
        for aspect in [0.0, -3.0, f32::NAN] {
            let placements = compose(LayoutMode::Tile, &ids(2), aspect);
            assert!(placements.iter().all(|p| p.bounds.validate().is_ok()));
        }
    }
}
