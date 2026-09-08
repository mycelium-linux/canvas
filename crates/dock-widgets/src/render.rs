//! Static PNG rendering through external tools — the mermaid/doc-forge
//! pipeline pattern. The spec names the renderer; this module shells out,
//! validates the result, and returns bytes. No rendering logic lives here:
//! ownership of each backend stays with its owning tool (tenet 10).

use crate::{ChartRenderer, Error, Result, WidgetKind, WidgetSpec};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const RENDER_TIMEOUT_SECS: u64 = 60;
const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;

/// Render a widget spec to PNG bytes. Charts use the spec's renderer choice;
/// every other kind falls back to the native plotters path.
pub fn render_png(spec: &WidgetSpec, workspace: &Path) -> Result<Vec<u8>> {
    let out = workspace.join(format!("{}.png", spec.id));
    match &spec.kind {
        WidgetKind::Chart {
            renderer: Some(ChartRenderer::Matplotlib),
            ..
        } => render_with_matplotlib(spec, workspace, &out)?,
        WidgetKind::Chart { .. } => render_with_plotters(spec, workspace, &out)?,
        _ => render_with_plotters(spec, workspace, &out)?,
    }
    let bytes = std::fs::read(&out).map_err(|e| Error(format!("renderer output unread: {e}")))?;
    let _ = std::fs::remove_file(&out);
    if bytes.is_empty() || bytes.len() as u64 > MAX_PNG_BYTES {
        return Err(Error("renderer produced an implausible PNG".into()));
    }
    Ok(bytes)
}

/// Native Rust rendering via the plotters helper binary. Boring default: no
/// Python, no network, deterministic output for the same spec.
fn render_with_plotters(spec: &WidgetSpec, workspace: &Path, out: &Path) -> Result<()> {
    let WidgetKind::Chart {
        chart_type, source, ..
    } = &spec.kind
    else {
        return Err(Error(
            "plotters renderer currently supports chart specs only".into(),
        ));
    };
    let helper = find_helper()?;
    run_helper(
        Command::new(&helper)
            .arg("--chart")
            .arg(chart_type_name(chart_type))
            .arg("--input")
            .arg(absolute(source, workspace))
            .arg("--output")
            .arg(out),
    )
}

/// Shell out to python3 + matplotlib for richer styling. Fails loud when the
/// interpreter or library is missing — no silent fallback (tenet 12).
fn render_with_matplotlib(spec: &WidgetSpec, workspace: &Path, out: &Path) -> Result<()> {
    let WidgetKind::Chart {
        chart_type, source, ..
    } = &spec.kind
    else {
        return Err(Error(
            "matplotlib renderer supports chart specs only".into(),
        ));
    };
    let script = matplotlib_script(chart_type_name(chart_type), source, out, workspace);
    run_helper(Command::new("python3").arg("-c").arg(&script))
}

fn run_helper(command: &mut Command) -> Result<()> {
    let output = command
        .output()
        .map_err(|e| Error(format!("renderer launch failed: {e}")))?;
    if !output.status.success() {
        return Err(Error(format!(
            "renderer failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn chart_type_name(chart: &crate::ChartType) -> &'static str {
    match chart {
        crate::ChartType::Bar => "bar",
        crate::ChartType::Line => "line",
        crate::ChartType::Scatter => "scatter",
    }
}

fn absolute(path: &Path, workspace: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn find_helper() -> Result<PathBuf> {
    let local =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/dock-widget-render");
    if local.exists() {
        return Ok(local);
    }
    which("dock-widget-render")
        .ok_or_else(|| Error("dock-widget-render helper not found in PATH or target/debug".into()))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn matplotlib_script(chart: &str, source: &Path, out: &Path, workspace: &Path) -> String {
    let chart_lit = format!("{:?}", chart);
    let source_lit = format!("{:?}", absolute(source, workspace).to_string_lossy());
    let out_lit = format!("{:?}", out.to_string_lossy());
    format!(
        r##"
import sys
try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
except ImportError as e:
    sys.exit("matplotlib unavailable: %s" % e)

chart, src, dst = {}, {}, {}
if src.endswith(".json"):
    import json
    with open(src) as f:
        rows = json.load(f)
    pts = [(r["x"], r["y"]) for r in rows if isinstance(r, dict) and "x" in r and "y" in r]
else:
    pts = []
    with open(src) as f:
        for line in f:
            parts = line.strip().split("," if not src.endswith(".tsv") else "\t")
            if len(parts) >= 2:
                try:
                    pts.append((float(parts[0]), float(parts[1])))
                except ValueError:
                    pass
if not pts:
    sys.exit("no data points parsed")

xs, ys = zip(*pts)
fig, ax = plt.subplots(figsize=(8, 4.5), dpi=120)
if chart == "bar":
    ax.bar(range(len(xs)), ys, color="#2563eb")
    ax.set_xticks(range(len(xs)), [str(x) for x in xs], rotation=45, ha="right")
elif chart == "line":
    ax.plot(xs, ys, color="#16a34a", linewidth=2, marker="o", markersize=3)
else:
    ax.scatter(xs, ys, s=18, color="#db2777")
ax.grid(True, alpha=0.25)
fig.tight_layout()
fig.savefig(dst)
print("rendered", dst)
"##,
        chart_lit, source_lit, out_lit
    )
}

/// Rendering must respect the helper timeout — a hung renderer must not pin
/// the canvas. Enforced by the caller through process supervision.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(RENDER_TIMEOUT_SECS);
