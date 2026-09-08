//! Dock widget library: reusable presentation primitives with a durable spec model.
//!
//! A [`WidgetSpec`] is presentation intent — the same JSON renders live inside
//! Canvas (iced-native, `live` feature) or statically through external tools
//! (matplotlib, doc-forge, mmdc) exactly like the mermaid `.mmd` pipeline.
//! Ownership of rendering lives in one place per backend; everything else is a
//! client of this crate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// A kind this crate does not define, supplied by a Canvas Lua module.
    ///
    /// The point of the variant: the vocabulary can grow without a Rust change,
    /// a rebuild, or a deploy. `module` names a manifest the host resolves; it is
    /// deliberately an id rather than a path, so a spec cannot reach into the
    /// filesystem to choose what code runs.
    ///
    /// `inputs` are *declared* values. A module kind receives its data and cannot
    /// go looking for any — which is the opposite of what [`WidgetKind::Clock`] and
    /// [`WidgetKind::Ticker`] do today, reading the host clock and inventing prices
    /// from a hash. Those two are the subject of dock-cov.5 and are not the model
    /// to follow here: if every externally-defined kind could reach for a clock or
    /// a number, that boundary becomes unenforceable exactly as the vocabulary
    /// starts growing.
    Module {
        module: String,
        #[serde(default)]
        inputs: BTreeMap<String, WidgetInput>,
    },
}

