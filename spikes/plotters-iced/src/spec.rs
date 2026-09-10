//! The plot description a Lua script produces. This is the interface the
//! spike is really testing: it is the thing a dynasim author writes against,
//! and the thing a Rust renderer consumes. Everything here is data — no
//! callbacks, no handles, no way for a script to reach the renderer.

use crate::palette::Role;

/// Same cap Canvas already applies to Lua series
/// (crates/canvas/src/lua_module.rs:25). On adoption this is imported, not
/// redefined.
pub const MAX_SERIES_POINTS: usize = 4_096;
/// Adapter source cap, matching the 32KB binder budget.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
pub const MAX_SERIES: usize = 8;
pub const MAX_LABEL_BYTES: usize = 64;
/// A tick count above this is unreadable, not a preference.
pub const MAX_TICKS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Axis {
    /// Derived from the data at draw time. The default, per tenet 3: a
    /// hand-maintained range drifts the moment the simulation changes.
    Auto,
    Fixed(f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesKind {
    Line,
    Area,
    Points,
}

impl SeriesKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "line" => Self::Line,
            "area" => Self::Area,
            "points" => Self::Points,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub color: Role,
    pub kind: SeriesKind,
    pub width: u32,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct PlotSpec {
    pub title: String,
    pub x_label: String,
    pub y_label: String,
    pub x: Axis,
    pub y: Axis,
    pub x_ticks: usize,
    pub y_ticks: usize,
    pub legend: bool,
    pub series: Vec<Series>,
}

impl PlotSpec {
    /// Resolve `Axis::Auto` against the actual data. Fails loud on an empty
    /// or non-finite plot rather than handing plotters a degenerate range.
    pub fn ranges(&self) -> Result<((f64, f64), (f64, f64)), String> {
        let mut xs = (f64::INFINITY, f64::NEG_INFINITY);
        let mut ys = (f64::INFINITY, f64::NEG_INFINITY);
        for s in &self.series {
            for &(x, y) in &s.points {
                xs.0 = xs.0.min(x);
                xs.1 = xs.1.max(x);
                ys.0 = ys.0.min(y);
                ys.1 = ys.1.max(y);
            }
        }
        let resolve = |axis: Axis, seen: (f64, f64), which: &str| -> Result<(f64, f64), String> {
            let (lo, hi) = match axis {
                Axis::Fixed(lo, hi) => (lo, hi),
                Axis::Auto => {
                    if !seen.0.is_finite() || !seen.1.is_finite() {
                        return Err(format!(
                            "{which} range is `auto` but the plot has no finite points"
                        ));
                    }
                    // Pad a flat series so it does not collapse to a zero-height band.
                    if (seen.1 - seen.0).abs() < f64::EPSILON {
                        (seen.0 - 1.0, seen.1 + 1.0)
                    } else {
                        let pad = (seen.1 - seen.0) * 0.05;
                        (seen.0 - pad, seen.1 + pad)
                    }
                }
            };
            if !(lo.is_finite() && hi.is_finite()) || hi <= lo {
                return Err(format!("{which} range {lo}..{hi} is not a usable interval"));
            }
            Ok((lo, hi))
        };
        Ok((
            resolve(self.x, xs, "x")?,
            resolve(self.y, ys, "y")?,
        ))
    }
}
