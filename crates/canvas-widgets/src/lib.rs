//! Canvas widget library: reusable presentation primitives with a durable spec model.
//!
//! A [`WidgetSpec`] is presentation intent — the same JSON renders live inside
//! Canvas (iced-native, `live` feature) or statically through external tools
//! (matplotlib, doc-forge, mmdc) exactly like the mermaid `.mmd` pipeline.
//! Ownership of rendering lives in one place per backend; everything else is a
//! client of this crate.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
#[cfg(test)]
use std::path::PathBuf;

pub mod quant;
pub mod roster;

pub const WIDGET_SCHEMA_VERSION: u32 = 2;
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

/// Rewrites v1 widget `kind` tags to their v2 names, so old saved views
/// still load after the schema bump. Returns `None` if no known v1 tag
/// was found, so the caller can surface the original parse error.
fn migrate_v1_kind(json: &[u8]) -> Option<Vec<u8>> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let kind = value.get("kind")?.as_str()?;
    let new_kind = match kind {
        "slo_burn_rate" => "rate_bars",
        "node_summary" => "status_list",
        _ => return None,
    };
    let mut migrated = value;
    if let Some(obj) = migrated.as_object_mut() {
        obj.insert(
            "kind".to_string(),
            serde_json::Value::String(new_kind.to_string()),
        );
        serde_json::to_vec(&migrated).ok()
    } else {
        None
    }
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

