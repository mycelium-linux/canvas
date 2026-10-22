//! Bounded Luau -> PlotSpec loader.
//!
//! Shape follows Canvas's existing binders: a sandboxed Luau state, a byte cap
//! on the source, a memory cap, an interrupt budget so a runaway script cannot
//! wedge the frame, and hard caps on everything the script can produce. The
//! script returns plain data; it never gets a handle to the renderer.

use mlua::{Lua, Table, Value};

use crate::palette::Role;
use crate::spec::{
    Axis, MAX_LABEL_BYTES, MAX_SERIES, MAX_SERIES_POINTS, MAX_SOURCE_BYTES, MAX_TICKS, PlotSpec,
    Series, SeriesKind,
};

/// Luau VM instruction budget per evaluation. A binder that defines a plot is
/// doing arithmetic over a few thousand points, not solving anything.
const INTERRUPT_BUDGET: u64 = 20_000_000;
const MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

pub fn load_plot(source: &str) -> Result<PlotSpec, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "plot script is {} bytes, over the {MAX_SOURCE_BYTES} byte budget",
            source.len()
        ));
    }

    let lua = Lua::new();
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)
        .map_err(|e| format!("could not set Lua memory limit: {e}"))?;
    lua.sandbox(true)
        .map_err(|e| format!("could not sandbox Luau: {e}"))?;

    let steps = std::cell::Cell::new(0u64);
    lua.set_interrupt(move |_| {
        steps.set(steps.get() + 1);
        if steps.get() > INTERRUPT_BUDGET {
            Err(mlua::Error::runtime("plot script exceeded its step budget"))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });

    let table: Table = lua
        .load(source)
        .set_name("plot")
        .eval()
        .map_err(|e| format!("plot script failed: {e}"))?;

    spec_from_table(&table)
}

fn label(t: &Table, key: &str, default: &str) -> Result<String, String> {
    let v: Option<String> = t
        .get(key)
        .map_err(|e| format!("`{key}` must be a string: {e}"))?;
    let s = v.unwrap_or_else(|| default.to_string());
    if s.len() > MAX_LABEL_BYTES {
        return Err(format!(
            "`{key}` is {} bytes, over the {MAX_LABEL_BYTES} byte cap",
            s.len()
        ));
    }
    if s.chars().any(|c| c.is_control()) {
        return Err(format!("`{key}` contains control characters"));
    }
    Ok(s)
}

fn axis(t: &Table, key: &str) -> Result<Axis, String> {
    match t.get::<Value>(key).map_err(|e| e.to_string())? {
        Value::Nil => Ok(Axis::Auto),
        Value::String(s) => {
            let s = s.to_str().map_err(|e| e.to_string())?.to_string();
            if s == "auto" {
                Ok(Axis::Auto)
            } else {
                Err(format!("`{key}` must be \"auto\" or {{lo, hi}}, got \"{s}\""))
            }
        }
        Value::Table(pair) => {
            let lo: f64 = pair
                .get(1)
                .map_err(|_| format!("`{key}[1]` must be a number"))?;
            let hi: f64 = pair
                .get(2)
                .map_err(|_| format!("`{key}[2]` must be a number"))?;
            Ok(Axis::Fixed(lo, hi))
        }
        other => Err(format!(
            "`{key}` must be \"auto\" or {{lo, hi}}, got {}",
            other.type_name()
        )),
    }
}

fn ticks(t: &Table, key: &str, default: usize) -> Result<usize, String> {
    let v: Option<usize> = t
        .get(key)
        .map_err(|e| format!("`{key}` must be a positive integer: {e}"))?;
    let n = v.unwrap_or(default);
    if n == 0 || n > MAX_TICKS {
        return Err(format!("`{key}` must be 1..={MAX_TICKS}, got {n}"));
    }
    Ok(n)
}

fn spec_from_table(t: &Table) -> Result<PlotSpec, String> {
    let series_tbl: Table = t
        .get("series")
        .map_err(|_| "plot must define a `series` array".to_string())?;

    let mut series = Vec::new();
    for (i, item) in series_tbl.sequence_values::<Table>().enumerate() {
        if series.len() >= MAX_SERIES {
            return Err(format!("plot defines more than {MAX_SERIES} series"));
        }
        let item = item.map_err(|e| format!("series[{}] must be a table: {e}", i + 1))?;
        series.push(series_from_table(&item, i + 1)?);
    }
    if series.is_empty() {
        return Err("plot must define at least one series".to_string());
    }

    Ok(PlotSpec {
        title: label(t, "title", "")?,
        x_label: label(t, "x_label", "")?,
        y_label: label(t, "y_label", "")?,
        x: axis(t, "x_range")?,
        y: axis(t, "y_range")?,
        x_ticks: ticks(t, "x_ticks", 6)?,
        y_ticks: ticks(t, "y_ticks", 5)?,
        legend: t.get::<Option<bool>>("legend").unwrap_or(None).unwrap_or(true),
        series,
    })
}

fn series_from_table(t: &Table, n: usize) -> Result<Series, String> {
    let name = label(t, "name", &format!("series {n}"))?;

    // The theming contract: a colour is a Coherence role name, and nothing
    // else. There is no hex escape hatch on purpose.
    let color_name: String = t
        .get("color")
        .map_err(|_| format!("series[{n}] `color` must be a palette role name"))?;
    let color = Role::parse(&color_name).ok_or_else(|| {
        format!(
            "series[{n}] colour \"{color_name}\" is not a palette role; expected one of {}",
            Role::NAMES.join(", ")
        )
    })?;

    let kind_name: String = t
        .get::<Option<String>>("kind")
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "line".into());
    let kind = SeriesKind::parse(&kind_name)
        .ok_or_else(|| format!("series[{n}] kind \"{kind_name}\" is not line|area|points"))?;

    let width: u32 = t.get::<Option<u32>>("width").unwrap_or(None).unwrap_or(2);
    if width == 0 || width > 12 {
        return Err(format!("series[{n}] width must be 1..=12, got {width}"));
    }

    let points_tbl: Table = t
        .get("points")
        .map_err(|_| format!("series[{n}] must define a `points` array"))?;
    let mut points = Vec::new();
    for (i, p) in points_tbl.sequence_values::<Table>().enumerate() {
        if points.len() >= MAX_SERIES_POINTS {
            return Err(format!(
                "series[{n}] has more than {MAX_SERIES_POINTS} points"
            ));
        }
        let p = p.map_err(|e| format!("series[{n}].points[{}] must be {{x, y}}: {e}", i + 1))?;
        let x: f64 = p
            .get(1)
            .map_err(|_| format!("series[{n}].points[{}][1] must be a number", i + 1))?;
        let y: f64 = p
            .get(2)
            .map_err(|_| format!("series[{n}].points[{}][2] must be a number", i + 1))?;
        if !x.is_finite() || !y.is_finite() {
            return Err(format!(
                "series[{n}].points[{}] is not finite ({x}, {y})",
                i + 1
            ));
        }
        points.push((x, y));
    }
    if points.is_empty() {
        return Err(format!("series[{n}] has no points"));
    }

    Ok(Series {
        name,
        color,
        kind,
        width,
        points,
    })
}
