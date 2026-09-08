//! Dock widget library: reusable presentation primitives with a durable spec model.
//!
//! A [`WidgetSpec`] is presentation intent — the same JSON renders live inside
//! Canvas (iced-native, `live` feature) or statically through external tools
//! (matplotlib, doc-forge, mmdc) exactly like the mermaid `.mmd` pipeline.
//! Ownership of rendering lives in one place per backend; everything else is a
//! client of this crate.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const WIDGET_SCHEMA_VERSION: u32 = 1;
const MAX_SPEC_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(Error(message.into()))
}

/// Durable widget description. Serializes to JSON; both live and static
/// renderers consume the same model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetSpec {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    #[serde(flatten)]
    pub kind: WidgetKind,
    /// Coherence project theme id; falls back to the canvas document theme.
    #[serde(default)]
    pub theme: Option<String>,
    /// Refresh hint for live data in milliseconds. Static renderers ignore it.
    #[serde(default)]
    pub refresh_ms: Option<u64>,
}

fn default_schema_version() -> u32 {
    WIDGET_SCHEMA_VERSION
}

/// The widget primitive set. Each variant carries exactly the params its
/// renderer needs; `deny_unknown_fields` keeps specs honest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WidgetKind {
    /// Local time. `format` uses strftime-style tokens.
    Clock {
        #[serde(default = "default_clock_format")]
        format: String,
    },
    /// Quote ticker. Symbols are uppercase tickers; prices derive from the
    /// live data source when wired, deterministic mock otherwise.
    Ticker { symbols: Vec<String> },
    /// Radial or linear gauge, 0..100.
    Gauge {
        value: f32,
        #[serde(default)]
        label: Option<String>,
    },
    /// Horizontal or vertical LED/VU bar meter, 0..1.
    Meter {
        value: f32,
        #[serde(default = "default_meter_style")]
        style: MeterStyle,
    },
    /// Data chart. `source` points at CSV/TSV/JSON; `renderer` selects the
    /// external pipeline for static output (plotters native by default).
    Chart {
        chart_type: ChartType,
        source: PathBuf,
        #[serde(default)]
        renderer: Option<ChartRenderer>,
    },
    /// Presence indicator dot.
    Status {
        #[serde(default)]
        state: Option<StatusState>,
    },
    /// Free text badge.
    Text { text: String },
}

fn default_clock_format() -> String {
    "%H:%M:%S".into()
}

fn default_meter_style() -> MeterStyle {
    MeterStyle::Led
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterStyle {
    Led,
    Vu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartType {
    Bar,
    Line,
    Scatter,
}

/// Static chart backend. `Plotters` is native Rust (no external deps);
/// `Matplotlib` shells out to python for richer styling; `Docforge` renders
/// arbitrary HTML/CSS widget templates through the doc-forge service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartRenderer {
    Plotters,
    Matplotlib,
    Docforge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Present,
    Away,
    Unknown,
}

impl WidgetSpec {
    pub fn from_json(json: &[u8]) -> Result<Self> {
        if json.is_empty() {
            return err("widget spec must not be empty");
        }
        if json.len() as u64 > MAX_SPEC_BYTES {
            return err(format!("widget spec must contain 1–{MAX_SPEC_BYTES} bytes"));
        }
        let spec: WidgetSpec =
            serde_json::from_slice(json).map_err(|e| Error(format!("invalid widget spec: {e}")))?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| Error(format!("cannot read spec: {e}")))?;
        Self::from_json(&bytes)
    }

    pub fn validate(&self) -> Result<()> {
        check(
            !self.id.is_empty()
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "widget id must be 1+ alphanumeric/hyphen/underscore characters",
        )?;
        if self.schema_version != WIDGET_SCHEMA_VERSION {
            return err(format!(
                "unsupported widget schema version {} (expected {WIDGET_SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        match &self.kind {
            WidgetKind::Ticker { symbols } => {
                check(!symbols.is_empty(), "ticker requires at least one symbol")?;
                check(
                    symbols.iter().all(|s| !s.is_empty() && s.len() <= 12),
                    "ticker symbols must be 1-12 characters",
                )?;
            }
            WidgetKind::Gauge { value, .. } => check(
                value.is_finite() && (0.0..=100.0).contains(value),
                "gauge value must be finite within 0..100",
            )?,
            WidgetKind::Meter { value, .. } => check(
                value.is_finite() && (0.0..=1.0).contains(value),
                "meter value must be finite within 0..1",
            )?,
            WidgetKind::Chart { source, .. } => check(
                !source.as_os_str().is_empty(),
                "chart requires a data source path",
            )?,
            WidgetKind::Text { text } => {
                check(!text.is_empty(), "text widget must not be empty")?;
                check(text.len() <= 4096, "text widget exceeds 4096 bytes")?;
            }
            WidgetKind::Clock { format } => check(
                !format.is_empty() && format.len() <= 64,
                "clock format must be 1-64 characters",
            )?,
            WidgetKind::Status { .. } => {}
        }
        Ok(())
    }
}

fn check(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { err(message) }
}

#[cfg(feature = "live")]
pub mod live;

/// Static PNG rendering through external tools (matplotlib / plotters helper).
pub mod render;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_roundtrips_through_json() {
        let json = r#"{
            "schema_version": 1,
            "id": "demo-ticker",
            "kind": "ticker",
            "symbols": ["AAPL", "GOOG"],
            "refresh_ms": 30000
        }"#;
        let spec = WidgetSpec::from_json(json.as_bytes()).expect("valid spec");
        assert_eq!(spec.id, "demo-ticker");
        assert_eq!(
            spec.kind,
            WidgetKind::Ticker {
                symbols: vec!["AAPL".into(), "GOOG".into()]
            }
        );
        let re = serde_json::to_string(&spec).expect("serialize");
        let reparsed = WidgetSpec::from_json(re.as_bytes()).expect("roundtrip");
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn gauge_rejects_out_of_range() {
        let json = r#"{"id":"bad","kind":"gauge","value":150}"#;
        assert!(WidgetSpec::from_json(json.as_bytes()).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let json = r#"{"id":"x","kind":"clock","format":"%H","bogus":1}"#;
        assert!(WidgetSpec::from_json(json.as_bytes()).is_err());
    }

    #[test]
    fn chart_spec_carries_renderer_choice() {
        let json = r#"{
            "id": "chart-1",
            "kind": "chart",
            "chart_type": "bar",
            "source": "/tmp/data.csv",
            "renderer": "matplotlib"
        }"#;
        let spec = WidgetSpec::from_json(json.as_bytes()).expect("valid chart spec");
        match spec.kind {
            WidgetKind::Chart {
                chart_type,
                renderer,
                ..
            } => {
                assert_eq!(chart_type, ChartType::Bar);
                assert_eq!(renderer, Some(ChartRenderer::Matplotlib));
            }
            other => panic!("expected chart, got {other:?}"),
        }
    }
}