/// A declared input value carried by a [`WidgetKind::Module`] spec.
///
/// The same five shapes `dock_canvas::lua_module::ModuleInputValue` accepts. They
/// live here because this is the lower crate — `dock-canvas` depends on
/// `dock-widgets`, not the reverse — and a spec has to be able to express a typed
/// input without pulling in the renderer. The two definitions must agree; folding
/// the canvas one onto this is follow-up, not this change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetInput {
    Number { value: f64 },
    Text { value: String },
    Boolean { value: bool },
    Series { points: Vec<SeriesPoint> },
    Status { level: StatusLevel, label: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SeriesPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum StatusLevel {
    Neutral,
    Good,
    Warning,
    Critical,
}

/// Ceilings for a module spec. Bounded because an agent writes these, and an
/// unbounded series is a denial of service against the renderer rather than a
/// rendering bug.
const MAX_MODULE_INPUTS: usize = 32;
const MAX_SERIES_POINTS: usize = 4096;
const MAX_INPUT_TEXT_BYTES: usize = 4096;

fn default_clock_format() -> String {
    "%H:%M:%S".into()
}

fn default_meter_style() -> MeterStyle {
    MeterStyle::Led
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MeterStyle {
    Led,
    Vu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ChartRenderer {
    Plotters,
    Matplotlib,
    Docforge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
            WidgetKind::Module { module, inputs } => {
                check(
                    !module.is_empty()
                        && module.len() <= 64
                        && module
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                    "module id must be 1-64 alphanumeric/hyphen/underscore characters",
                )?;
                check(
                    inputs.len() <= MAX_MODULE_INPUTS,
                    "module spec declares too many inputs",
                )?;
                for (name, input) in inputs {
                    check(
                        !name.is_empty()
                            && name.len() <= 64
                            && name
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                        "module input names must be 1-64 alphanumeric/hyphen/underscore characters",
                    )?;
                    input.validate()?;
                }
                // Whether this module exists, and whether these inputs match the
                // ones it declares, is the host's to answer: this crate does not
                // own module manifests. Everything checkable without one is
                // checked here so a malformed spec never reaches a renderer.
            }
        }
        Ok(())
    }
}

impl WidgetInput {
    pub fn validate(&self) -> Result<()> {
        match self {
            WidgetInput::Number { value } => {
                check(value.is_finite(), "module number input must be finite")
            }
            WidgetInput::Text { value } => check(
                value.len() <= MAX_INPUT_TEXT_BYTES,
                "module text input exceeds 4096 bytes",
            ),
            WidgetInput::Boolean { .. } => Ok(()),
            WidgetInput::Series { points } => {
                check(
                    points.len() <= MAX_SERIES_POINTS,
                    "module series input exceeds 4096 points",
                )?;
                check(
                    points.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
                    "module series points must be finite",
                )
            }
            WidgetInput::Status { label, .. } => check(
                label.len() <= MAX_INPUT_TEXT_BYTES,
                "module status label exceeds 4096 bytes",
            ),
        }
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

#[cfg(test)]
mod module_kind_tests {
    use super::*;

    /// Wrap a bare `kind` object in the common spec fields.
    fn spec(kind_json: &str) -> Result<WidgetSpec> {
        let inner = kind_json.trim();
        let inner = inner
            .strip_prefix('{')
            .expect("kind fixtures are JSON objects");
        WidgetSpec::from_json(format!(r#"{{"schema_version":1,"id":"w",{inner}"#).as_bytes())
    }

    #[test]
    fn a_kind_this_crate_never_heard_of_round_trips_through_a_module_spec() {
        let spec = spec(
            r#"{"kind":"module","module":"sparkline-bands","inputs":{
                 "samples":{"type":"series","points":[{"x":0.0,"y":1.5}]},
                 "label":{"type":"text","value":"revenue"}}}"#,
        )
        .expect("module spec");
        let WidgetKind::Module { module, inputs } = &spec.kind else {
            panic!("expected a module kind");
        };
        assert_eq!(module, "sparkline-bands");
        assert_eq!(inputs.len(), 2);
        // Durable: what goes in comes back out, because instances are the corpus a
        // later consolidation has to rewrite.
        let round_tripped =
            WidgetSpec::from_json(&serde_json::to_vec(&spec).expect("serialize")).expect("reparse");
        assert_eq!(round_tripped, spec);
    }

    #[test]
    fn the_seven_built_in_kinds_still_parse_unchanged() {
        for json in [
            r#"{"kind":"clock","format":"%H:%M"}"#,
            r#"{"kind":"ticker","symbols":["AAPL"]}"#,
            r#"{"kind":"gauge","value":50.0}"#,
            r#"{"kind":"meter","value":0.5}"#,
            r#"{"kind":"chart","chart_type":"bar","source":"data.csv"}"#,
            r#"{"kind":"status"}"#,
            r#"{"kind":"text","text":"hi"}"#,
        ] {
            spec(json).unwrap_or_else(|e| panic!("{json} regressed: {e}"));
        }
    }

    #[test]
    fn a_kind_that_is_neither_built_in_nor_a_module_still_fails_loudly() {
        // Opening the enum must not turn an unknown kind into a silent no-op.
        let error =
            spec(r#"{"kind":"holographic_pony","glitter":true}"#).expect_err("unknown kind");
        assert!(error.0.contains("invalid widget spec"), "{}", error.0);
    }

    #[test]
    fn a_module_id_cannot_smuggle_a_path() {
        for id in ["../../etc/passwd", "/abs/path", "a/b", "with space", ""] {
            let json = format!(r#"{{"kind":"module","module":"{id}"}}"#);
            assert!(spec(&json).is_err(), "accepted module id {id:?}");
        }
    }

    #[test]
    fn module_inputs_are_bounded_because_an_agent_writes_them() {
        let points: Vec<String> = (0..MAX_SERIES_POINTS + 1)
            .map(|i| format!(r#"{{"x":{i}.0,"y":1.0}}"#))
            .collect();
        let json = format!(
            r#"{{"kind":"module","module":"m","inputs":{{"s":{{"type":"series","points":[{}]}}}}}}"#,
            points.join(",")
        );
        assert!(spec(&json).is_err(), "an unbounded series must be refused");

        let nan = r#"{"kind":"module","module":"m","inputs":{"n":{"type":"number","value":null}}}"#;
        assert!(spec(nan).is_err(), "a non-finite number must be refused");
    }

    #[test]
    fn a_module_input_of_an_unknown_type_is_refused_rather_than_coerced() {
        let json = r#"{"kind":"module","module":"m","inputs":{"x":{"type":"geojson","value":{}}}}"#;
        assert!(spec(json).is_err(), "the input vocabulary must stay closed");
    }
}
