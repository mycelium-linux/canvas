//! The Lua-facing plot contract.
//!
//! A script is a Canvas binder in shape: a bounded chunk that receives globals
//! and *returns a table*. It never calls into the renderer, never holds a
//! handle, and cannot name a colour -- only a palette *role*, which the host
//! resolves. That is what keeps an externally supplied theme authoritative.

use crate::palette::{Palette, ROLES};
use mlua::{Lua, Table, VmState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Matches the 32 KiB adapter-source bound Canvas already applies to binders.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
/// Matches `MAX_SERIES_POINTS` in crates/canvas/src/lua_module.rs:25.
pub const MAX_SERIES_POINTS: usize = 4_096;
/// A legend stops being readable well before this.
pub const MAX_SERIES: usize = 8;
pub const MAX_LABEL_CHARS: usize = 64;
/// Hard ceiling on the Luau VM's heap for one evaluation.
pub const MAX_LUA_HEAP_BYTES: usize = 4 * 1024 * 1024;
/// Interrupt ticks before a script is killed. A heap cap alone does NOT stop a
/// non-allocating spin loop (`while true do end`), which would wedge the frame
/// thread forever -- so the budget is a separate, independent bound.
pub const MAX_LUA_INTERRUPTS: u64 = 200_000;

#[derive(Debug, Clone)]
pub struct SeriesSpec {
    pub name: String,
    /// A palette role name, already proven to exist in `Palette`.
    pub role: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct PlotSpec {
    pub title: String,
    pub x_label: String,
    pub y_label: String,
    pub x_range: (f64, f64),
    pub y_range: (f64, f64),
    pub series: Vec<SeriesSpec>,
}

impl PlotSpec {
    pub fn point_count(&self) -> usize {
        self.series.iter().map(|s| s.points.len()).sum()
    }
}

fn label(table: &Table, key: &str) -> Result<String, String> {
    let raw: String = table
        .get(key)
        .map_err(|e| format!("plot.{key} must be a string: {e}"))?;
    if raw.chars().count() > MAX_LABEL_CHARS {
        return Err(format!(
            "plot.{key} exceeds {MAX_LABEL_CHARS} characters ({} given)",
            raw.chars().count()
        ));
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(format!("plot.{key} contains control characters"));
    }
    Ok(raw)
}

fn range(table: &Table, key: &str) -> Result<(f64, f64), String> {
    let pair: Table = table
        .get(key)
        .map_err(|e| format!("plot.{key} must be a {{min, max}} table: {e}"))?;
    let lo: f64 = pair.get(1).map_err(|e| format!("plot.{key}[1]: {e}"))?;
    let hi: f64 = pair.get(2).map_err(|e| format!("plot.{key}[2]: {e}"))?;
    if !lo.is_finite() || !hi.is_finite() {
        return Err(format!("plot.{key} bounds must be finite"));
    }
    if hi <= lo {
        return Err(format!("plot.{key} must satisfy max > min (got {lo}..{hi})"));
    }
    Ok((lo, hi))
}

/// Evaluate a plot script under the Luau sandbox and validate everything it
/// produced. Every failure is an `Err`; nothing is silently defaulted.
pub fn load(source: &str, palette: &Palette, tick: f64) -> Result<PlotSpec, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "plot script is {} bytes, over the {MAX_SOURCE_BYTES} byte bound",
            source.len()
        ));
    }

    let lua = Lua::new();
    lua.sandbox(true)
        .map_err(|e| format!("could not enable the Luau sandbox: {e}"))?;
    lua.set_memory_limit(MAX_LUA_HEAP_BYTES)
        .map_err(|e| format!("could not bound the Luau heap: {e}"))?;

    let budget = Arc::new(AtomicU64::new(MAX_LUA_INTERRUPTS));
    {
        let budget = Arc::clone(&budget);
        lua.set_interrupt(move |_| {
            if budget.fetch_sub(1, Ordering::Relaxed) == 0 {
                return Err(mlua::Error::runtime("plot script exceeded its step budget"));
            }
            Ok(VmState::Continue)
        });
    }

    let globals = lua.globals();
    globals
        .set("tick", tick)
        .map_err(|e| format!("could not publish `tick`: {e}"))?;
    // The script is told which roles exist; it is never handed RGB values, so a
    // theme swap cannot be defeated by a script that cached a colour.
    globals
        .set("roles", ROLES.to_vec())
        .map_err(|e| format!("could not publish `roles`: {e}"))?;

    let table: Table = lua
        .load(source)
        .set_name("plot")
        .eval()
        .map_err(|e| format!("plot script failed: {e}"))?;

    let title = label(&table, "title")?;
    let x_label = label(&table, "x_label")?;
    let y_label = label(&table, "y_label")?;
    let x_range = range(&table, "x_range")?;
    let y_range = range(&table, "y_range")?;

    let series_table: Table = table
        .get("series")
        .map_err(|e| format!("plot.series must be a table: {e}"))?;
    let declared = series_table.raw_len();
    if declared == 0 {
        return Err("plot.series is empty".into());
    }
    if declared > MAX_SERIES {
        return Err(format!(
            "plot.series declares {declared} series, over the {MAX_SERIES} bound"
        ));
    }

    let mut series = Vec::with_capacity(declared);
    for index in 1..=declared {
        let entry: Table = series_table
            .get(index)
            .map_err(|e| format!("plot.series[{index}]: {e}"))?;
        let name = label(&entry, "name")
            .map_err(|e| format!("plot.series[{index}].{}", e.trim_start_matches("plot.")))?;
        let role: String = entry
            .get("role")
            .map_err(|e| format!("plot.series[{index}].role must be a string: {e}"))?;
        if palette.role(&role).is_none() {
            return Err(format!(
                "plot.series[{index}].role `{role}` is not a palette role; valid roles: {}",
                ROLES.join(", ")
            ));
        }

        let points_table: Table = entry
            .get("points")
            .map_err(|e| format!("plot.series[{index}].points must be a table: {e}"))?;
        let count = points_table.raw_len();
        if count > MAX_SERIES_POINTS {
            return Err(format!(
                "plot.series[{index}] carries {count} points, over the {MAX_SERIES_POINTS} bound"
            ));
        }
        let mut points = Vec::with_capacity(count);
        for p in 1..=count {
            let pair: Table = points_table
                .get(p)
                .map_err(|e| format!("plot.series[{index}].points[{p}]: {e}"))?;
            let x: f64 = pair
                .get(1)
                .map_err(|e| format!("plot.series[{index}].points[{p}][1]: {e}"))?;
            let y: f64 = pair
                .get(2)
                .map_err(|e| format!("plot.series[{index}].points[{p}][2]: {e}"))?;
            if !x.is_finite() || !y.is_finite() {
                return Err(format!(
                    "plot.series[{index}].points[{p}] is not finite ({x}, {y})"
                ));
            }
            points.push((x, y));
        }

        series.push(SeriesSpec {
            name,
            role,
            points,
        });
    }

    Ok(PlotSpec {
        title,
        x_label,
        y_label,
        x_range,
        y_range,
        series,
    })
}
