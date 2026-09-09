//! Canvas widget library: reusable presentation primitives with a durable spec model.
//!
//! A [`WidgetSpec`] is presentation intent — the same JSON renders live inside
//! Canvas (iced-native, `live` feature) or statically through external tools
//! (matplotlib, doc-forge, mmdc) exactly like the mermaid `.mmd` pipeline.
//! Ownership of rendering lives in one place per backend; everything else is a
//! client of this crate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod quant;

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
    /// Window treatment when this widget is opened as a movable Canvas surface.
    #[serde(default)]
    pub chrome: WidgetChrome,
    /// Source health and provenance supplied by the connector that owns the data.
    /// Renderers consume this uniformly; individual widget kinds do not invent
    /// their own freshness badges or empty-state conventions.
    #[serde(default)]
    pub data: WidgetData,
}
/// Where a widget's values came from.
///
/// This sits alongside [`WidgetDataState`] rather than inside it because the two
/// are independent: a demo widget can be perfectly `Ready`, and must still say
/// so on its face. Demo values are ordinary values in the real shape, supplied
/// from outside like any other payload — a widget never contains the ability to
/// invent one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Provenance {
    /// Real values from the widget's declared source.
    #[default]
    Live,
    /// Stand-in values for designing or demonstrating a surface.
    Demo,
    /// Representative values shipped as documentation of the payload shape.
    Sample,
}