/// Window treatment for a widget opened as a Canvas surface.
///
/// `Auto` chooses a kind-appropriate treatment: instruments such as clocks do
/// not pretend to be app windows, while ordinary widgets retain visible window
/// controls. Lua and JSON may override that decision per widget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetChrome {
    #[default]
    Auto,
    Standard,
    Integrated,
    DragOnly,
    Frameless,
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
pub const KNOWN_WIDGET_KINDS: &[&str] = &[
    "quant",
    "stat",
    "time_series",
    "media_card",
    "clock",
    "ticker",
    "gauge",
    "meter",
    "daw_deck",
    "mix_field",
    "histogram",
    "spectral_waterfall",
    "heatmap",
    "ranked_bar",
    "stacked_bar",
    "pie_chart",
    "sankey",
    "hierarchy",
    "metric_board",
    "distribution",
    "rich_card",
    "approval_card",
    "command_palette",
    "media_transport",
    "model_telemetry",
    "span_timeline",
    "state_timeline",
    "rate_bars",
    "table",
    "event_log",
    "social_feed",
    "status_list",
    "neural_graph",
    "network_topology",
    "dag_graph",
    "spatial_tracks",
    "chart",
    "status",
    "text",
    "module",
    "launcher",
    "hyperspace_shader",
    "lyrics_display",
    "cdg_viewer",
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
    /// Retained media metadata with host-resolved artwork and smooth progress.
    /// Producers replace this snapshot on state changes; Rust extrapolates the
    /// progress rail between observations without invoking Lua per frame.
    MediaCard {
        title: String,
        artist: String,
        #[serde(default)]
        album: Option<String>,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        artwork_source: Option<String>,
        elapsed_seconds: f64,
        duration_seconds: f64,
        observed_at_ms: i64,
        #[serde(default)]
        playing: bool,
    },
    /// Local time. `format` uses strftime-style tokens.
    ///
    /// The one documented intrinsic exception to "widgets must not source their
    /// own data." No other kind reads the clock, filesystem, or network; this one
    /// does because a wall clock is presentation metadata rather than a
    /// measurement. Every other widget receives its data through a declared
    /// input contract.
    Clock {
        #[serde(default = "default_clock_format")]
        format: String,
        /// Presentation mode is data-driven so Lua can restyle a clock without
        /// exposing another control in the visible window chrome.
        #[serde(default)]
        mode: ClockMode,
    },
    /// Quote ticker. Entries carry explicit label, value, and optional status —
    /// resolved by a data stream or declared input, never invented by the widget.
    Ticker { entries: Vec<BarValue> },
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
    /// Live studio transport with reel motion, piano-roll notes, and channel meters.
    DawDeck {
        title: String,
        playing: bool,
        paused: bool,
        #[serde(default)]
        recording: bool,
        #[serde(default)]
        bpm: Option<f64>,
        position_ticks: u64,
        cycle_ticks: u64,
        tracks: Vec<DawTrack>,
        notes: Vec<DawNote>,
    },
    /// Source energy positioned across the stereo field. Synesthesia owns the
    /// measurements; Canvas only renders this bounded projection.
    MixField {
        title: String,
        surface: String,
        strips: Vec<MixFieldStrip>,
    },
    /// Prometheus histogram buckets rendered as an operational distribution.
    Histogram {
        title: String,
        buckets: Vec<HistogramBucket>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Rolling frequency-by-time intensity history. Stream binders append one
    /// normalized FFT row at a time; Rust retains the bounded history.
    SpectralWaterfall {
        title: String,
        rows: Vec<Vec<f32>>,
        min_frequency: f64,
        max_frequency: f64,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default = "default_waterfall_history")]
        history: usize,
        /// Latest radio-domain frame metadata. Radioman owns these facts;
        /// Canvas retains only the bounded projection needed to draw them.
        #[serde(default)]
        radio: Option<RadioSpectrumState>,
    },
    /// Dense row/column intensity matrix for latency, load, and correlation.
    Heatmap {
        title: String,
        rows: Vec<String>,
        columns: Vec<String>,
        values: Vec<Vec<f64>>,
        #[serde(default)]
        style: HeatmapStyle,
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
    /// Part-to-whole categorical values. Labels are revealed on pointer hover
    /// so dense model names do not turn the chart into a permanent legend.
    PieChart {
        title: String,
        values: Vec<BarValue>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        period: Option<String>,
    },
    /// Weighted directed flows. Node placement is inferred by the retained
    /// renderer so Lua producers only describe identity and magnitude.
    Sankey {
        title: String,
        nodes: Vec<FlowNode>,
        links: Vec<FlowLink>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// One validated hierarchy rendered as either area allocation or radial
    /// dependency rings. The data contract stays identical across views.
    Hierarchy {
        title: String,
        style: HierarchyStyle,
        nodes: Vec<HierarchyNode>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        focus: Option<String>,
    },
    /// Related target-bearing measures rendered as radial instruments or
    /// compact bullet comparisons from one semantic contract.
    MetricBoard {
        title: String,
        style: MetricBoardStyle,
        metrics: Vec<TargetMetric>,
        #[serde(default)]
        unit: Option<String>,
    },
    Distribution {
        title: String,
        style: DistributionStyle,
        groups: Vec<DistributionGroup>,
        #[serde(default)]
        unit: Option<String>,
    },
    RichCard {
        title: String,
        format: RichCardFormat,
        body: String,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        citations: Vec<Citation>,
    },
    ApprovalCard {
        request_id: String,
        title: String,
        summary: String,
        scope: Vec<String>,
        expires_at: String,
        provenance: String,
        approve_action: String,
        deny_action: String,
        #[serde(default)]
        status: ApprovalStatus,
    },
    CommandPalette {
        query: String,
        results: Vec<CommandResult>,
        #[serde(default)]
        selected: usize,
    },
    MediaTransport {
        title: String,
        source: String,
        codec: String,
        width: u32,
        height: u32,
        fps: f32,
        bitrate_kbps: f64,
        buffer_ms: f64,
        delay_ms: f64,
        route: Vec<String>,
        #[serde(default)]
        state: Option<StatusLevel>,
    },
    ModelTelemetry {
        title: String,
        model: String,
        view: ModelView,
        annotations: Vec<ModelAnnotation>,
        #[serde(default)]
        exploded: f32,
    },
    /// Bounded timed spans rendered as a flame graph, request waterfall,
    /// execution Gantt, or event river from one shared retained primitive.
    SpanTimeline {
        title: String,
        style: SpanTimelineStyle,
        spans: Vec<TimedSpan>,
        #[serde(default)]
        unit: Option<String>,
    },
    /// Bounded state intervals grouped into named lanes.
    StateTimeline {
        title: String,
        lanes: Vec<StateLane>,
    },
    /// Error-budget consumption across operational windows.
    ///
    /// Renamed from `slo_burn_rate` in schema version 2; old specs are
    /// migrated by `from_v1_tag`.
    RateBars {
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
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        paused: bool,
        #[serde(default = "default_true")]
        follow: bool,
    },
    /// Rotating social posts rendered as readable cards rather than tabular rows.
    SocialFeed {
        title: String,
        status: String,
        #[serde(default)]
        detail: Option<String>,
        posts: Vec<SocialFeedPost>,
        #[serde(default = "default_social_feed_rotate_seconds")]
        rotate_seconds: u64,
    },
    /// Compact health summary for nodes, agents, or DAG stages.
    ///
    /// Renamed from `node_summary` in schema version 2; old specs are
    /// migrated by `from_v1_tag`.
    StatusList {
        title: String,
        nodes: Vec<NodeSummaryItem>,
    },
    /// Live topology with short-lived activity heat on nodes and edges.
    ///
    /// Producers own decay and replace this bounded snapshot; the widget owns
    /// only deterministic layout and presentation. This keeps clocks, sockets,
    /// and transport retries out of the renderer.
    NeuralGraph {
        title: String,
        nodes: Vec<NeuralNode>,
        #[serde(default)]
        edges: Vec<NeuralEdge>,
    },
    /// Layer-2/3 topology with explicit LAN, VLAN, WAN, and point-to-point zones.
    NetworkTopology {
        title: String,
        segments: Vec<NetworkSegment>,
        nodes: Vec<NetworkNode>,
        #[serde(default)]
        links: Vec<NetworkLink>,
        #[serde(default)]
        conflicts: usize,
    },
    /// Rooted execution lineage with deterministic parent-before-child layout.
    /// Intended for agents/sub-agents, Merkle trees, and workflow ancestry.
    DagGraph {
        title: String,
        nodes: Vec<NeuralNode>,
        #[serde(default)]
        edges: Vec<NeuralEdge>,
    },
    /// Top-down positions and bounded motion trails in room coordinates.
    SpatialTracks {
        title: String,
        tracks: Vec<SpatialTrack>,
        bounds: SpatialBounds,
    },
    /// Data chart. `source` is a content-addressed asset identifier (sha256)
    /// resolved by the host; the widget itself never touches the filesystem.
    /// `renderer` selects the external pipeline for static output (plotters
    /// native by default).
    Chart {
        chart_type: ChartType,
        source: String,
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
    /// go looking for any — which is the opposite of what [`WidgetKind::Clock`]
    /// does today, reading the host clock. Clock is the documented exception
    /// (intrinsic rather than sourced); a module that wants live quotes must
    /// receive them through a declared input contract.
    Module {
        module: String,
        #[serde(default)]
        inputs: BTreeMap<String, WidgetInput>,
    },
    /// A horizontal launcher bar of project tiles with unread indicators.
    /// Each tile carries a project_id, label, optional icon, and unread flag.
    /// Clicking a tile emits a `canvas.widget.activation` event with the
    /// project_id — the host routes it to workspace switch.
    Launcher {
        items: Vec<LauncherItem>,
        #[serde(default)]
        pinned: Vec<String>,
        #[serde(default)]
        orientation: LauncherOrientation,
    },
    /// Hyperspace shader visualizer.
    HyperspaceShader {
        /// Shader name (e.g., "hyperspace_tunnel", "acid_worms").
        shader_id: String,
        /// Animation speed parameter.
        speed: f32,
        /// Resolution scale.
        scale: f32,
    },
    /// CDG (CD+G) graphics viewer for karaoke. Renders CDG frames with timing.
    CdgViewer {
        title: String,
        /// CDG file source (path or URL).
        source: String,
        /// Current frame index.
        current_frame: u32,
        /// Total frames.
        total_frames: u32,
        /// Playback position in ms.
        position_ms: i64,
        /// Total duration in ms.
        duration_ms: i64,
        /// Whether audio is playing.
        playing: bool,
        /// Scale factor for CDG rendering.
        scale: f32,
    },
    /// Lyrics display with hyperspace animations and karaoke-style highlighting.
    /// Producers push lyric lines with timestamps; renderer interpolates scroll
    /// position between observations for smooth playback.
    LyricsDisplay {
        /// Current track metadata.
        title: String,
        artist: String,
        /// Album art source handle (host-resolved).
        #[serde(default)]
        artwork_source: Option<String>,
        /// Lyric lines: text with start/end in milliseconds and active state.
        lines: Vec<LyricLine>,
        /// Current playback position for determining active line.
        position_ms: i64,
        duration_ms: i64,
        playing: bool,
        /// Animation style for hyperspace effects.
        #[serde(default)]
        style: LyricsStyle,
        /// Scroll offset in lines (floats for interpolation).
        scroll_offset: f32,
        /// Highlight intensity for active line (0..1, interpolated).
        highlight_intensity: f32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockMode {
    #[default]
    Digital,
    Analog,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LauncherItem {
    pub project_id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub last_used: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum LauncherOrientation {
    #[default]
    Horizontal,
    Vertical,
}

/// Small, typed changes accepted from host-owned stream bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WidgetMutation {
    Quant {
        mutation: quant::QuantMutation,
    },
    SpectralFrame {
        bins: Vec<f32>,
        #[serde(default)]
        radio: Option<RadioSpectrumState>,
    },
    /// Replace only a widget's bounded, typed data payload. Layout, identity,
    /// theme and chrome remain owned by the saved Canvas view.
    Snapshot {
        widget: WidgetKind,
        #[serde(default)]
        data: WidgetData,
    },
}

/// Narrow presentation projection of a radio spectrum frame.
///
/// This deliberately contains no device, tuning, or session commands. Those
/// belong to Radioman; Canvas only needs enough state to render the instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RadioSpectrumState {
    pub schema_version: u32,
    pub session_id: String,
    pub generation: u64,
    pub sequence: u64,
    pub observed_at_ms: i64,
    pub center_frequency_hz: f64,
    pub span_hz: f64,
    pub floor_dbfs: f32,
    pub ceiling_dbfs: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at_ms: Option<i64>,
}

/// A paced step in a Canvas briefing presentation.
///
/// Unlike [`WidgetMutation`], a `BriefingBeat` affects the whole workspace
/// document (backgrounds, themes, layouts, surfaces) and may request TTS
/// narration. Producers emit one envelope per beat; the binder renders the
/// beat into this structure, and Canvas applies the patches atomically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BriefingBeat {
    pub beat: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrate: Option<String>,
    #[serde(default)]
    pub wait_seconds: f32,
    #[serde(default)]
    pub patches: Vec<PatchOp>,
    /// Actions applied before the first beat and after the last, respectively.
    /// Each is a list of PatchOp that target the per-zone state: hiding a
    /// mesh node (Conky), muting presence, etc. The zone is the display
    /// Canvas is presenting on, so the effect is scoped rather than global.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_actions: Vec<PatchOp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_actions: Vec<PatchOp>,
    #[serde(default)]
    pub done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum PatchOp {
    UpsertSurface {
        surface: BriefingSurface,
    },
    RemoveSurface {
        surface_id: String,
    },
    SetLayout {
        layout: serde_json::Value,
    },
    SetTheme {
        theme: serde_json::Value,
    },
    SetBackground {
        background: serde_json::Value,
    },
    SetChrome {
        chrome: serde_json::Value,
    },
    SetFocus {
        surface_id: Option<String>,
    },
    UpsertBinding {
        binding: serde_json::Value,
    },
    RemoveBinding {
        binding_id: String,
    },
    /// Hide a mesh node (e.g. Conky) for the duration of the briefing.
    /// Restored by post_actions with SetMesh or by the mesh returning.
    SetMeshNode {
        node_id: String,
        hidden: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BriefingSurface {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub inputs: Vec<serde_json::Value>,
}

const MAX_BRIEFING_PATCHES: usize = 64;

impl BriefingBeat {
    pub fn validate(&self) -> Result<()> {
        check(
            self.wait_seconds.is_finite() && (0.0..=300.0).contains(&self.wait_seconds),
            "wait_seconds must be between 0 and 300",
        )?;
        check(
            self.patches.len() <= MAX_BRIEFING_PATCHES,
            &format!("briefing beat exceeds {} patches", MAX_BRIEFING_PATCHES),
        )?;
        for patch in &self.patches {
            match patch {
                PatchOp::UpsertSurface { surface } => {
                    check(
                        !surface.id.is_empty() && surface.id.len() <= 64,
                        "surface id must be 1-64 chars",
                    )?;
                    check(
                        !surface.title.is_empty() && surface.title.len() <= 128,
                        "surface title must be 1-128 chars",
                    )?;
                }
                PatchOp::RemoveSurface { surface_id } => {
                    check(
                        !surface_id.is_empty() && surface_id.len() <= 64,
                        "surface id must be 1-64 chars",
                    )?;
                }
                PatchOp::SetFocus {
                    surface_id: Some(id),
                } => {
                    check(
                        !id.is_empty() && id.len() <= 64,
                        "surface id must be 1-64 chars",
                    )?;
                }
                PatchOp::RemoveBinding { binding_id } => {
                    check(
                        !binding_id.is_empty() && binding_id.len() <= 64,
                        "binding id must be 1-64 chars",
                    )?;
                }
                PatchOp::SetMeshNode { node_id, .. } => {
                    check(
                        !node_id.is_empty() && node_id.len() <= 64,
                        "mesh node id must be 1-64 chars",
                    )?;
                }
                _ => {}
            }
        }
        if let Some(narrate) = &self.narrate {
            check(narrate.len() <= 4096, "narration text exceeds 4096 bytes")?;
        }
        Ok(())
    }
}

impl WidgetSpec {
    /// Window treatment after applying the widget kind's sane default.
    pub fn effective_chrome(&self) -> WidgetChrome {
        match (self.chrome, &self.kind) {
            (
                WidgetChrome::Auto,
                WidgetKind::Clock { .. }
                | WidgetKind::MediaCard { .. }
                | WidgetKind::LyricsDisplay { .. },
            ) => WidgetChrome::Frameless,
            (WidgetChrome::Auto, _) => WidgetChrome::Standard,
            (explicit, _) => explicit,
        }
    }

    /// Fastest useful redraw interval for intrinsic widget animation.
    ///
    /// The host combines this demand with cameras, backgrounds, and other
    /// animation sources. Keeping the policy beside the widget definition
    /// prevents hosts from accumulating kind-specific cadence checks.
    pub fn animation_interval_ms(&self) -> Option<u64> {
        match self.kind {
            WidgetKind::Clock {
                mode: ClockMode::Analog,
                ..
            } => Some(40),
            WidgetKind::MediaCard { playing: true, .. } => Some(40),
            WidgetKind::Quant {
                widget: quant::QuantWidget::MevOpportunity { .. },
            } => Some(40),
            // Lua only replaces the retained samples when data changes. Rust
            // owns the short interpolation window between snapshots, using the
            // same 25 fps cadence as the analog clock.
            WidgetKind::TimeSeries { .. } => Some(40),
            // Incoming FFT rows move independently of clocks and other widgets.
            // A modest cadence keeps the waterfall flowing without asking the
            // host for display-rate frames.
            WidgetKind::SpectralWaterfall { .. } => Some(80),
            _ => None,
        }
    }

    pub fn apply_mutation(&mut self, mutation: WidgetMutation) -> Result<()> {
        match (&mut self.kind, mutation) {
            (WidgetKind::Quant { widget }, WidgetMutation::Quant { mutation }) => {
                widget.apply(mutation)?;
            }
            (
                WidgetKind::SpectralWaterfall {
                    rows,
                    history,
                    radio: current_radio,
                    ..
                },
                WidgetMutation::SpectralFrame { bins, radio },
            ) => {
                let valid_width = if radio.is_some() { 64..=4096 } else { 8..=1024 };
                check(
                    valid_width.contains(&bins.len())
                        && bins
                            .iter()
                            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
                    "spectral frame requires 64-4096 Radioman bins or 8-1024 legacy bins",
                )?;
                check(
                    rows.first().is_none_or(|row| row.len() == bins.len()),
                    "spectral frame width changed",
                )?;
                rows.push(bins);
                if rows.len() > *history {
                    rows.drain(..rows.len() - *history);
                }
                if radio.is_some() {
                    *current_radio = radio;
                }
                // A validated frame is the authoritative readiness signal.
                // Leaving the initial loading state intact hides real rows
                // behind the host's loading presentation forever.
                self.data.state = WidgetDataState::Ready;
                self.data.detail = None;
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
/// A single lyric line with timing information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LyricLine {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub words: Vec<LyricWord>,
}

/// Word-level timing for karaoke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LyricWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Hyperspace animation style for lyrics display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum LyricsStyle {
    #[default]
    /// Subtle pulse and fade effects.
    Normal,
    /// Glitchy, cyberpunk aesthetic with chromatic aberration.
    Glitch,
    /// Flowing wave animations with liquid transitions.
    Flow,
    /// Particles emanating from active line like stardust.
    Particles,
    /// Deep space expansion with parallax depth.
    Hyperspace,
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
            data: WidgetData {
                state: WidgetDataState::Loading,
                detail: Some("Awaiting RF frames".into()),
                ..Default::default()
            },
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

    #[test]
    fn spectral_frames_append_and_evict_inside_rust() {
        let mut spec = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "sdr".into(),
            kind: WidgetKind::SpectralWaterfall {
                title: "SDR".into(),
                rows: Vec::new(),
                min_frequency: 88.0,
                max_frequency: 108.0,
                unit: Some("MHz".into()),
                history: 8,
                radio: None,
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::Auto,
            data: WidgetData::default(),
        };
        for frame in 0..10 {
            spec.apply_mutation(WidgetMutation::SpectralFrame {
                bins: vec![frame as f32 / 10.0; 8],
                radio: None,
            })
            .unwrap();
        }
        let WidgetKind::SpectralWaterfall { rows, .. } = spec.kind else {
            panic!("spectral mutation changed widget family")
        };
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0][0], 0.2);
        assert_eq!(rows[7][0], 0.9);
        assert_eq!(spec.data.state, WidgetDataState::Ready);
        assert_eq!(spec.data.detail, None);
    }

    #[test]
    fn briefing_beat_validates_bounds_and_ids() {
        let valid = BriefingBeat {
            beat: 0,
            narrate: Some("hello".into()),
            wait_seconds: 3.0,
            patches: vec![],
            pre_actions: vec![],
            post_actions: vec![],
            done: false,
            message: None,
        };
        assert!(valid.validate().is_ok());

        let negative_wait = BriefingBeat {
            beat: 0,
            narrate: None,
            wait_seconds: -1.0,
            patches: vec![],
            pre_actions: vec![],
            post_actions: vec![],
            done: false,
            message: None,
        };
        assert!(negative_wait.validate().is_err());

        let empty_surface = BriefingBeat {
            beat: 1,
            narrate: None,
            wait_seconds: 1.0,
            patches: vec![PatchOp::UpsertSurface {
                surface: BriefingSurface {
                    id: String::new(),
                    title: "t".into(),
                    content: serde_json::Value::Null,
                    inputs: vec![],
                },
            }],
            pre_actions: vec![],
            post_actions: vec![],
            done: false,
            message: None,
        };
        assert!(empty_surface.validate().is_err());
    }

    #[test]
    fn briefing_beat_roundtrips_through_json() {
        let beat = BriefingBeat {
            beat: 3,
            narrate: Some("What would you like to work on?".into()),
            wait_seconds: 5.0,
            patches: vec![
                PatchOp::SetBackground {
                    background: serde_json::json!({"kind": "shader", "shader_id": "smoke-rings", "speed": 0.6}),
                },
                PatchOp::SetChrome {
                    chrome: serde_json::json!({"title": "focused", "canvas_padding": 24, "surface_gap": 16}),
                },
            ],
            pre_actions: vec![],
            post_actions: vec![],
            done: false,
            message: None,
        };
        let json = serde_json::to_string(&beat).unwrap();
        let reparsed: BriefingBeat = serde_json::from_str(&json).unwrap();
        assert_eq!(beat, reparsed);
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    fn clock(mode: ClockMode) -> WidgetSpec {
        WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "clock".into(),
            kind: WidgetKind::Clock {
                format: "%H:%M:%S".into(),
                mode,
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::Standard,
            data: WidgetData::default(),
        }
    }

    fn media_card(playing: bool) -> WidgetSpec {
        WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "now-playing".into(),
            kind: WidgetKind::MediaCard {
                title: "Everything In Its Right Place".into(),
                artist: "Radiohead".into(),
                album: Some("Kid A".into()),
                source: Some("Spotify".into()),
                artwork_source: Some("now-playing-art".into()),
                elapsed_seconds: 42.0,
                duration_seconds: 251.0,
                observed_at_ms: 1_000,
                playing,
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::Auto,
            data: WidgetData::default(),
        }
    }

    #[test]
    fn only_the_sweeping_clock_requests_animation_frames() {
        assert_eq!(clock(ClockMode::Analog).animation_interval_ms(), Some(40));
        assert_eq!(clock(ClockMode::Digital).animation_interval_ms(), None);
    }

    #[test]
    fn only_playing_media_requests_animation_frames() {
        assert_eq!(media_card(true).animation_interval_ms(), Some(40));
        assert_eq!(media_card(false).animation_interval_ms(), None);
        assert_eq!(media_card(true).effective_chrome(), WidgetChrome::Frameless);
    }

    #[test]
    fn time_series_uses_the_clock_quality_cadence() {
        let spec = WidgetSpec {
            schema_version: WIDGET_SCHEMA_VERSION,
            id: "trend".into(),
            kind: WidgetKind::TimeSeries {
                title: "Trend".into(),
                series: Vec::new(),
                unit: None,
            },
            theme: None,
            refresh_ms: None,
            chrome: WidgetChrome::Auto,
            data: WidgetData::default(),
        };
        assert_eq!(spec.animation_interval_ms(), Some(40));
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
pub struct FlowNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub state: Option<StatusLevel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FlowLink {
    pub from: String,
    pub to: String,
    pub value: f64,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum HierarchyStyle {
    Treemap,
    Sunburst,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HierarchyNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MetricBoardStyle {
    Radial,
    Bullet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TargetMetric {
    pub label: String,
    pub value: f64,
    pub max: f64,
    #[serde(default)]
    pub target: Option<f64>,
    #[serde(default)]
    pub forecast: Option<f64>,
    #[serde(default)]
    pub state: Option<StatusLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DistributionStyle {
    Box,
    Violin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DistributionGroup {
    pub label: String,
    pub samples: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum RichCardFormat {
    Markdown,
    Code,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Citation {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ApprovalStatus {
    #[default]
    Pending,
    Approved,
    Denied,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CommandResult {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ModelView {
    Orbit,
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelAnnotation {
    pub id: String,
    pub label: String,
    pub position: [f32; 3],
    pub value: String,
    #[serde(default)]
    pub state: Option<StatusLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SpanTimelineStyle {
    Flame,
    Waterfall,
    Gantt,
    EventRiver,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TimedSpan {
    pub id: String,
    pub label: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub state: Option<StatusLevel>,
    #[serde(default)]
    pub detail: Option<String>,
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

fn default_social_feed_rotate_seconds() -> u64 {
    8
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SocialFeedPost {
    pub id: String,
    pub author: String,
    pub text: String,
    pub age: String,
    pub engagement: u64,
    #[serde(default)]
    pub matches: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DawTrack {
    pub label: String,
    pub level: f32,
    #[serde(default)]
    pub clipping: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MixFieldStrip {
    pub index: usize,
    pub cc: u8,
    pub label: String,
    pub gain: f32,
    pub active: bool,
    pub peak: f32,
    pub stereo_position: f32,
    pub left_peak: f32,
    pub right_peak: f32,
    /// Optional normalized low-to-high frequency energy, populated by a
    /// separate analyzer rather than the real-time mixer.
    #[serde(default)]
    pub bands: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DawNote {
    pub track: String,
    pub tick: u64,
    pub length_ticks: u64,
    pub note: u8,
    pub velocity: u8,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NeuralNode {
    pub id: String,
    pub label: String,
    /// Secondary facts shown on focus/hover instead of permanently painted
    /// over dense graphs.
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// Host-computed recency, from idle (0) to just fired (1).
    pub heat: f32,
    #[serde(default = "neutral_status_level")]
    pub state: StatusLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NeuralEdge {
    pub from: String,
    pub to: String,
    /// Host-computed recency, from idle (0) to just traversed (1).
    pub heat: f32,
    /// Measured live activity on this edge. Zero means no animated traffic.
    #[serde(default)]
    pub activity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum NetworkSegmentKind {
    Lan,
    Vlan,
    Wan,
    PointToPoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkSegment {
    pub id: String,
    pub label: String,
    pub kind: NetworkSegmentKind,
    /// Observation site that owns this L2/L3 domain. Mycelium remains the
    /// source of truth; Canvas uses this only to group the rendered map.
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub vlan: Option<u16>,
    #[serde(default)]
    pub subnet: Option<String>,
    #[serde(default)]
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum NetworkNodeKind {
    Device,
    Host,
    Gateway,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkNode {
    pub id: String,
    pub label: String,
    pub kind: NetworkNodeKind,
    #[serde(default)]
    pub segments: Vec<String>,
    /// Sites from which this node was observed. More than one is meaningful
    /// for infrastructure joining routing domains.
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default = "neutral_status_level")]
    pub state: StatusLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum NetworkLinkKind {
    Access,
    Trunk,
    Uplink,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkLink {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
    pub kind: NetworkLinkKind,
    #[serde(default = "neutral_status_level")]
    pub state: StatusLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SpatialPoint {
    pub x: f64,
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SpatialTrack {
    pub id: u32,
    pub position: SpatialPoint,
    #[serde(default)]
    pub trail: Vec<SpatialPoint>,
    pub confidence: f64,
    pub motion: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SpatialBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
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

fn neutral_status_level() -> StatusLevel {
    StatusLevel::Neutral
}

fn validate_graph_id(id: &str, label: &str) -> Result<()> {
    check(
        !id.is_empty()
            && id.len() <= 128
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
            }),
        &format!("{label} requires a bounded identifier"),
    )
}

fn validate_graph(
    title: &str,
    nodes: &[NeuralNode],
    edges: &[NeuralEdge],
    label: &str,
) -> Result<()> {
    bounded_label(title, &format!("{label} graph title"))?;
    check(
        nodes.len() <= 512,
        &format!("{label} graph exceeds 512 nodes"),
    )?;
    check(
        edges.len() <= 2048,
        &format!("{label} graph exceeds 2048 edges"),
    )?;
    let mut ids = std::collections::BTreeSet::new();
    for node in nodes {
        check(
            !node.id.is_empty()
                && node.id.len() <= 128
                && node.id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
                })
                && ids.insert(node.id.as_str()),
            &format!("{label} graph nodes require unique, bounded IDs"),
        )?;
        bounded_graph_label(&node.label, &format!("{label} graph node label"))?;
        optional_bounded_text(&node.detail, &format!("{label} graph node detail"), 512)?;
        optional_label(&node.group, &format!("{label} graph node group"))?;
        check(
            node.heat.is_finite() && (0.0..=1.0).contains(&node.heat),
            &format!("{label} graph node heat must be finite within 0..1"),
        )?;
    }
    let mut pairs = std::collections::BTreeSet::new();
    for edge in edges {
        check(
            ids.contains(edge.from.as_str())
                && ids.contains(edge.to.as_str())
                && edge.from != edge.to
                && pairs.insert((edge.from.as_str(), edge.to.as_str()))
                && edge.heat.is_finite()
                && (0.0..=1.0).contains(&edge.heat)
                && edge.activity.is_finite()
                && (0.0..=1024.0).contains(&edge.activity),
            &format!(
                "{label} graph edges require distinct known nodes, unique pairs, heat within 0..1, and bounded activity"
            ),
        )?;
    }
    Ok(())
}

fn is_acyclic(nodes: &[NeuralNode], edges: &[NeuralEdge]) -> bool {
    let mut indegree = nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect::<std::collections::BTreeMap<_, _>>();
    for edge in edges {
        *indegree.get_mut(edge.to.as_str()).expect("validated edge") += 1;
    }
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop() {
        visited += 1;
        for edge in edges.iter().filter(|edge| edge.from == id) {
            let degree = indegree.get_mut(edge.to.as_str()).expect("validated edge");
            *degree -= 1;
            if *degree == 0 {
                ready.push(edge.to.as_str());
            }
        }
    }
    visited == nodes.len()
}

/// Ceilings for a module spec. Bounded because an agent writes these, and an
/// unbounded series is a denial of service against the renderer rather than a
/// rendering bug.
const MAX_MODULE_INPUTS: usize = 32;
const MAX_LAUNCHER_ITEMS: usize = 32;
const MAX_SERIES_POINTS: usize = 4096;
const MAX_INPUT_TEXT_BYTES: usize = 4096;

fn default_clock_format() -> String {
    "%H:%M:%S".into()
}

fn default_meter_style() -> MeterStyle {
    MeterStyle::Led
}

fn default_true() -> bool {
    true
}

fn default_waterfall_history() -> usize {
    96
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MeterStyle {
    Led,
    Vu,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum HeatmapStyle {
    #[default]
    Matrix,
    Activity,
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
        // Try v2 schema first. If parsing fails due to a renamed kind tag,
        // attempt v1 → v2 migration and retry.
        match serde_json::from_slice::<WidgetSpec>(json) {
            Ok(spec) => {
                spec.validate()?;
                Ok(spec)
            }
            Err(first_error) => {
                // Check if this is a known v1 kind rename.
                if let Some(migrated) = migrate_v1_kind(json) {
                    match serde_json::from_slice::<WidgetSpec>(&migrated) {
                        Ok(spec) => {
                            spec.validate()?;
                            Ok(spec)
                        }
                        Err(_) => Err(Error(format!("invalid widget spec: {first_error}"))),
                    }
                } else {
                    Err(Error(format!("invalid widget spec: {first_error}")))
                }
            }
        }
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
            WidgetKind::MediaCard {
                title,
                artist,
                album,
                source,
                artwork_source,
                elapsed_seconds,
                duration_seconds,
                observed_at_ms,
                ..
            } => {
                bounded_label(title, "media title")?;
                bounded_label(artist, "media artist")?;
                optional_label(album, "media album")?;
                optional_label(source, "media source")?;
                optional_label(artwork_source, "media artwork source")?;
                check(
                    elapsed_seconds.is_finite() && *elapsed_seconds >= 0.0,
                    "media elapsed seconds must be finite and non-negative",
                )?;
                check(
                    duration_seconds.is_finite() && *duration_seconds >= 0.0,
                    "media duration seconds must be finite and non-negative",
                )?;
                check(
                    *observed_at_ms >= 0,
                    "media observation time must be non-negative",
                )?;
            }
            WidgetKind::Ticker { entries } => {
                check(!entries.is_empty(), "ticker requires at least one entry")?;
                for entry in entries {
                    bounded_label(&entry.label, "ticker entry label")?;
                    check(
                        entry.value.is_finite() && entry.value >= 0.0,
                        "ticker entry value must be finite and non-negative",
                    )?;
                }
            }
            WidgetKind::Gauge { value, .. } => check(
                value.is_finite() && (0.0..=100.0).contains(value),
                "gauge value must be finite within 0..100",
            )?,
            WidgetKind::Meter { value, .. } => check(
                value.is_finite() && (0.0..=1.0).contains(value),
                "meter value must be finite within 0..1",
            )?,
            WidgetKind::DawDeck {
                title,
                bpm,
                position_ticks,
                cycle_ticks,
                tracks,
                notes,
                ..
            } => {
                bounded_label(title, "DAW deck title")?;
                check(
                    bpm.is_none_or(|value| value.is_finite() && (20.0..=300.0).contains(&value)),
                    "DAW deck BPM must be 20..300",
                )?;
                check(*cycle_ticks > 0, "DAW deck cycle must be non-zero")?;
                check(
                    *position_ticks < *cycle_ticks,
                    "DAW deck position exceeds cycle",
                )?;
                check(
                    !tracks.is_empty() && tracks.len() <= 32,
                    "DAW deck requires 1-32 tracks",
                )?;
                check(notes.len() <= 256, "DAW deck exceeds 256 notes")?;
                for track in tracks {
                    bounded_label(&track.label, "DAW track label")?;
                    check(
                        track.level.is_finite() && (0.0..=1.0).contains(&track.level),
                        "DAW track level must be within 0..1",
                    )?;
                }
                for note in notes {
                    bounded_label(&note.track, "DAW note track")?;
                    check(
                        note.note <= 127 && note.velocity <= 127 && note.length_ticks > 0,
                        "DAW note is invalid",
                    )?;
                    check(note.tick < *cycle_ticks, "DAW note starts outside cycle")?;
                }
            }
            WidgetKind::MixField {
                title,
                surface,
                strips,
            } => {
                bounded_label(title, "mix field title")?;
                bounded_label(surface, "mix field surface")?;
                check(
                    !strips.is_empty() && strips.len() <= 8,
                    "mix field requires 1-8 strips",
                )?;
                for strip in strips {
                    bounded_label(&strip.label, "mix field strip label")?;
                    check(
                        strip.index > 0 && strip.index <= 8,
                        "mix field strip index must be 1-8",
                    )?;
                    check(strip.cc <= 127, "mix field CC must be MIDI-safe")?;
                    check(
                        [strip.gain, strip.peak, strip.left_peak, strip.right_peak]
                            .into_iter()
                            .all(|value| value.is_finite() && (0.0..=2.0).contains(&value))
                            && strip.stereo_position.is_finite()
                            && (-1.0..=1.0).contains(&strip.stereo_position),
                        "mix field levels and stereo position must be finite and bounded",
                    )?;
                    check(
                        strip.bands.len() <= 64
                            && strip
                                .bands
                                .iter()
                                .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
                        "mix field exceeds 64 normalized bands",
                    )?;
                }
            }
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
            WidgetKind::SpectralWaterfall {
                title,
                rows,
                min_frequency,
                max_frequency,
                unit,
                history,
                radio,
            } => {
                bounded_label(title, "spectral waterfall title")?;
                optional_label(unit, "spectral waterfall unit")?;
                check(
                    min_frequency.is_finite()
                        && max_frequency.is_finite()
                        && min_frequency < max_frequency,
                    "spectral waterfall requires finite increasing frequency bounds",
                )?;
                check(
                    (8..=256).contains(history),
                    "spectral waterfall history must be within 8-256 rows",
                )?;
                check(
                    rows.len() <= *history,
                    "spectral waterfall rows exceed retained history",
                )?;
                if let Some(width) = rows.first().map(Vec::len) {
                    check(
                        (8..=4096).contains(&width)
                            && rows.iter().all(|row| {
                                row.len() == width
                                    && row.iter().all(|value| {
                                        value.is_finite() && (0.0..=1.0).contains(value)
                                    })
                            }),
                        "spectral waterfall rows require 8-1024 equal normalized bins",
                    )?;
                }
                if let Some(radio) = radio {
                    check(
                        radio.schema_version == 1
                            && !radio.session_id.is_empty()
                            && radio.session_id.len() <= 128
                            && radio.center_frequency_hz.is_finite()
                            && radio.center_frequency_hz > 0.0
                            && radio.span_hz.is_finite()
                            && radio.span_hz > 0.0
                            && radio.floor_dbfs.is_finite()
                            && radio.ceiling_dbfs.is_finite()
                            && radio.floor_dbfs < radio.ceiling_dbfs,
                        "radio spectrum metadata is invalid",
                    )?;
                }
            }
            WidgetKind::Heatmap {
                title,
                rows,
                columns,
                values,
                style,
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
                if *style == HeatmapStyle::Activity {
                    check(
                        (1..=7).contains(&rows.len()),
                        "activity heatmap requires 1-7 day rows",
                    )?;
                }
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
            WidgetKind::PieChart {
                title,
                values,
                unit,
                period,
            } => {
                bounded_label(title, "pie chart title")?;
                optional_label(unit, "pie chart unit")?;
                optional_label(period, "pie chart period")?;
                check(
                    !values.is_empty() && values.len() <= 24,
                    "pie chart requires 1-24 values",
                )?;
                for value in values {
                    bounded_label(&value.label, "pie chart label")?;
                    check(
                        value.value.is_finite() && value.value >= 0.0,
                        "pie chart values must be finite and non-negative",
                    )?;
                }
            }
            WidgetKind::Sankey {
                title,
                nodes,
                links,
                unit,
            } => {
                bounded_label(title, "sankey title")?;
                optional_label(unit, "sankey unit")?;
                check(
                    !nodes.is_empty() && nodes.len() <= 64,
                    "sankey requires 1-64 nodes",
                )?;
                check(
                    !links.is_empty() && links.len() <= 128,
                    "sankey requires 1-128 links",
                )?;
                let mut ids = HashSet::new();
                for node in nodes {
                    bounded_label(&node.id, "sankey node id")?;
                    bounded_label(&node.label, "sankey node label")?;
                    check(
                        ids.insert(node.id.as_str()),
                        "sankey node ids must be unique",
                    )?;
                }
                let mut pairs = HashSet::new();
                for link in links {
                    optional_label(&link.label, "sankey link label")?;
                    check(
                        link.from != link.to
                            && ids.contains(link.from.as_str())
                            && ids.contains(link.to.as_str())
                            && link.value.is_finite()
                            && link.value > 0.0
                            && pairs.insert((link.from.as_str(), link.to.as_str())),
                        "sankey links require distinct known nodes, unique pairs, and positive finite values",
                    )?;
                }
            }
            WidgetKind::Hierarchy {
                title,
                nodes,
                unit,
                focus,
                ..
            } => {
                bounded_label(title, "hierarchy title")?;
                optional_label(unit, "hierarchy unit")?;
                optional_label(focus, "hierarchy focus")?;
                check(
                    !nodes.is_empty() && nodes.len() <= 128,
                    "hierarchy requires 1-128 nodes",
                )?;
                let mut ids = HashSet::new();
                for node in nodes {
                    bounded_label(&node.id, "hierarchy node id")?;
                    bounded_label(&node.label, "hierarchy node label")?;
                    check(
                        node.value.is_finite() && node.value > 0.0,
                        "hierarchy values must be positive and finite",
                    )?;
                    check(ids.insert(node.id.as_str()), "hierarchy ids must be unique")?;
                }
                check(
                    nodes.iter().any(|node| node.parent.is_none()),
                    "hierarchy requires a root",
                )?;
                for node in nodes {
                    check(
                        node.parent.as_ref().is_none_or(|parent| {
                            parent != &node.id && ids.contains(parent.as_str())
                        }),
                        "hierarchy parents must name a distinct known node",
                    )?;
                    let mut seen = HashSet::new();
                    let mut cursor = node.parent.as_deref();
                    while let Some(parent) = cursor {
                        check(seen.insert(parent), "hierarchy must be acyclic")?;
                        cursor = nodes
                            .iter()
                            .find(|candidate| candidate.id == parent)
                            .and_then(|candidate| candidate.parent.as_deref());
                    }
                }
                check(
                    focus.as_ref().is_none_or(|id| ids.contains(id.as_str())),
                    "hierarchy focus must name a known node",
                )?;
            }
            WidgetKind::MetricBoard {
                title,
                metrics,
                unit,
                ..
            } => {
                bounded_label(title, "metric board title")?;
                optional_label(unit, "metric board unit")?;
                check(
                    !metrics.is_empty() && metrics.len() <= 12,
                    "metric board requires 1-12 metrics",
                )?;
                for metric in metrics {
                    bounded_label(&metric.label, "metric board label")?;
                    check(
                        metric.value.is_finite()
                            && metric.max.is_finite()
                            && metric.max > 0.0
                            && metric.value >= 0.0
                            && metric.value <= metric.max
                            && metric.target.is_none_or(|value| {
                                value.is_finite() && value >= 0.0 && value <= metric.max
                            })
                            && metric.forecast.is_none_or(|value| {
                                value.is_finite() && value >= 0.0 && value <= metric.max
                            }),
                        "metric board values, targets, and forecasts must fit a positive finite scale",
                    )?;
                }
            }
            WidgetKind::Distribution {
                title,
                groups,
                unit,
                ..
            } => {
                bounded_label(title, "distribution title")?;
                optional_label(unit, "distribution unit")?;
                check(
                    !groups.is_empty() && groups.len() <= 12,
                    "distribution requires 1-12 groups",
                )?;
                for group in groups {
                    bounded_label(&group.label, "distribution label")?;
                    check(
                        (5..=1024).contains(&group.samples.len())
                            && group.samples.iter().all(|value| value.is_finite()),
                        "distribution groups require 5-1024 finite samples",
                    )?;
                }
            }
            WidgetKind::RichCard {
                title,
                body,
                language,
                citations,
                ..
            } => {
                bounded_label(title, "rich card title")?;
                optional_label(language, "rich card language")?;
                check(
                    !body.is_empty() && body.len() <= 65_536,
                    "rich card body must contain 1-65536 bytes",
                )?;
                check(citations.len() <= 16, "rich card exceeds 16 citations")?;
                for citation in citations {
                    bounded_label(&citation.label, "citation label")?;
                    check(
                        citation.url.len() <= 2048
                            && (citation.url.starts_with("https://")
                                || citation.url.starts_with("fpl://")),
                        "citation URL must use https or fpl and fit 2048 bytes",
                    )?;
                }
            }
            WidgetKind::ApprovalCard {
                request_id,
                title,
                summary,
                scope,
                expires_at,
                provenance,
                approve_action,
                deny_action,
                ..
            } => {
                for (value, name) in [
                    (request_id, "approval request id"),
                    (title, "approval title"),
                    (expires_at, "approval expiry"),
                    (provenance, "approval provenance"),
                    (approve_action, "approve action"),
                    (deny_action, "deny action"),
                ] {
                    bounded_label(value, name)?;
                }
                check(
                    !summary.is_empty() && summary.len() <= 4096,
                    "approval summary must contain 1-4096 bytes",
                )?;
                validate_labels(scope, "approval scope", 32)?;
                check(
                    approve_action != deny_action,
                    "approval actions must be distinct",
                )?;
            }
            WidgetKind::CommandPalette {
                query,
                results,
                selected,
            } => {
                check(query.len() <= 1024, "command query exceeds 1024 bytes")?;
                check(results.len() <= 64, "command palette exceeds 64 results")?;
                check(
                    results.is_empty() || *selected < results.len(),
                    "command selection is out of bounds",
                )?;
                let mut ids = HashSet::new();
                for result in results {
                    for (value, name) in [
                        (&result.id, "command id"),
                        (&result.label, "command label"),
                        (&result.kind, "command kind"),
                    ] {
                        bounded_label(value, name)?;
                    }
                    optional_label(&result.detail, "command detail")?;
                    optional_label(&result.shortcut, "command shortcut")?;
                    check(
                        ids.insert(result.id.as_str()),
                        "command result ids must be unique",
                    )?;
                }
            }
            WidgetKind::MediaTransport {
                title,
                source,
                codec,
                width,
                height,
                fps,
                bitrate_kbps,
                buffer_ms,
                delay_ms,
                route,
                ..
            } => {
                for (value, name) in [
                    (title, "media transport title"),
                    (source, "media transport source"),
                    (codec, "media transport codec"),
                ] {
                    bounded_label(value, name)?;
                }
                check(
                    *width > 0 && *width <= 16384 && *height > 0 && *height <= 16384,
                    "media transport dimensions must be within 1-16384",
                )?;
                check(
                    fps.is_finite()
                        && *fps >= 0.0
                        && *fps <= 240.0
                        && bitrate_kbps.is_finite()
                        && *bitrate_kbps >= 0.0
                        && buffer_ms.is_finite()
                        && *buffer_ms >= 0.0
                        && delay_ms.is_finite()
                        && *delay_ms >= 0.0,
                    "media transport rates and latency must be finite and non-negative",
                )?;
                validate_labels(route, "media route", 16)?;
            }
            WidgetKind::ModelTelemetry {
                title,
                model,
                annotations,
                exploded,
                ..
            } => {
                bounded_label(title, "model telemetry title")?;
                bounded_label(model, "model telemetry model")?;
                check(
                    exploded.is_finite() && (0.0..=1.0).contains(exploded),
                    "model exploded amount must be within 0-1",
                )?;
                check(
                    annotations.len() <= 64,
                    "model telemetry exceeds 64 annotations",
                )?;
                let mut ids = HashSet::new();
                for annotation in annotations {
                    bounded_label(&annotation.id, "model annotation id")?;
                    bounded_label(&annotation.label, "model annotation label")?;
                    bounded_label(&annotation.value, "model annotation value")?;
                    check(
                        ids.insert(annotation.id.as_str())
                            && annotation
                                .position
                                .iter()
                                .all(|value| value.is_finite() && (-1.0..=1.0).contains(value)),
                        "model annotations require unique ids and normalized positions",
                    )?;
                }
            }
            WidgetKind::SpanTimeline {
                title, spans, unit, ..
            } => {
                bounded_label(title, "span timeline title")?;
                optional_label(unit, "span timeline unit")?;
                check(
                    !spans.is_empty() && spans.len() <= 256,
                    "span timeline requires 1-256 spans",
                )?;
                let mut ids = HashSet::new();
                for span in spans {
                    bounded_label(&span.id, "span id")?;
                    bounded_label(&span.label, "span label")?;
                    optional_label(&span.lane, "span lane")?;
                    optional_label(&span.parent, "span parent")?;
                    optional_label(&span.detail, "span detail")?;
                    check(
                        span.start.is_finite() && span.end.is_finite() && span.start <= span.end,
                        "span times must be finite and ordered",
                    )?;
                    check(ids.insert(span.id.as_str()), "span ids must be unique")?;
                }
                check(
                    spans.iter().all(|span| {
                        span.parent.as_ref().is_none_or(|parent| {
                            parent != &span.id && ids.contains(parent.as_str())
                        })
                    }),
                    "span parents must reference a distinct known span",
                )?;
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
            WidgetKind::RateBars {
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
            WidgetKind::EventLog {
                title,
                entries,
                query,
                ..
            } => {
                bounded_label(title, "event log title")?;
                optional_label(query, "event log query")?;
                check(entries.len() <= 256, "event log exceeds 256 entries")?;
                check(
                    entries.iter().all(|entry| {
                        entry.timestamp.len() <= 64 && entry.message.len() <= MAX_INPUT_TEXT_BYTES
                    }),
                    "event log entry is too large",
                )?;
            }
            WidgetKind::SocialFeed {
                title,
                status,
                detail,
                posts,
                rotate_seconds,
            } => {
                bounded_label(title, "social feed title")?;
                bounded_label(status, "social feed status")?;
                optional_label(detail, "social feed detail")?;
                check(
                    (3..=300).contains(rotate_seconds),
                    "social feed rotation must be 3-300 seconds",
                )?;
                check(posts.len() <= 64, "social feed exceeds 64 posts")?;
                for post in posts {
                    check(
                        !post.id.is_empty() && post.id.len() <= 512,
                        "social post id is invalid",
                    )?;
                    bounded_label(&post.author, "social post author")?;
                    check(
                        post.text.len() <= MAX_INPUT_TEXT_BYTES,
                        "social post text is too large",
                    )?;
                    bounded_label(&post.age, "social post age")?;
                    check(post.matches.len() <= 16, "social post has too many matches")?;
                }
            }
            WidgetKind::StatusList { title, nodes } => {
                bounded_label(title, "node summary title")?;
                check(nodes.len() <= 256, "node summary exceeds 256 nodes")?;
                for node in nodes {
                    bounded_label(&node.label, "node label")?;
                    optional_label(&node.detail, "node detail")?;
                }
            }
            WidgetKind::NeuralGraph {
                title,
                nodes,
                edges,
            } => {
                bounded_label(title, "neural graph title")?;
                check(nodes.len() <= 512, "neural graph exceeds 512 nodes")?;
                check(edges.len() <= 2048, "neural graph exceeds 2048 edges")?;
                let mut ids = std::collections::BTreeSet::new();
                for node in nodes {
                    check(
                        !node.id.is_empty()
                            && node.id.len() <= 128
                            && node.id.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric()
                                    || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
                            })
                            && ids.insert(node.id.as_str()),
                        "neural graph nodes require unique, bounded IDs",
                    )?;
                    bounded_graph_label(&node.label, "neural graph node label")?;
                    optional_bounded_text(&node.detail, "neural graph node detail", 512)?;
                    optional_label(&node.group, "neural graph node group")?;
                    check(
                        node.heat.is_finite() && (0.0..=1.0).contains(&node.heat),
                        "neural graph node heat must be finite within 0..1",
                    )?;
                }
                let mut pairs = std::collections::BTreeSet::new();
                for edge in edges {
                    check(
                        ids.contains(edge.from.as_str())
                            && ids.contains(edge.to.as_str())
                            && edge.from != edge.to
                            && pairs.insert((edge.from.as_str(), edge.to.as_str()))
                            && edge.heat.is_finite()
                            && (0.0..=1.0).contains(&edge.heat)
                            && edge.activity.is_finite()
                            && (0.0..=1024.0).contains(&edge.activity),
                        "neural graph edges require distinct known nodes, unique pairs, heat within 0..1, and bounded activity",
                    )?;
                }
            }
            WidgetKind::NetworkTopology {
                title,
                segments,
                nodes,
                links,
                conflicts,
            } => {
                bounded_label(title, "network topology title")?;
                check(segments.len() <= 64, "network topology exceeds 64 segments")?;
                check(nodes.len() <= 1024, "network topology exceeds 1024 nodes")?;
                check(links.len() <= 4096, "network topology exceeds 4096 links")?;
                check(
                    *conflicts <= 1_000_000,
                    "network topology conflict count is unreasonable",
                )?;
                let mut segment_ids = std::collections::BTreeSet::new();
                for segment in segments {
                    validate_graph_id(&segment.id, "network segment")?;
                    check(
                        segment_ids.insert(segment.id.as_str()),
                        "network segment IDs must be unique",
                    )?;
                    bounded_label(&segment.label, "network segment label")?;
                    optional_label(&segment.subnet, "network segment subnet")?;
                    optional_label(&segment.gateway, "network segment gateway")?;
                    if let Some(vlan) = segment.vlan {
                        check((1..=4094).contains(&vlan), "VLAN id must be 1..=4094")?;
                    }
                }
                let mut node_ids = std::collections::BTreeSet::new();
                for node in nodes {
                    validate_graph_id(&node.id, "network node")?;
                    check(
                        node_ids.insert(node.id.as_str()),
                        "network node IDs must be unique",
                    )?;
                    bounded_label(&node.label, "network node label")?;
                    optional_label(&node.detail, "network node detail")?;
                    check(
                        node.segments
                            .iter()
                            .all(|id| segment_ids.contains(id.as_str())),
                        "network node references an unknown segment",
                    )?;
                }
                let mut pairs = std::collections::BTreeSet::new();
                for link in links {
                    check(
                        node_ids.contains(link.from.as_str())
                            && node_ids.contains(link.to.as_str())
                            && link.from != link.to
                            && pairs.insert((link.from.as_str(), link.to.as_str())),
                        "network links require distinct known nodes and unique pairs",
                    )?;
                    optional_label(&link.label, "network link label")?;
                }
            }
            WidgetKind::DagGraph {
                title,
                nodes,
                edges,
            } => {
                validate_graph(title, nodes, edges, "DAG")?;
                check(is_acyclic(nodes, edges), "DAG graph contains a cycle")?;
            }
            WidgetKind::SpatialTracks {
                title,
                tracks,
                bounds,
            } => {
                bounded_label(title, "spatial tracks title")?;
                check(tracks.len() <= 64, "spatial tracks exceeds 64 tracks")?;
                check(
                    bounds.min_x.is_finite()
                        && bounds.max_x.is_finite()
                        && bounds.min_x < bounds.max_x
                        && bounds.min_z.is_finite()
                        && bounds.max_z.is_finite()
                        && bounds.min_z < bounds.max_z,
                    "spatial tracks requires finite, ordered bounds",
                )?;
                for track in tracks {
                    check(
                        track.confidence.is_finite()
                            && (0.0..=1.0).contains(&track.confidence)
                            && track.motion.is_finite()
                            && (0.0..=100.0).contains(&track.motion)
                            && track.position.x.is_finite()
                            && track.position.z.is_finite()
                            && track.trail.len() <= 64
                            && track
                                .trail
                                .iter()
                                .all(|point| point.x.is_finite() && point.z.is_finite()),
                        "spatial track values must be finite and bounded",
                    )?;
                }
            }
            WidgetKind::Chart { source, .. } => check(
                !source.is_empty() && source.len() <= 128,
                "chart requires a non-empty content-addressed source identifier",
            )?,
            WidgetKind::Text { text } => {
                check(!text.is_empty(), "text widget must not be empty")?;
                check(text.len() <= 4096, "text widget exceeds 4096 bytes")?;
            }
            WidgetKind::Clock { format, .. } => check(
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
            WidgetKind::Launcher { items, .. } => {
                check(
                    items.len() <= MAX_LAUNCHER_ITEMS,
                    "launcher has too many items",
                )?;
                let mut seen_ids = std::collections::BTreeSet::new();
                for item in items {
                    check(
                        !item.project_id.is_empty()
                            && item.project_id.len() <= 64
                            && item
                                .project_id
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                        "launcher item project_id must be a valid identifier",
                    )?;
                    check(
                        seen_ids.insert(item.project_id.as_str()),
                        "launcher has duplicate project_id",
                    )?;
                    check(
                        !item.label.is_empty() && item.label.len() <= 64,
                        "launcher item label must be 1-64 characters",
                    )?;
                }
            }
            WidgetKind::LyricsDisplay {
                title,
                artist,
                lines,
                position_ms,
                duration_ms,
                style: _,
                scroll_offset,
                highlight_intensity,
                ..
            } => {
                check(
                    !title.is_empty() && title.len() <= 128,
                    "lyrics title exceeds 128 chars",
                )?;
                check(
                    !artist.is_empty() && artist.len() <= 128,
                    "lyrics artist exceeds 128 chars",
                )?;
                check(lines.len() <= 1024, "lyrics exceeds 1024 lines")?;
                for line in lines {
                    check(line.text.len() <= 256, "lyric line exceeds 256 chars")?;
                    check(
                        line.start_ms >= 0 && line.end_ms >= line.start_ms,
                        "lyric timestamps invalid",
                    )?;
                    check(line.words.len() <= 64, "lyric line exceeds 64 words")?;
                }
                check(
                    *position_ms >= 0 && *duration_ms > 0,
                    "lyrics duration must be positive",
                )?;
                check(
                    *scroll_offset >= 0.0 && *scroll_offset <= 100.0,
                    "scroll offset out of range",
                )?;
                check(
                    *highlight_intensity >= 0.0 && *highlight_intensity <= 1.0,
                    "highlight intensity out of range",
                )?;
            }
            WidgetKind::CdgViewer {
                title,
                source,
                current_frame,
                total_frames,
                position_ms,
                duration_ms,
                scale,
                ..
            } => {
                check(
                    !title.is_empty() && title.len() <= 128,
                    "CDG title exceeds 128 chars",
                )?;
                check(!source.is_empty(), "CDG source required")?;
                check(source.len() <= 512, "CDG source exceeds 512 chars")?;
                check(
                    *current_frame <= *total_frames,
                    "CDG current frame exceeds total",
                )?;
                check(*total_frames > 0, "CDG total frames must be positive")?;
                check(*position_ms >= 0, "CDG position must be non-negative")?;
                check(*duration_ms > 0, "CDG duration must be positive")?;
                check(*scale > 0.0, "CDG scale must be positive")?;
            }
            WidgetKind::HyperspaceShader {
                shader_id,
                speed,
                scale,
                ..
            } => {
                check(
                    !shader_id.is_empty() && shader_id.len() <= 64,
                    "shader id exceeds 64 chars",
                )?;
                let scale = *scale;
                check(
                    speed.is_finite() && *speed > 0.0,
                    "shader speed must be positive",
                )?;
                check(
                    scale.is_finite() && (0.1..=4.0).contains(&scale),
                    "shader scale must be 0.1-4.0",
                )?;
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

fn bounded_graph_label(value: &str, name: &str) -> Result<()> {
    check(
        value.len() <= 128,
        &format!("{name} must contain at most 128 bytes"),
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

/// How much room a widget kind needs, in logical pixels.
///
/// Two numbers, because one is not enough. `min` is where the content stops
/// being readable — below it a widget should degrade rather than squash.
/// `comfortable` is what a layout should aim for; beyond it most kinds gain
/// nothing, which is what makes it safe to give leftover space to the kinds
/// that do.
///
/// Both are derived from [`ThemeMetrics`] rather than written as constants, so
/// they follow the type scale instead of drifting from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeHint {
    pub min_width: f32,
    pub min_height: f32,
    pub comfortable_width: f32,
    pub comfortable_height: f32,
    /// Width divided by height this kind reads best at, when a layout has slack
    /// to spend. A preference, never a lock: charts deliberately reflow rather
    /// than preserving a ratio.
    pub preferred_aspect: f32,
}

impl SizeHint {
    fn new(
        min_width: f32,
        min_height: f32,
        comfortable_width: f32,
        comfortable_height: f32,
        preferred_aspect: f32,
    ) -> Self {
        Self {
            min_width,
            min_height,
            comfortable_width,
            comfortable_height,
            preferred_aspect,
        }
    }
}

/// The room a widget needs to stay readable.
///
/// Digits are roughly 0.6em wide, so a value rendered at `hero` needs about
/// `hero * 0.6` per character; the widths below assume five or six characters
/// plus a unit, which covers `100.0%`, `23.6 W` and `154173`.
pub fn size_hint(kind: &WidgetKind, metrics: &canvas_core::ThemeMetrics) -> SizeHint {
    let pad = metrics.space_4 * 2.0;
    let gap = metrics.space_2;
    // The chrome every panelled widget carries: a title line and a footer line.
    let chrome = metrics.body + metrics.small + gap * 2.0 + pad;
    // The same token the renderer sets a hero value in, so a hint stays a
    // statement about what will be drawn rather than a guess about it.
    let hero = metrics.hero;
    let digit = 0.6;
    match kind {
        // A number and its label. Gains nothing from height, so comfortable is
        // close to min; extra room here is room a chart could have used.
        WidgetKind::Stat { .. } => SizeHint::new(
            hero * digit * 5.0,
            hero + metrics.small * 2.0 + gap * 2.0 + pad,
            hero * digit * 7.0,
            hero + metrics.small * 2.0 + gap * 3.0 + pad,
            1.6,
        ),
        // Artwork plus two compact metadata rows and a thin progress rail.
        WidgetKind::MediaCard { .. } => SizeHint::new(
            metrics.body * 14.0,
            metrics.body * 5.0 + pad,
            metrics.body * 24.0,
            metrics.body * 7.0 + pad,
            2.4,
        ),
        // A dial reads as a dial only while it is roughly square.
        WidgetKind::Gauge { .. } | WidgetKind::Meter { .. } => {
            let side = hero + metrics.small + pad;
            SizeHint::new(side * 0.8, side * 0.8, side, side, 1.0)
        }
        WidgetKind::Clock { .. } => SizeHint::new(
            hero * digit * 5.0,
            hero + pad,
            hero * digit * 8.0,
            hero + pad,
            2.4,
        ),
        // Below roughly 48px of plot area a line is a smear rather than a shape,
        // and 180 is the point budget a snapshot is thinned to, so a pixel per
        // point is the width at which detail stops being thrown away.
        WidgetKind::TimeSeries { .. }
        | WidgetKind::Histogram { .. }
        | WidgetKind::SpectralWaterfall { .. } => {
            SizeHint::new(180.0, 48.0 + chrome, 320.0, 140.0 + chrome, 2.4)
        }
        // Bars need a label gutter plus a readable bar; rows below `small` * 2
        // stop being separable.
        WidgetKind::RankedBar { .. }
        | WidgetKind::StackedBar { .. }
        | WidgetKind::PieChart { .. } => {
            let row = metrics.small * 2.0;
            SizeHint::new(220.0, row * 3.0 + chrome, 360.0, row * 6.0 + chrome, 2.0)
        }
        WidgetKind::Sankey { .. } => SizeHint::new(300.0, 180.0, 560.0, 300.0, 2.2),
        WidgetKind::Hierarchy { .. } => SizeHint::new(260.0, 200.0, 520.0, 360.0, 1.5),
        WidgetKind::MetricBoard { .. } => SizeHint::new(240.0, 160.0, 480.0, 300.0, 1.8),
        WidgetKind::Distribution { .. } => SizeHint::new(260.0, 170.0, 520.0, 300.0, 1.8),
        WidgetKind::RichCard { .. }
        | WidgetKind::ApprovalCard { .. }
        | WidgetKind::CommandPalette { .. } => SizeHint::new(280.0, 180.0, 520.0, 360.0, 1.5),
        WidgetKind::MediaTransport { .. } | WidgetKind::ModelTelemetry { .. } => {
            SizeHint::new(280.0, 180.0, 540.0, 340.0, 1.8)
        }
        WidgetKind::DawDeck { .. } | WidgetKind::MixField { .. } => {
            SizeHint::new(420.0, 260.0, 760.0, 520.0, 1.2)
        }
        WidgetKind::SpanTimeline { .. } => SizeHint::new(300.0, 160.0, 600.0, 320.0, 2.4),
        // A cell smaller than this is a colour with no shape; four rows is the
        // fewest that reads as a matrix rather than a bar chart.
        WidgetKind::Heatmap { .. } => {
            let cell = metrics.small * 0.7;
            SizeHint::new(
                cell * 12.0 + 60.0,
                cell * 4.0 + chrome,
                cell * 24.0 + 80.0,
                cell * 8.0 + chrome,
                2.0,
            )
        }
        WidgetKind::StateTimeline { .. } | WidgetKind::RateBars { .. } => {
            let lane = metrics.small * 2.2;
            SizeHint::new(240.0, lane * 2.0 + chrome, 420.0, lane * 4.0 + chrome, 3.0)
        }
        // Rows of text: a header and three rows, or the space is better spent
        // elsewhere.
        WidgetKind::Table { .. } | WidgetKind::EventLog { .. } | WidgetKind::StatusList { .. } => {
            let row = metrics.small * 1.6;
            SizeHint::new(240.0, row * 4.0 + chrome, 420.0, row * 8.0 + chrome, 1.8)
        }
        WidgetKind::SocialFeed { .. } => {
            SizeHint::new(320.0, 260.0 + chrome, 720.0, 620.0 + chrome, 1.4)
        }
        WidgetKind::NeuralGraph { .. }
        | WidgetKind::NetworkTopology { .. }
        | WidgetKind::DagGraph { .. }
        | WidgetKind::SpatialTracks { .. } => SizeHint::new(300.0, 180.0, 640.0, 380.0, 1.7),
        WidgetKind::Ticker { .. } => SizeHint::new(
            200.0,
            metrics.large * 3.0 + pad,
            480.0,
            metrics.large * 3.5 + pad,
            4.0,
        ),
        WidgetKind::Status { .. } => {
            let side = metrics.body * 3.0 + pad;
            SizeHint::new(side, side * 0.7, side * 1.6, side * 0.8, 1.6)
        }
        WidgetKind::Text { .. } => SizeHint::new(
            metrics.body * 12.0,
            metrics.body * 3.0 + pad,
            metrics.body * 24.0,
            metrics.body * 6.0 + pad,
            1.6,
        ),
        // Unknown extents: a chart's source and a module's renderer both decide
        // their own content, so give them a generous, chart-shaped default
        // rather than pretending to know.
        WidgetKind::Chart { .. } | WidgetKind::Quant { .. } | WidgetKind::Module { .. } => {
            SizeHint::new(220.0, 140.0, 420.0, 260.0, 1.6)
        }
        WidgetKind::Launcher { .. } => SizeHint::new(220.0, 40.0, 600.0, 60.0, 1.0),
        WidgetKind::LyricsDisplay { .. } => SizeHint::new(400.0, 300.0, 800.0, 600.0, 2.0),
        // Live preview surfaces (shaders and CD+G viewers) render their own
        // content, so give them a generous, chart-shaped default.
        WidgetKind::HyperspaceShader { .. } | WidgetKind::CdgViewer { .. } => {
            SizeHint::new(300.0, 180.0, 560.0, 360.0, 2.0)
        }
    }
}

#[cfg(test)]
mod tests {

    fn metrics() -> canvas_core::ThemeMetrics {
        canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .visual_theme()
        .metrics
    }

    fn every_kind() -> Vec<WidgetKind> {
        vec![
            WidgetKind::Stat {
                label: "l".into(),
                value: 1.0,
                unit: None,
                change: None,
                state: None,
                history: Vec::new(),
            },
            WidgetKind::Gauge {
                value: 1.0,
                label: None,
            },
            WidgetKind::Meter {
                value: 0.5,
                style: MeterStyle::Led,
            },
            WidgetKind::Clock {
                format: "%H".into(),
                mode: ClockMode::Digital,
            },
            WidgetKind::TimeSeries {
                title: "t".into(),
                series: Vec::new(),
                unit: None,
            },
            WidgetKind::Histogram {
                title: "t".into(),
                buckets: Vec::new(),
                unit: None,
            },
            WidgetKind::SpectralWaterfall {
                title: "t".into(),
                rows: vec![vec![0.0; 8]],
                min_frequency: 88.0,
                max_frequency: 108.0,
                unit: Some("MHz".into()),
                history: 96,
                radio: None,
            },
            WidgetKind::RankedBar {
                title: "t".into(),
                bars: Vec::new(),
                unit: None,
            },
            WidgetKind::StackedBar {
                title: "t".into(),
                series: Vec::new(),
                bars: Vec::new(),
                unit: None,
            },
            WidgetKind::PieChart {
                title: "t".into(),
                values: vec![BarValue {
                    label: "A".into(),
                    value: 1.0,
                    state: None,
                }],
                unit: None,
                period: None,
            },
            WidgetKind::Sankey {
                title: "t".into(),
                nodes: vec![
                    FlowNode {
                        id: "a".into(),
                        label: "A".into(),
                        state: None,
                    },
                    FlowNode {
                        id: "b".into(),
                        label: "B".into(),
                        state: None,
                    },
                ],
                links: vec![FlowLink {
                    from: "a".into(),
                    to: "b".into(),
                    value: 1.0,
                    label: None,
                }],
                unit: None,
            },
            WidgetKind::Hierarchy {
                title: "t".into(),
                style: HierarchyStyle::Treemap,
                nodes: vec![HierarchyNode {
                    id: "root".into(),
                    label: "Root".into(),
                    parent: None,
                    value: 1.0,
                }],
                unit: None,
                focus: None,
            },
            WidgetKind::MetricBoard {
                title: "t".into(),
                style: MetricBoardStyle::Radial,
                metrics: vec![TargetMetric {
                    label: "CPU".into(),
                    value: 42.0,
                    max: 100.0,
                    target: Some(80.0),
                    forecast: None,
                    state: None,
                }],
                unit: Some("%".into()),
            },
            WidgetKind::Distribution {
                title: "t".into(),
                style: DistributionStyle::Box,
                groups: vec![DistributionGroup {
                    label: "api".into(),
                    samples: vec![1.0, 2.0, 3.0, 4.0, 5.0],
                }],
                unit: Some("ms".into()),
            },
            WidgetKind::RichCard {
                title: "t".into(),
                format: RichCardFormat::Markdown,
                body: "body".into(),
                language: None,
                citations: Vec::new(),
            },
            WidgetKind::ApprovalCard {
                request_id: "r".into(),
                title: "t".into(),
                summary: "summary".into(),
                scope: vec!["read".into()],
                expires_at: "soon".into(),
                provenance: "agent".into(),
                approve_action: "approve".into(),
                deny_action: "deny".into(),
                status: ApprovalStatus::Pending,
            },
            WidgetKind::CommandPalette {
                query: String::new(),
                results: Vec::new(),
                selected: 0,
            },
            WidgetKind::MediaTransport {
                title: "Stream".into(),
                source: "studio".into(),
                codec: "h264".into(),
                width: 1280,
                height: 720,
                fps: 24.0,
                bitrate_kbps: 2500.0,
                buffer_ms: 80.0,
                delay_ms: 120.0,
                route: vec!["studio".into(), "home-pi".into()],
                state: Some(StatusLevel::Good),
            },
            WidgetKind::ModelTelemetry {
                title: "Model".into(),
                model: "benchy.glb".into(),
                view: ModelView::Isometric,
                annotations: Vec::new(),
                exploded: 0.0,
            },
            WidgetKind::SpanTimeline {
                title: "t".into(),
                style: SpanTimelineStyle::Waterfall,
                spans: vec![TimedSpan {
                    id: "root".into(),
                    label: "Root".into(),
                    start: 0.0,
                    end: 1.0,
                    lane: None,
                    parent: None,
                    state: None,
                    detail: None,
                }],
                unit: Some("ms".into()),
            },
            WidgetKind::Heatmap {
                title: "t".into(),
                rows: Vec::new(),
                columns: Vec::new(),
                values: Vec::new(),
                style: HeatmapStyle::Matrix,
                unit: None,
            },
            WidgetKind::StateTimeline {
                title: "t".into(),
                lanes: Vec::new(),
            },
            WidgetKind::Table {
                title: "t".into(),
                columns: Vec::new(),
                rows: Vec::new(),
            },
            WidgetKind::EventLog {
                title: "t".into(),
                entries: Vec::new(),
                query: None,
                paused: false,
                follow: true,
            },
            WidgetKind::SocialFeed {
                title: "Social".into(),
                status: "healthy".into(),
                detail: None,
                posts: vec![SocialFeedPost {
                    id: "at://did:plc:test/app.bsky.feed.post/one".into(),
                    author: "test.example".into(),
                    text: "A real post".into(),
                    age: "now".into(),
                    engagement: 3,
                    matches: vec!["topic".into()],
                    url: None,
                }],
                rotate_seconds: 8,
            },
            WidgetKind::NeuralGraph {
                title: "Neural activity".into(),
                nodes: vec![NeuralNode {
                    id: "gateway".into(),
                    label: "Gateway".into(),
                    detail: None,
                    group: Some("control".into()),
                    heat: 1.0,
                    state: StatusLevel::Good,
                }],
                edges: Vec::new(),
            },
            WidgetKind::DagGraph {
                title: "Agent lineage".into(),
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            WidgetKind::Ticker {
                entries: vec![BarValue {
                    label: "A".into(),
                    value: 1.0,
                    state: None,
                }],
            },
            WidgetKind::Status { state: None },
            WidgetKind::Text { text: "x".into() },
        ]
    }

    #[test]
    fn neural_graph_rejects_unknown_edges_and_invalid_heat() {
        let valid = br#"{
          "schema_version":2,"id":"brain","kind":"neural_graph","title":"Brain",
          "nodes":[
            {"id":"gateway","label":"Gateway","group":"control","heat":1.0,"state":"good"},
            {"id":"tool","label":"Tool","group":"effect","heat":0.5}
          ],
          "edges":[{"from":"gateway","to":"tool","heat":0.75}]
        }"#;
        assert!(WidgetSpec::from_json(valid).is_ok());

        let unknown = String::from_utf8(valid.to_vec())
            .unwrap()
            .replace("\"to\":\"tool\"", "\"to\":\"missing\"");
        assert!(WidgetSpec::from_json(unknown.as_bytes()).is_err());

        let invalid_heat = String::from_utf8(valid.to_vec())
            .unwrap()
            .replace("\"heat\":0.75", "\"heat\":1.25");
        assert!(WidgetSpec::from_json(invalid_heat.as_bytes()).is_err());
    }

    #[test]
    fn dag_graph_rejects_cycles() {
        let cyclic = br#"{
          "schema_version":2,"id":"lineage","kind":"dag_graph","title":"Agents",
          "nodes":[
            {"id":"root","label":"Root","heat":1.0},
            {"id":"child","label":"Child","heat":0.5}
          ],
          "edges":[
            {"from":"root","to":"child","heat":1.0},
            {"from":"child","to":"root","heat":0.5}
          ]
        }"#;
        assert!(WidgetSpec::from_json(cyclic).is_err());
    }

    #[test]
    fn neural_graph_sample_is_a_valid_widget() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/widgets/mcp-neural-graph.json");
        WidgetSpec::from_file(&path).expect("neural graph sample must validate");
    }

    #[test]
    fn comfortable_is_never_smaller_than_min() {
        let m = metrics();
        for kind in every_kind() {
            let h = size_hint(&kind, &m);
            assert!(h.comfortable_width >= h.min_width, "{kind:?} width");
            assert!(h.comfortable_height >= h.min_height, "{kind:?} height");
        }
    }

    #[test]
    fn every_minimum_leaves_room_for_the_content_it_frames() {
        // A minimum smaller than the panel's own padding plus one line of text
        // is a minimum that guarantees an unreadable widget.
        let m = metrics();
        let floor = m.space_4 * 2.0 + m.small;
        for kind in every_kind() {
            let h = size_hint(&kind, &m);
            assert!(
                h.min_height > floor,
                "{kind:?} min_height {} <= {floor}",
                h.min_height
            );
            assert!(
                h.min_width > floor,
                "{kind:?} min_width {} <= {floor}",
                h.min_width
            );
        }
    }

    #[test]
    fn minimums_follow_the_type_scale_rather_than_being_pasted_constants() {
        // Doubling the type scale must grow the value-led kinds. If it does not,
        // someone replaced a derivation with a literal and the hint will drift
        // from what is actually rendered.
        let mut big = metrics();
        big.micro *= 2.0;
        big.small *= 2.0;
        big.body *= 2.0;
        big.display *= 2.0;
        big.hero *= 2.0;
        for kind in [
            WidgetKind::Stat {
                label: "l".into(),
                value: 1.0,
                unit: None,
                change: None,
                state: None,
                history: Vec::new(),
            },
            WidgetKind::Gauge {
                value: 1.0,
                label: None,
            },
            WidgetKind::Clock {
                format: "%H".into(),
                mode: ClockMode::Digital,
            },
        ] {
            let small = size_hint(&kind, &metrics());
            let large = size_hint(&kind, &big);
            assert!(
                large.min_height > small.min_height,
                "{kind:?} ignored the type scale"
            );
        }
    }

    #[test]
    fn every_value_led_kind_sizes_from_the_one_hero_token() {
        // Stat, Gauge and Clock all render their value at `metrics.hero`. If a
        // hint still derives its own multiple of `display`, moving `hero` alone
        // leaves it behind and the hint stops describing what is drawn — which
        // is how 42, 47.6 and 63 pixels coexisted in one library.
        let base = metrics();
        let mut taller = base;
        taller.hero *= 2.0;
        for kind in [
            WidgetKind::Stat {
                label: "l".into(),
                value: 1.0,
                unit: None,
                change: None,
                state: None,
                history: Vec::new(),
            },
            WidgetKind::Gauge {
                value: 1.0,
                label: None,
            },
            WidgetKind::Clock {
                format: "%H".into(),
                mode: ClockMode::Digital,
            },
        ] {
            assert!(
                size_hint(&kind, &taller).min_height > size_hint(&kind, &base).min_height,
                "{kind:?} does not follow the hero token"
            );
        }
    }

    #[test]
    fn a_dial_stays_square_and_a_series_stays_wide() {
        // Aspect is a preference a layout spends slack on, so the ordering
        // between kinds is what matters, not the exact ratios.
        let m = metrics();
        let gauge = size_hint(
            &WidgetKind::Gauge {
                value: 1.0,
                label: None,
            },
            &m,
        );
        let series = size_hint(
            &WidgetKind::TimeSeries {
                title: "t".into(),
                series: Vec::new(),
                unit: None,
            },
            &m,
        );
        assert!(
            (gauge.preferred_aspect - 1.0).abs() < 0.2,
            "a dial should be roughly square"
        );
        assert!(
            series.preferred_aspect > gauge.preferred_aspect,
            "a series wants to be wider than a dial"
        );
        assert!(
            series.min_width > series.min_height,
            "a series minimum should be landscape"
        );
    }
    use super::*;

    #[test]
    fn spec_roundtrips_through_json() {
        let json = r#"{
            "schema_version": 2,
            "id": "demo-ticker",
            "kind": "ticker",
            "entries": [
                {"label": "AAPL", "value": 187.50, "state": "good"},
                {"label": "GOOG", "value": 152.30, "state": "good"}
            ],
            "refresh_ms": 30000
        }"#;
        let spec = WidgetSpec::from_json(json.as_bytes()).expect("valid spec");
        assert_eq!(spec.id, "demo-ticker");
        assert_eq!(
            spec.kind,
            WidgetKind::Ticker {
                entries: vec![
                    BarValue {
                        label: "AAPL".into(),
                        value: 187.50,
                        state: Some(StatusLevel::Good),
                    },
                    BarValue {
                        label: "GOOG".into(),
                        value: 152.30,
                        state: Some(StatusLevel::Good),
                    },
                ]
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
            "source": "sha256:abcdef0123456789",
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
    fn operational_inputs_are_bounded_and_chrome_defaults_by_kind() {
        let stat = WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":42,"history":[1,2,3]}"#,
        )
        .unwrap();
        // Auto preserves controls for ordinary widgets.
        assert_eq!(stat.chrome, WidgetChrome::Auto);
        assert_eq!(stat.effective_chrome(), WidgetChrome::Standard);
        let clock = WidgetSpec::from_json(
            br#"{"id":"clock","kind":"clock","format":"%H:%M:%S","mode":"analog"}"#,
        )
        .unwrap();
        assert_eq!(clock.chrome, WidgetChrome::Auto);
        assert_eq!(clock.effective_chrome(), WidgetChrome::Frameless);
        let framed_clock = WidgetSpec::from_json(
            br#"{"id":"clock","kind":"clock","format":"%H:%M:%S","mode":"analog","chrome":"standard"}"#,
        )
        .unwrap();
        assert_eq!(framed_clock.effective_chrome(), WidgetChrome::Standard);
        let quiet = WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":42,"chrome":"drag_only"}"#,
        )
        .unwrap();
        assert_eq!(quiet.chrome, WidgetChrome::DragOnly);

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
            r#"{"id":"activity","kind":"heatmap","style":"activity","title":"Contributions","rows":["Mon","Tue"],"columns":["W1","W2"],"values":[[0,3],[1,8]],"unit":"events"}"#,
            r#"{"id":"rank","kind":"ranked_bar","title":"Tokens","bars":[{"label":"agent-a","value":42}]}"#,
            r#"{"id":"stack","kind":"stacked_bar","title":"Outcomes","series":["ok","error"],"bars":[{"label":"run","values":[9,1]}]}"#,
            r#"{"id":"flow","kind":"sankey","title":"Requests","nodes":[{"id":"edge","label":"Edge"},{"id":"api","label":"API"}],"links":[{"from":"edge","to":"api","value":42}],"unit":"req/s"}"#,
            r#"{"id":"tree","kind":"hierarchy","title":"Deps","style":"treemap","nodes":[{"id":"root","label":"Root","value":10},{"id":"leaf","label":"Leaf","parent":"root","value":10}]}"#,
            r#"{"id":"targets","kind":"metric_board","title":"Targets","style":"bullet","metrics":[{"label":"CPU","value":67,"max":100,"target":80}]}"#,
            r#"{"id":"dist","kind":"distribution","title":"Latency","style":"box","groups":[{"label":"api","samples":[1,2,3,4,5]}]}"#,
            r#"{"id":"states","kind":"state_timeline","title":"Health","lanes":[{"label":"api","segments":[{"start":0,"end":10,"state":"good"}]}]}"#,
            r#"{"id":"burn","kind":"rate_bars","title":"Budget","objective":99.9,"windows":[{"label":"5m","burn_rate":0.7}]}"#,
        ] {
            WidgetSpec::from_json(json.as_bytes())
                .unwrap_or_else(|error| panic!("{json}: {error}"));
        }
    }

    #[test]
    fn malformed_operational_charts_fail_loudly() {
        for json in [
            r#"{"id":"heat","kind":"heatmap","title":"Latency","rows":["api"],"columns":["p50"],"values":[[1,2]]}"#,
            r#"{"id":"activity","kind":"heatmap","style":"activity","title":"Activity","rows":["1","2","3","4","5","6","7","8"],"columns":["W1"],"values":[[1],[1],[1],[1],[1],[1],[1],[1]]}"#,
            r#"{"id":"stack","kind":"stacked_bar","title":"Outcomes","series":["ok"],"bars":[{"label":"run","values":[1,2]}]}"#,
            r#"{"id":"flow","kind":"sankey","title":"Bad","nodes":[{"id":"a","label":"A"}],"links":[{"from":"a","to":"missing","value":1}]}"#,
            r#"{"id":"tree","kind":"hierarchy","title":"Cycle","style":"sunburst","nodes":[{"id":"root","label":"Root","value":1},{"id":"a","label":"A","parent":"b","value":1},{"id":"b","label":"B","parent":"a","value":1}]}"#,
            r#"{"id":"targets","kind":"metric_board","title":"Bad","style":"radial","metrics":[{"label":"CPU","value":120,"max":100}]}"#,
            r#"{"id":"dist","kind":"distribution","title":"Bad","style":"violin","groups":[{"label":"api","samples":[1,2,3,4]}]}"#,
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
        WidgetSpec::from_json(format!(r#"{{"schema_version":2,"id":"w",{inner}"#).as_bytes())
    }

    #[test]
    fn a_v1_slo_burn_rate_kind_is_migrated_to_rate_bars() {
        let json = br#"{"schema_version":2,"id":"budget","kind":"slo_burn_rate","title":"Budget","objective":99.9,"windows":[{"label":"5m","burn_rate":0.7}]}"#;
        let spec = WidgetSpec::from_json(json).expect("migrated spec");
        assert!(matches!(spec.kind, WidgetKind::RateBars { .. }));
        assert_eq!(spec.schema_version, 2);
    }

    #[test]
    fn a_v1_node_summary_kind_is_migrated_to_status_list() {
        let json = br#"{"schema_version":2,"id":"health","kind":"node_summary","title":"Health","nodes":[]}"#;
        let spec = WidgetSpec::from_json(json).expect("migrated spec");
        assert!(matches!(spec.kind, WidgetKind::StatusList { .. }));
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
            r#"{"kind":"ticker","entries":[{"label":"AAPL","value":187.50}]}"#,
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