impl Provenance {
    /// The badge a viewer sees, or `None` when the values are real.
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Live => None,
            Self::Demo => Some("DEMO"),
            Self::Sample => Some("SAMPLE"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WidgetData {
    #[serde(default)]
    pub state: WidgetDataState,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// Absent in specs written before provenance existed, hence `default`.
    /// Defaulting to `Live` is safe only because nothing may fabricate: a widget
    /// that cannot invent a value cannot silently claim a real one.
    #[serde(default)]
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetDataState {
    #[default]
    Ready,
    Loading,
    NoData,
    Stale,
    Partial,
    Disconnected,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetChrome {
    Standard,
    #[default]
    DragOnly,
}

fn default_schema_version() -> u32 {
    WIDGET_SCHEMA_VERSION
}

/// Widget `kind` tags this build can render.
///
/// A caller needs to tell "a widget from a newer Canvas" apart from "a typo in a
/// widget this build owns": the first should degrade to a placeholder, the
/// second must fail loudly. Serde's derived enum knows the difference internally
/// but only reports it as an error string, so the list is named here.
///
/// Adding a variant to [`WidgetKind`] means adding its tag here;
/// `every_known_widget_kind_is_listed` fails if they drift apart.
pub const KNOWN_WIDGET_KINDS: [&str; 20] = [
    "quant",
    "stat",
    "time_series",
    "clock",
    "ticker",
    "gauge",
    "meter",
    "histogram",
    "heatmap",
    "ranked_bar",
    "stacked_bar",
    "state_timeline",
    "slo_burn_rate",
    "table",
    "event_log",
    "node_summary",
    "chart",
    "status",
    "text",
    "module",
];

/// Whether this build can render `tag`, or it belongs to a newer Canvas.
pub fn is_known_widget_kind(tag: &str) -> bool {
    KNOWN_WIDGET_KINDS.contains(&tag)
}

/// The widget primitive set. Each variant carries exactly the params its
/// renderer needs; `deny_unknown_fields` keeps specs honest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetKind {
    /// Domain-owned quantitative visualizations sharing the common widget shell.
    Quant { widget: quant::QuantWidget },
    /// Prometheus-style single value with optional trend and retained history.
    Stat {
        label: String,
        value: f64,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        change: Option<f64>,
        #[serde(default)]
        state: Option<StatusLevel>,
        #[serde(default)]
        history: Vec<f32>,
    },
    /// One or more bounded time series. Connectors replace the retained points.
    TimeSeries {
        title: String,
        series: Vec<MetricSeries>,
        #[serde(default)]
        unit: Option<String>,
    },
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
    /// Prometheus histogram buckets rendered as an operational distribution.
    Histogram {
        title: String,
        buckets: Vec<HistogramBucket>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Dense row/column intensity matrix for latency, load, and correlation.
    Heatmap {
        title: String,
        rows: Vec<String>,
        columns: Vec<String>,
        values: Vec<Vec<f64>>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Ranked categorical values, highest first at render time.
    RankedBar {
        title: String,
        bars: Vec<BarValue>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Part-to-whole values for a small set of categories and series.
    StackedBar {
        title: String,
        series: Vec<String>,
        bars: Vec<StackedBarValue>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Bounded state intervals grouped into named lanes.
    StateTimeline {
        title: String,
        lanes: Vec<StateLane>,
    },
    /// Error-budget consumption across operational windows.
    SloBurnRate {
        title: String,
        objective: f64,
        windows: Vec<BurnRateWindow>,
    },
    /// Compact read-only table for labels, targets, and query results.
    Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Recent alert/event/log lines with explicit severity.
    EventLog {
        title: String,
        entries: Vec<EventEntry>,
    },
    /// Compact health summary for nodes, agents, or DAG stages.
    NodeSummary {
        title: String,
        nodes: Vec<NodeSummaryItem>,
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

/// Small, typed changes accepted from host-owned stream bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetMutation {
    Quant {
        mutation: quant::QuantMutation,
    },
    /// Replace only a widget's bounded, typed data payload. Layout, identity,
    /// theme and chrome remain owned by the saved Canvas view.
    Snapshot {
        widget: WidgetKind,
        #[serde(default)]
        data: WidgetData,
    },
}

impl WidgetSpec {
    pub fn apply_mutation(&mut self, mutation: WidgetMutation) -> Result<()> {
        match (&mut self.kind, mutation) {
            (WidgetKind::Quant { widget }, WidgetMutation::Quant { mutation }) => {
                widget.apply(mutation)?;
            }
            (current, WidgetMutation::Snapshot { widget, data })
                if same_widget_kind(current, &widget) =>
            {
                *current = widget;
                self.data = data;
            }
            _ => return err("widget mutation does not match the target widget kind"),
        }
        self.validate()
    }
}

#[cfg(test)]
mod mutation_tests {
    use super::*;

    fn stat() -> WidgetSpec {
        WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "health".into(),
            kind: WidgetKind::Stat {
                label: "Health".into(),
                value: 0.0,
                unit: None,
                change: None,
                state: None,
                history: Vec::new(),
            },
            theme: Some("project".into()),
            refresh_ms: Some(1_000),
            chrome: WidgetChrome::DragOnly,
            data: WidgetData::default(),
        }
    }

    #[test]
    fn snapshot_replaces_only_matching_widget_data() {
        let mut spec = stat();
        spec.apply_mutation(WidgetMutation::Snapshot {
            widget: WidgetKind::Stat {
                label: "Health".into(),
                value: 1.0,
                unit: None,
                change: None,
                state: Some(StatusLevel::Good),
                history: vec![0.0, 1.0],
            },
            data: WidgetData {
                state: WidgetDataState::Ready,
                ..Default::default()
            },
        })
        .unwrap();
        assert_eq!(spec.id, "health");
        assert_eq!(spec.theme.as_deref(), Some("project"));
        assert_eq!(spec.data.state, WidgetDataState::Ready);
    }

    #[test]
    fn snapshot_cannot_change_widget_family() {
        let mut spec = stat();
        let before = spec.clone();
        assert!(
            spec.apply_mutation(WidgetMutation::Snapshot {
                widget: WidgetKind::Gauge {
                    value: 50.0,
                    label: None,
                },
                data: WidgetData::default(),
            })
            .is_err()
        );
        assert_eq!(spec, before);
    }
}

fn same_widget_kind(left: &WidgetKind, right: &WidgetKind) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

/// A declared input value carried by a [`WidgetKind::Module`] spec.
///
/// The same five shapes `canvas::lua_module::ModuleInputValue` accepts. They
/// live here because this is the lower crate — `canvas` depends on
/// `canvas-widgets`, not the reverse — and a spec has to be able to express a typed
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MetricSeries {
    pub label: String,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistogramBucket {
    pub upper_bound: f64,
    pub count: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BarValue {
    pub label: String,
    pub value: f64,
    #[serde(default)]
    pub state: Option<StatusLevel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StackedBarValue {
    pub label: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StateLane {
    pub label: String,
    pub segments: Vec<StateSegment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StateSegment {
    pub start: f64,
    pub end: f64,
    pub state: StatusLevel,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BurnRateWindow {
    pub label: String,
    pub burn_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventEntry {
    pub timestamp: String,
    pub message: String,
    pub level: StatusLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NodeSummaryItem {
    pub label: String,
    pub state: StatusLevel,
    #[serde(default)]
    pub detail: Option<String>,
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
        optional_bounded_text(&self.data.source, "widget data source", 256)?;
        optional_bounded_text(&self.data.observed_at, "widget observed time", 128)?;
        optional_bounded_text(&self.data.window, "widget data window", 128)?;
        optional_bounded_text(
            &self.data.detail,
            "widget data detail",
            MAX_INPUT_TEXT_BYTES,
        )?;
        match &self.kind {
            WidgetKind::Quant { widget } => widget.validate()?,
            WidgetKind::Stat {
                label,
                value,
                unit,
                change,
                history,
                ..
            } => {
                bounded_label(label, "stat label")?;
                optional_label(unit, "stat unit")?;
                check(value.is_finite(), "stat value must be finite")?;
                check(
                    change.is_none_or(f64::is_finite),
                    "stat change must be finite",
                )?;
                validate_f32_series(history, "stat history")?;
            }
            WidgetKind::TimeSeries {
                title,
                series,
                unit,
            } => {
                bounded_label(title, "time series title")?;
                optional_label(unit, "time series unit")?;
                check(
                    !series.is_empty() && series.len() <= 12,
                    "time series requires 1-12 series",
                )?;
                for item in series {
                    bounded_label(&item.label, "series label")?;
                    check(
                        !item.points.is_empty() && item.points.len() <= MAX_SERIES_POINTS,
                        "series requires 1-4096 points",
                    )?;
                    check(
                        item.points
                            .iter()
                            .all(|point| point.x.is_finite() && point.y.is_finite()),
                        "series points must be finite",
                    )?;
                }
            }
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
            WidgetKind::Histogram {
                title,
                buckets,
                unit,
            } => {
                bounded_label(title, "histogram title")?;
                optional_label(unit, "histogram unit")?;
                check(
                    !buckets.is_empty() && buckets.len() <= 128,
                    "histogram requires 1-128 buckets",
                )?;
                check(
                    buckets.iter().all(|bucket| {
                        bucket.upper_bound.is_finite()
                            && bucket.count.is_finite()
                            && bucket.count >= 0.0
                    }),
                    "histogram buckets must be finite and non-negative",
                )?;
            }
            WidgetKind::Heatmap {
                title,
                rows,
                columns,
                values,
                unit,
            } => {
                bounded_label(title, "heatmap title")?;
                optional_label(unit, "heatmap unit")?;
                validate_labels(rows, "heatmap row", 32)?;
                validate_labels(columns, "heatmap column", 64)?;
                check(values.len() == rows.len(), "heatmap values must match rows")?;
                check(
                    values.iter().all(|row| {
                        row.len() == columns.len() && row.iter().all(|value| value.is_finite())
                    }),
                    "heatmap rows must match columns and contain finite values",
                )?;
            }
            WidgetKind::RankedBar { title, bars, unit } => {
                bounded_label(title, "ranked bar title")?;
                optional_label(unit, "ranked bar unit")?;
                check(
                    !bars.is_empty() && bars.len() <= 64,
                    "ranked bar requires 1-64 bars",
                )?;
                for bar in bars {
                    bounded_label(&bar.label, "ranked bar label")?;
                    check(
                        bar.value.is_finite() && bar.value >= 0.0,
                        "ranked bar values must be finite and non-negative",
                    )?;
                }
            }
            WidgetKind::StackedBar {
                title,
                series,
                bars,
                unit,
            } => {
                bounded_label(title, "stacked bar title")?;
                optional_label(unit, "stacked bar unit")?;
                validate_labels(series, "stacked bar series", 12)?;
                check(
                    !bars.is_empty() && bars.len() <= 32,
                    "stacked bar requires 1-32 bars",
                )?;
                for bar in bars {
                    bounded_label(&bar.label, "stacked bar label")?;
                    check(
                        bar.values.len() == series.len()
                            && bar
                                .values
                                .iter()
                                .all(|value| value.is_finite() && *value >= 0.0),
                        "stacked bar values must match series and be finite and non-negative",
                    )?;
                }
            }
            WidgetKind::StateTimeline { title, lanes } => {
                bounded_label(title, "state timeline title")?;
                check(
                    !lanes.is_empty() && lanes.len() <= 32,
                    "state timeline requires 1-32 lanes",
                )?;
                for lane in lanes {
                    bounded_label(&lane.label, "state timeline lane")?;
                    check(
                        !lane.segments.is_empty() && lane.segments.len() <= 256,
                        "state timeline lane requires 1-256 segments",
                    )?;
                    let mut previous_end = f64::NEG_INFINITY;
                    for segment in &lane.segments {
                        check(
                            segment.start.is_finite()
                                && segment.end.is_finite()
                                && segment.start < segment.end
                                && segment.start >= previous_end,
                            "state timeline segments must be finite, ordered, and non-overlapping",
                        )?;
                        optional_label(&segment.label, "state segment label")?;
                        previous_end = segment.end;
                    }
                }
            }
            WidgetKind::SloBurnRate {
                title,
                objective,
                windows,
            } => {
                bounded_label(title, "SLO burn-rate title")?;
                check(
                    objective.is_finite() && *objective > 0.0 && *objective <= 100.0,
                    "SLO objective must be within 0-100 percent",
                )?;
                check(
                    !windows.is_empty() && windows.len() <= 16,
                    "SLO burn-rate requires 1-16 windows",
                )?;
                for window in windows {
                    bounded_label(&window.label, "SLO burn-rate window")?;
                    check(
                        window.burn_rate.is_finite() && window.burn_rate >= 0.0,
                        "SLO burn rate must be finite and non-negative",
                    )?;
                }
            }
            WidgetKind::Table {
                title,
                columns,
                rows,
            } => {
                bounded_label(title, "table title")?;
                check(
                    !columns.is_empty() && columns.len() <= 12,
                    "table requires 1-12 columns",
                )?;
                check(rows.len() <= 256, "table exceeds 256 rows")?;
                for column in columns {
                    bounded_label(column, "table column")?;
                }
                check(
                    rows.iter().all(|row| row.len() == columns.len()),
                    "table rows must match column count",
                )?;
                check(
                    rows.iter()
                        .flatten()
                        .all(|cell| cell.len() <= MAX_INPUT_TEXT_BYTES),
                    "table cell exceeds 4096 bytes",
                )?;
            }
            WidgetKind::EventLog { title, entries } => {
                bounded_label(title, "event log title")?;
                check(entries.len() <= 256, "event log exceeds 256 entries")?;
                check(
                    entries.iter().all(|entry| {
                        entry.timestamp.len() <= 64 && entry.message.len() <= MAX_INPUT_TEXT_BYTES
                    }),
                    "event log entry is too large",
                )?;
            }
            WidgetKind::NodeSummary { title, nodes } => {
                bounded_label(title, "node summary title")?;
                check(nodes.len() <= 256, "node summary exceeds 256 nodes")?;
                for node in nodes {
                    bounded_label(&node.label, "node label")?;
                    optional_label(&node.detail, "node detail")?;
                }
            }
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

fn bounded_label(value: &str, name: &str) -> Result<()> {
    check(
        !value.is_empty() && value.len() <= 128,
        &format!("{name} must contain 1-128 bytes"),
    )
}

fn optional_label(value: &Option<String>, name: &str) -> Result<()> {
    if let Some(value) = value {
        bounded_label(value, name)
    } else {
        Ok(())
    }
}

fn optional_bounded_text(value: &Option<String>, name: &str, max: usize) -> Result<()> {
    if let Some(value) = value {
        check(
            !value.is_empty() && value.len() <= max,
            &format!("{name} must contain 1-{max} bytes"),
        )
    } else {
        Ok(())
    }
}

fn validate_labels(values: &[String], name: &str, max: usize) -> Result<()> {
    check(
        !values.is_empty() && values.len() <= max,
        &format!("{name} requires 1-{max} values"),
    )?;
    for value in values {
        bounded_label(value, name)?;
    }
    Ok(())
}

fn validate_f32_series(values: &[f32], name: &str) -> Result<()> {
    check(
        values.len() <= MAX_SERIES_POINTS && values.iter().all(|value| value.is_finite()),
        &format!("{name} must contain at most 4096 finite points"),
    )
}

#[cfg(feature = "live")]
pub mod live;

/// Static PNG rendering through external tools (matplotlib / plotters helper).
pub mod render;

#[cfg(test)]
mod known_kind_tests {
    use super::*;

    #[test]
    fn every_known_widget_kind_is_listed() {
        // The list and the enum drift the moment someone adds a variant and not
        // a tag — and the symptom would be a real widget quietly degrading to a
        // placeholder, which looks like a rendering bug rather than a stale list.
        for tag in KNOWN_WIDGET_KINDS {
            let spec = serde_json::json!({"kind": tag});
            // A bare tag may parse (kinds whose fields all default, like
            // `clock`) or fail for a missing field. Either proves the variant
            // exists; only "unknown variant" proves it does not.
            if let Err(error) = serde_json::from_value::<WidgetKind>(spec) {
                assert!(
                    !error.to_string().contains("unknown variant"),
                    "{tag} is listed but is not a WidgetKind variant"
                );
            }
        }
        assert!(!is_known_widget_kind("flux_capacitor"));
    }
}

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

    #[test]
    fn canvas_demo_specs_use_the_production_widget_schema() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/widgets");
        for name in [
            "demo-clock.json",
            "demo-ticker.json",
            "demo-gauge.json",
            "demo-meter.json",
            "demo-status.json",
            "ops-stat.json",
            "ops-timeseries.json",
            "ops-histogram.json",
            "ops-table.json",
            "ops-events.json",
            "ops-nodes.json",
            "quant-candles.json",
        ] {
            WidgetSpec::from_file(&root.join(name))
                .unwrap_or_else(|error| panic!("{name} is invalid: {error}"));
        }
    }

    #[test]
    fn operational_inputs_are_bounded_and_drag_only_by_default() {
        let stat = WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":42,"history":[1,2,3]}"#,
        )
        .unwrap();
        assert_eq!(stat.chrome, WidgetChrome::DragOnly);

        let oversized_rows = vec![vec!["ok".to_string()]; 257];
        let invalid = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "too-many-rows".into(),
            kind: WidgetKind::Table {
                title: "Targets".into(),
                columns: vec!["State".into()],
                rows: oversized_rows,
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::DragOnly,
            data: WidgetData::default(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn connector_metadata_and_data_state_roundtrip() {
        let spec = WidgetSpec::from_json(
            br#"{
                "id":"requests",
                "kind":"stat",
                "label":"Requests",
                "value":42,
                "data":{
                    "state":"stale",
                    "source":"prometheus:bifrost",
                    "observed_at":"2026-09-08T19:30:00Z",
                    "window":"5m",
                    "detail":"Last scrape was 42s ago"
                }
            }"#,
        )
        .expect("metadata is valid");
        assert_eq!(spec.data.state, WidgetDataState::Stale);
        assert_eq!(spec.data.source.as_deref(), Some("prometheus:bifrost"));
        let round_trip = WidgetSpec::from_json(&serde_json::to_vec(&spec).unwrap()).unwrap();
        assert_eq!(round_trip, spec);
    }

    #[test]
    fn connector_metadata_is_bounded() {
        let detail = "x".repeat(MAX_INPUT_TEXT_BYTES + 1);
        let json = serde_json::json!({
            "id": "requests",
            "kind": "stat",
            "label": "Requests",
            "value": 42,
            "data": { "detail": detail }
        });
        assert!(WidgetSpec::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }

    #[test]
    fn operational_chart_families_validate() {
        for json in [
            r#"{"id":"heat","kind":"heatmap","title":"Latency","rows":["api"],"columns":["p50","p99"],"values":[[12,48]],"unit":"ms"}"#,
            r#"{"id":"rank","kind":"ranked_bar","title":"Tokens","bars":[{"label":"agent-a","value":42}]}"#,
            r#"{"id":"stack","kind":"stacked_bar","title":"Outcomes","series":["ok","error"],"bars":[{"label":"run","values":[9,1]}]}"#,
            r#"{"id":"states","kind":"state_timeline","title":"Health","lanes":[{"label":"api","segments":[{"start":0,"end":10,"state":"good"}]}]}"#,
            r#"{"id":"burn","kind":"slo_burn_rate","title":"Budget","objective":99.9,"windows":[{"label":"5m","burn_rate":0.7}]}"#,
        ] {
            WidgetSpec::from_json(json.as_bytes())
                .unwrap_or_else(|error| panic!("{json}: {error}"));
        }
    }

    #[test]
    fn malformed_operational_charts_fail_loudly() {
        for json in [
            r#"{"id":"heat","kind":"heatmap","title":"Latency","rows":["api"],"columns":["p50"],"values":[[1,2]]}"#,
            r#"{"id":"stack","kind":"stacked_bar","title":"Outcomes","series":["ok"],"bars":[{"label":"run","values":[1,2]}]}"#,
            r#"{"id":"states","kind":"state_timeline","title":"Health","lanes":[{"label":"api","segments":[{"start":4,"end":2,"state":"critical"}]}]}"#,
        ] {
            assert!(
                WidgetSpec::from_json(json.as_bytes()).is_err(),
                "accepted {json}"
            );
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
