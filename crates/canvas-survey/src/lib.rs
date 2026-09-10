//! Turns a Prometheus metadata survey into candidate widget definitions.
//!
//! Everything here is a pure transform over facts a caller has already
//! collected, so the heuristic is testable without a Prometheus, a credential,
//! or a socket. The binary supplies the facts; this decides what they should
//! draw.
//!
//! Two of the three shapes on the wire are deliberately not defined here. A
//! candidate emits a [`canvas_widgets::WidgetSpec`] (`config/widgets/*.json`)
//! and a [`canvas_metrics::SourceSpec`] (`config/metrics.json`) because those
//! crates already own those contracts: a candidate a reviewer accepts is a file
//! copy rather than a translation, and a candidate the renderer could not load
//! is a compile error here instead of a blank panel later. What this crate does
//! define — [`MetricFact`] in, [`Candidate`], [`Rationale`] and [`Reported`]
//! out — is the survey contract itself, and it is designed to be read by the
//! person correcting a wrong guess.
//!
//! Three properties are load-bearing, and each is tested:
//!
//! * A raw counter is a meaningless ramp, so a counter becomes a rate and never
//!   anything else. This single rule is most of what separates a useful
//!   generator from a naive one.
//! * Metrics that belong together arrive as one widget. A used/total pair is a
//!   utilization gauge, not two stats; a `_bucket`/`_sum`/`_count` trio is one
//!   latency widget, not three.
//! * A metric no rule claims is reported with a reason. It is never dropped and
//!   never defaulted to a stat, per `docs/widget-quality.md`: silent omission is
//!   how a dashboard quietly lacks the metric that mattered.
use canvas_metrics::{Mapping, SourceSpec};
use canvas_widgets::{
    Provenance, WIDGET_SCHEMA_VERSION, WidgetData, WidgetDataState, WidgetKind as WidgetBody,
    WidgetSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const SURVEY_SCHEMA_VERSION: u32 = 1;

/// Caps mirroring what the connector actually renders: `canvas-metrics` keeps
/// 12 bars, 12 heatmap rows, and 8 time series. A candidate that would exceed
/// one of these is written as an explicit `topk` with the tail disclosed in its
/// own title, because the alternative is the connector silently dropping series
/// nobody was told about.
pub const RANKED_MAX_SERIES: usize = 12;
pub const HEATMAP_MAX_ROWS: usize = 12;
pub const TIME_SERIES_MAX: usize = 8;

pub const DEFAULT_WINDOW_SECONDS: u64 = 1800;
pub const DEFAULT_STEP_SECONDS: u64 = 30;
pub const DEFAULT_RATE_WINDOW: &str = "5m";
pub const DEFAULT_POLL_SECONDS: u64 = 30;

/// The quantile a latency histogram is reduced to. p95 rather than p99 because
/// p99 on a small scrape is mostly noise, and rather than a mean because a mean
/// latency hides exactly the tail the widget exists to show.
pub const LATENCY_QUANTILE: f64 = 0.95;

/// Longest HELP string accepted as a widget title. Beyond this a help text is a
/// sentence, not a label, and reads badly on a wall.
const MAX_TITLE_FROM_HELP: usize = 60;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// What Prometheus says about one metric name.
///
/// `series` and `labels` are cardinality facts a caller reads from the series
/// index; `metric_type`, `help` and `unit` come from the metadata endpoint. The
/// heuristic needs all three groups: type alone cannot tell a single-value stat
/// from a twelve-node bar chart.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricFact {
    pub name: String,
    #[serde(rename = "type")]
    pub metric_type: MetricType,
    #[serde(default)]
    pub help: Option<String>,
    /// Prometheus metadata `unit`. Usually empty in practice, but when a scrape
    /// does declare one it beats anything inferred from the name.
    #[serde(default)]
    pub unit: Option<String>,
    /// Distinct series carried by this metric name right now.
    pub series: usize,
    /// Label names that vary across those series, most distinguishing first.
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
    Untyped,
}

impl MetricType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
            Self::Summary => "summary",
            Self::Untyped => "untyped",
        }
    }
}

/// What the dashboard being generated is for.
///
/// Several rules are genuinely ambiguous without this: a gauge with four series
/// is a ranked bar on a status board and a time series on a trend wall. That
/// intent belongs to the dashboard, not to the metric, so it is asked for once
/// rather than guessed per metric — and the answer is recorded in the survey.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyOptions {
    /// `Some` for a trend wall, `None` for an instant status board.
    #[serde(default)]
    pub window_seconds: Option<u64>,
    #[serde(default = "default_step")]
    pub step_seconds: u64,
    /// Range every `rate()` is taken over. Wide enough to survive a missed
    /// scrape at the default 30s interval.
    #[serde(default = "default_rate_window")]
    pub rate_window: String,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
}

fn default_step() -> u64 {
    DEFAULT_STEP_SECONDS
}
fn default_rate_window() -> String {
    DEFAULT_RATE_WINDOW.to_owned()
}
fn default_poll() -> u64 {
    DEFAULT_POLL_SECONDS
}

impl Default for SurveyOptions {
    fn default() -> Self {
        Self {
            window_seconds: None,
            step_seconds: default_step(),
            rate_window: default_rate_window(),
            poll_seconds: default_poll(),
        }
    }
}

impl SurveyOptions {
    /// A trend wall over `seconds`.
    pub fn windowed(seconds: u64) -> Self {
        Self {
            window_seconds: Some(seconds),
            ..Self::default()
        }
    }

    fn validate(&self) -> Result<(), String> {
        let window = self.rate_window.as_bytes();
        let (digits, suffix) = window.split_at(window.len().saturating_sub(1));
        if digits.is_empty()
            || !digits.iter().all(u8::is_ascii_digit)
            || !matches!(suffix, [b's' | b'm' | b'h' | b'd' | b'w' | b'y'])
        {
            return Err(format!(
                "rate_window must be a PromQL duration such as 5m: {}",
                self.rate_window
            ));
        }
        if self.step_seconds == 0 {
            return Err("step_seconds must be non-zero".into());
        }
        Ok(())
    }

    /// The window a range-query candidate is given. A rate needs a range even
    /// on a status board, so an instant survey still gets the default width.
    fn range(&self) -> (u64, u64) {
        (
            self.window_seconds.unwrap_or(DEFAULT_WINDOW_SECONDS),
            self.step_seconds,
        )
    }

    fn windowed_dashboard(&self) -> bool {
        self.window_seconds.is_some()
    }
}

// ---------------------------------------------------------------------------
// Output contract
// ---------------------------------------------------------------------------

/// The widget kinds the heuristic is allowed to propose.
///
/// Closed on purpose: the set of shapes a generator may reach for is a decision
/// worth making once, and a closed enum means a new shape arrives with the rule
/// that justifies it. Every tag here is checked against
/// [`canvas_widgets::KNOWN_WIDGET_KINDS`] by a test, so the heuristic cannot
/// propose something this build cannot draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    Stat,
    Gauge,
    TimeSeries,
    RankedBar,
    Heatmap,
    Histogram,
    NodeSummary,
}

impl WidgetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stat => "stat",
            Self::Gauge => "gauge",
            Self::TimeSeries => "time_series",
            Self::RankedBar => "ranked_bar",
            Self::Heatmap => "heatmap",
            Self::Histogram => "histogram",
            Self::NodeSummary => "node_summary",
        }
    }

    /// Whether the connector must issue a range query to fill this kind.
    ///
    /// Derived from the kind rather than carried alongside it, because the two
    /// disagreeing is silent: a `stat` given a window gets a matrix back and
    /// reads no value out of it. Note that this is not the same question as
    /// whether the query mentions a range — `sum(rate(x[5m]))` is a perfectly
    /// good instant query, and an error-rate gauge wants exactly that.
    fn range_query(self) -> bool {
        matches!(self, Self::TimeSeries | Self::Heatmap)
    }

    /// An empty body of this kind. Empty rather than zeroed-with-meaning: the
    /// accompanying [`WidgetData`] is `NoData` with a reason, so nothing here
    /// reads as a claim about the world before the connector has spoken.
    fn body(self, text: String, unit: Option<String>) -> WidgetBody {
        match self {
            Self::Stat => WidgetBody::Stat {
                label: text,
                value: 0.0,
                unit,
                change: None,
                state: None,
                history: Vec::new(),
            },
            Self::Gauge => WidgetBody::Gauge {
                value: 0.0,
                label: Some(text),
            },
            Self::TimeSeries => WidgetBody::TimeSeries {
                title: text,
                series: Vec::new(),
                unit,
            },
            Self::RankedBar => WidgetBody::RankedBar {
                title: text,
                bars: Vec::new(),
                unit,
            },
            Self::Heatmap => WidgetBody::Heatmap {
                title: text,
                rows: Vec::new(),
                columns: Vec::new(),
                values: Vec::new(),
                unit,
            },
            Self::Histogram => WidgetBody::Histogram {
                title: text,
                buckets: Vec::new(),
                unit,
            },
            Self::NodeSummary => WidgetBody::NodeSummary {
                title: text,
                nodes: Vec::new(),
            },
        }
    }
}

/// Which rule fired. The reviewer's first question about a wrong guess is
/// "why did it think that", so the answer is a value, not a comment in this
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    CounterRate,
    ErrorRate,
    Utilization,
    LatencyQuantile,
    BucketHeatmap,
    SummaryAverage,
    PercentGauge,
    SingleSeriesStat,
    RankedGauge,
    WindowedGauge,
    WideGaugeHeatmap,
    TopNGauge,
    UpStatusList,
}

impl Rule {
    /// The rule in one line, carried in the output so the JSON stands alone.
    pub fn why(self) -> &'static str {
        match self {
            Self::CounterRate => {
                "counter, any cardinality: a raw counter is a meaningless ramp, so it is a rate"
            }
            Self::ErrorRate => {
                "an errors counter beside its total: one error rate, not two counters"
            }
            Self::Utilization => {
                "a used/total pair in the same unit: one utilization gauge, not two stats"
            }
            Self::LatencyQuantile => {
                "histogram buckets on a latency metric: one quantile, not a bucket per widget"
            }
            Self::BucketHeatmap => "histogram buckets on a non-latency metric: a distribution",
            Self::SummaryAverage => "a _sum beside its _count with no buckets: an average",
            Self::PercentGauge => "a value already bounded 0..1 or 0..100: a gauge",
            Self::SingleSeriesStat => "gauge with one series: a single number",
            Self::RankedGauge => "gauge with 2-12 series on a status board: ranked bars, instant",
            Self::WindowedGauge => "gauge with 2-12 series on a trend wall: one line per series",
            Self::WideGaugeHeatmap => "gauge with more than 12 series on a trend wall: a heatmap",
            Self::TopNGauge => {
                "gauge with more than 12 series on a status board: top N, tail disclosed"
            }
            Self::UpStatusList => {
                "an up-ness signal: a status list, because summing it hides which target is down"
            }
        }
    }
}

/// How the metrics behind a candidate relate to each other. `Singleton` is the
/// honest majority case; the rest are what makes the survey smarter than a
/// per-metric switch statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Singleton,
    Utilization,
    ErrorRate,
    Histogram,
    Summary,
}

/// What the rule fired on. Everything here is an observed fact, so a reviewer
/// can check the decision without re-deriving it from Prometheus.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub metric_type: MetricType,
    /// Cardinality of the family's primary metric.
    pub series: usize,
    pub family: Family,
    /// Every metric name this candidate consumed, primary first.
    pub members: Vec<String>,
    pub labels: Vec<String>,
}

/// Why this candidate looks the way it does, and what to change if it is wrong.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rationale {
    pub rule: Rule,
    /// [`Rule::why`], inlined so the emitted JSON is readable on its own.
    pub why: String,
    pub evidence: Evidence,
    /// Kinds that were also defensible here. A reviewer correcting a guess
    /// changes `kind` in both files to one of these; nothing else moves.
    pub alternatives: Vec<WidgetKind>,
    /// Things the heuristic knows it could not settle: an unguessable unit, an
    /// aggregation that hides detail, a threshold that is policy rather than
    /// metadata. Empty is the good case.
    pub caveats: Vec<String>,
}

/// One proposed widget: the two files to write, plus why.
///
/// No `PartialEq`: [`canvas_metrics::SourceSpec`] does not derive one, and
/// declaring a second mapping type here to gain it would put the connector's
/// contract in two places.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The `config/metrics.json` key, `owner/name`, which is also how Canvas
    /// routes the snapshot stream.
    pub source_id: String,
    /// Panel group, derived from the longest common prefix of its neighbours.
    pub group: String,
    /// The `config/widgets/<id>.json` file.
    pub widget: WidgetSpec,
    /// The `config/metrics.json` entry under `source_id`.
    pub source: SourceSpec,
    pub rationale: Rationale,
}

impl Candidate {
    /// Filename this candidate's widget spec belongs in, under `config/widgets/`.
    pub fn widget_filename(&self) -> String {
        format!("{}.json", self.widget.id)
    }

    pub fn kind(&self) -> &str {
        &self.source.kind
    }
}

/// Why a metric produced no candidate. Both dispositions are reported: one is a
/// decision, the other is an admission, and collapsing them would hide the
/// admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Deliberately not a widget: an `_info` metric is label enrichment for
    /// other metrics, and drawing its constant 1 says nothing.
    LabelEnrichment,
    /// No rule claimed it. This is the case that must never be silently
    /// swallowed or defaulted into a stat.
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reported {
    pub metric: String,
    pub metric_type: MetricType,
    pub series: usize,
    pub disposition: Disposition,
    pub reason: String,
}

/// The whole result. `options` travels with it because the same metrics survey
/// to different widgets on a status board than on a trend wall, and a candidate
/// read six months later should say which it was.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Survey {
    pub schema_version: u32,
    pub options: SurveyOptions,
    pub candidates: Vec<Candidate>,
    pub reported: Vec<Reported>,
}

impl Survey {
    /// Panel groups: group id to the source ids inside it. Derived on demand
    /// rather than stored, so it cannot disagree with `candidates`.
    pub fn groups(&self) -> BTreeMap<String, Vec<String>> {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for candidate in &self.candidates {
            groups
                .entry(candidate.group.clone())
                .or_default()
                .push(candidate.source_id.clone());
        }
        groups
    }

    /// The candidates as a `config/metrics.json` document.
    pub fn mapping(&self) -> Mapping {
        Mapping {
            schema_version: 1,
            poll_seconds: self.options.poll_seconds,
            sources: self
                .candidates
                .iter()
                .map(|candidate| (candidate.source_id.clone(), candidate.source.clone()))
                .collect(),
        }
    }

    /// The same document with absent optional fields removed, matching how the
    /// hand-written `config/metrics.json` reads. `SourceSpec` defaults every
    /// option, so this round-trips.
    pub fn mapping_json(&self) -> Value {
        prune_nulls(serde_json::to_value(self.mapping()).unwrap_or(Value::Null))
    }

    /// Filename to widget spec, for everything under `config/widgets/`.
    pub fn widget_files(&self) -> BTreeMap<String, Value> {
        self.candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.widget_filename(),
                    prune_nulls(serde_json::to_value(&candidate.widget).unwrap_or(Value::Null)),
                )
            })
            .collect()
    }

    /// A reviewer-facing summary: one line per candidate with the rule and the
    /// evidence, then everything the heuristic refused to guess at.
    pub fn report(&self) -> String {
        let mut out = String::new();
        let groups = self.groups();
        out.push_str(&format!(
            "{} candidates in {} groups, {} reported ({})\n",
            self.candidates.len(),
            groups.len(),
            self.reported.len(),
            match self.options.window_seconds {
                Some(seconds) => format!("trend wall, {seconds}s window"),
                None => "status board, instant".to_owned(),
            }
        ));
        for (group, ids) in &groups {
            out.push_str(&format!("\n[{group}]\n"));
            for id in ids {
                let Some(candidate) = self.candidates.iter().find(|c| &c.source_id == id) else {
                    continue;
                };
                let evidence = &candidate.rationale.evidence;
                out.push_str(&format!(
                    "  {:<44} {:<13} {:<18} {}/{} series\n",
                    candidate.source_id,
                    candidate.kind(),
                    format!("{:?}", candidate.rationale.rule),
                    evidence.metric_type.as_str(),
                    evidence.series,
                ));
                out.push_str(&format!("      {}\n", candidate.source.query));
                if evidence.members.len() > 1 {
                    out.push_str(&format!("      family: {}\n", evidence.members.join(" + ")));
                }
                for caveat in &candidate.rationale.caveats {
                    out.push_str(&format!("      caveat: {caveat}\n"));
                }
            }
        }
        if !self.reported.is_empty() {
            out.push_str("\n[reported: no candidate]\n");
            for reported in &self.reported {
                out.push_str(&format!(
                    "  {:<44} {:<18} {}\n",
                    reported.metric,
                    format!("{:?}", reported.disposition),
                    reported.reason
                ));
            }
        }
        out
    }
}

fn prune_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, prune_nulls(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(prune_nulls).collect()),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Names, units, and panel groups
// ---------------------------------------------------------------------------

fn tokens(name: &str) -> Vec<&str> {
    name.split('_').filter(|token| !token.is_empty()).collect()
}

/// The rendered unit for a trailing unit token, or `None` when the name does
/// not say. Deliberately conservative: `docs/widget-quality.md` requires that a
/// unit is never implied, and implying the wrong one is worse than leaving it
/// for the reviewer — so an unguessable unit becomes a caveat, not a guess.
fn unit_token(token: &str) -> Option<&'static str> {
    Some(match token {
        "bytes" => "B",
        "seconds" => "s",
        "ms" | "milliseconds" => "ms",
        "watts" => "W",
        "pct" | "percent" => "%",
        "ratio" | "fraction" => "%",
        "celsius" => "°C",
        "volts" => "V",
        "amps" => "A",
        "hertz" => "Hz",
        _ => return None,
    })
}

fn trailing_unit(base: &str) -> Option<&'static str> {
    unit_token(tokens(base).last()?)
}

/// Whether a name's trailing token means the value is already a proportion, and
/// by how much it must be scaled to reach the 0..100 a gauge draws.
fn percent_scale(base: &str) -> Option<Option<f64>> {
    match *tokens(base).last()? {
        "ratio" | "fraction" => Some(Some(100.0)),
        "pct" | "percent" => Some(None),
        _ => None,
    }
}

/// Base tokens with a trailing unit token dropped. `..._latency_seconds`
/// titles as "latency", not "latency seconds": the unit is carried as a unit.
fn display_tokens(base: &str) -> Vec<&str> {
    let mut tokens = tokens(base);
    if tokens.len() > 1 && unit_token(tokens[tokens.len() - 1]).is_some() {
        tokens.pop();
    }
    tokens
}

fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.chars() {
        let lower = byte.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(64).collect::<String>()
}

fn humanize(tokens: &[&str], qualifier: &str) -> String {
    let mut words: Vec<String> = tokens.iter().map(|token| token.to_string()).collect();
    if !qualifier.is_empty() {
        words.extend(qualifier.split('-').map(str::to_string));
    }
    let mut text = words.join(" ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    text
}

fn common_prefix_tokens(names: &[&str]) -> usize {
    let Some(first) = names.first().map(|name| tokens(name)) else {
        return 0;
    };
    let mut common = first.len();
    for name in &names[1..] {
        let other = tokens(name);
        common = common.min(
            first
                .iter()
                .zip(other.iter())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    common
}

/// Panel group per metric: cluster by first token, then take the cluster's
/// longest common prefix. `fpl_auth_*` groups as `fpl_auth` while `bifrost_*`
/// groups as `bifrost`, because that is what the names actually share — nothing
/// is configured.
///
/// The prefix is capped short of consuming a whole metric name, so a group and
/// the widget inside it never end up with the same identity.
fn panel_groups(facts: &[MetricFact]) -> BTreeMap<String, String> {
    let mut clusters: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for fact in facts {
        let head = tokens(&fact.name).first().copied().unwrap_or("metric");
        clusters.entry(head).or_default().push(&fact.name);
    }
    let mut groups = BTreeMap::new();
    for names in clusters.into_values() {
        let shortest = names
            .iter()
            .map(|name| tokens(name).len())
            .min()
            .unwrap_or(1);
        let cap = shortest.saturating_sub(1).max(1);
        let common = common_prefix_tokens(&names).clamp(1, cap);
        let group = tokens(names[0])[..common].join("_");
        for name in names {
            groups.insert(name.to_owned(), group.clone());
        }
    }
    groups
}

/// The `name` half of `owner/name`: what is left of the base once the group
/// prefix and the unit token are removed, plus the rule's qualifier. Falls back
/// to the base itself for a single-token metric such as `up`, where the group
/// has already consumed the whole name.
fn source_name(base: &str, group: &str, qualifier: &str) -> String {
    let base_tokens = display_tokens(base);
    let group_len = tokens(group).len().min(base_tokens.len());
    let mut parts: Vec<String> = base_tokens[group_len..]
        .iter()
        .map(|token| token.to_string())
        .collect();
    if !qualifier.is_empty() {
        parts.push(qualifier.to_owned());
    }
    if parts.is_empty() {
        return sanitize(base);
    }
    sanitize(&parts.join("-"))
}

/// The widget's title. A HELP string is what the exporter's author says the
/// metric means, which beats anything reconstructed from the name — so it wins
/// when it is short enough to read as a label.
fn display_text(base: &str, help: Option<&str>, qualifier: &str) -> String {
    if let Some(help) = help {
        let help = help.trim().trim_end_matches('.');
        if !help.is_empty() && help.chars().count() <= MAX_TITLE_FROM_HELP {
            return humanize(&[help], qualifier);
        }
    }
    humanize(&display_tokens(base), qualifier)
}

// ---------------------------------------------------------------------------
// Family detection
// ---------------------------------------------------------------------------

struct FamilyGroup<'a> {
    family: Family,
    /// The name the widget is derived from: the shared stem for a family, the
    /// metric itself for a singleton.
    base: String,
    /// Members, primary first. Every input metric lands in exactly one of these.
    members: Vec<&'a MetricFact>,
}

impl<'a> FamilyGroup<'a> {
    fn primary(&self) -> &'a MetricFact {
        self.members[0]
    }
    fn names(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|member| member.name.clone())
            .collect()
    }
}

/// Groups metrics that describe one thing, most specific rule first, so a
/// `_sum` inside a histogram family is never also claimed as half an average.
/// Every metric is consumed exactly once.
fn families(facts: &[MetricFact]) -> Vec<FamilyGroup<'_>> {
    let index: BTreeMap<&str, &MetricFact> = facts
        .iter()
        .map(|fact| (fact.name.as_str(), fact))
        .collect();
    let mut consumed: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<FamilyGroup> = Vec::new();

    // Histograms. A `_bucket` implies the family even when the metadata for the
    // stem is not in the survey, and a histogram-typed stem implies it even when
    // the suffixed series are not.
    let mut bases: BTreeSet<String> = BTreeSet::new();
    for fact in facts {
        if let Some(base) = fact.name.strip_suffix("_bucket") {
            bases.insert(base.to_owned());
        } else if fact.metric_type == MetricType::Histogram {
            bases.insert(fact.name.clone());
        }
    }
    for base in bases {
        let mut members = Vec::new();
        for name in [
            format!("{base}_bucket"),
            base.clone(),
            format!("{base}_sum"),
            format!("{base}_count"),
        ] {
            if let Some(fact) = index.get(name.as_str())
                && consumed.insert(fact.name.as_str())
            {
                members.push(*fact);
            }
        }
        if !members.is_empty() {
            out.push(FamilyGroup {
                family: Family::Histogram,
                base,
                members,
            });
        }
    }

    // A `_sum` beside its `_count` with no buckets: a summary, i.e. an average.
    for fact in facts {
        let Some(base) = fact.name.strip_suffix("_sum") else {
            continue;
        };
        if consumed.contains(fact.name.as_str()) {
            continue;
        }
        let Some(count) = index.get(format!("{base}_count").as_str()).copied() else {
            continue;
        };
        if consumed.contains(count.name.as_str()) {
            continue;
        }
        consumed.insert(fact.name.as_str());
        consumed.insert(count.name.as_str());
        out.push(FamilyGroup {
            family: Family::Summary,
            base: base.to_owned(),
            members: vec![fact, count],
        });
    }

    // `X_used_bytes` beside `X_bytes`: a proportion, not two quantities.
    for fact in facts {
        if consumed.contains(fact.name.as_str()) {
            continue;
        }
        let Some(total) = utilization_partner(&fact.name, &index, &consumed) else {
            continue;
        };
        consumed.insert(fact.name.as_str());
        consumed.insert(total.name.as_str());
        // Named off the total, minus its unit token: the widget is a percentage,
        // so `..._memory_bytes` becomes `..._memory`.
        let base = display_tokens(&total.name).join("_");
        out.push(FamilyGroup {
            family: Family::Utilization,
            base,
            members: vec![fact, total],
        });
    }

    // `X_errors_total` beside `X_requests_total`: a ratio, not two ramps.
    for fact in facts {
        if consumed.contains(fact.name.as_str()) || fact.metric_type != MetricType::Counter {
            continue;
        }
        let Some(total) = error_rate_partner(&fact.name, &index, &consumed) else {
            continue;
        };
        let base = strip_error_suffix(&fact.name)
            .map(str::to_owned)
            .unwrap_or_else(|| fact.name.clone());
        consumed.insert(fact.name.as_str());
        consumed.insert(total.name.as_str());
        out.push(FamilyGroup {
            family: Family::ErrorRate,
            base,
            members: vec![fact, total],
        });
    }

    for fact in facts {
        if consumed.insert(fact.name.as_str()) {
            out.push(FamilyGroup {
                family: Family::Singleton,
                base: fact.name.clone(),
                members: vec![fact],
            });
        }
    }
    out
}

/// The total that a `..._used_...` metric is a fraction of.
fn utilization_partner<'a>(
    name: &str,
    index: &BTreeMap<&'a str, &'a MetricFact>,
    consumed: &BTreeSet<&str>,
) -> Option<&'a MetricFact> {
    let parts = tokens(name);
    let used = parts.iter().position(|token| *token == "used")?;
    let without: String = [&parts[..used], &parts[used + 1..]].concat().join("_");
    let mut replaced = parts.clone();
    replaced[used] = "total";
    for candidate in [without, replaced.join("_"), parts.join("_") + "_total"] {
        if candidate == name {
            continue;
        }
        if let Some(fact) = index.get(candidate.as_str())
            && !consumed.contains(fact.name.as_str())
            && fact.metric_type == MetricType::Gauge
        {
            return Some(fact);
        }
    }
    None
}

fn strip_error_suffix(name: &str) -> Option<&str> {
    for suffix in ["_errors_total", "_failures_total", "_errors", "_failures"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return Some(base);
        }
    }
    None
}

/// The denominator an error counter is a rate against. Only an explicit total
/// counts: inventing one from a loosely similar name would produce a ratio that
/// is silently wrong, which is worse than an error count.
fn error_rate_partner<'a>(
    name: &str,
    index: &BTreeMap<&'a str, &'a MetricFact>,
    consumed: &BTreeSet<&str>,
) -> Option<&'a MetricFact> {
    let prefix = strip_error_suffix(name)?;
    for candidate in [
        format!("{prefix}_requests_total"),
        format!("{prefix}s_total"),
        format!("{prefix}_total"),
        format!("{prefix}_calls_total"),
        format!("{prefix}_operations_total"),
    ] {
        if candidate == name {
            continue;
        }
        if let Some(fact) = index.get(candidate.as_str())
            && !consumed.contains(fact.name.as_str())
            && fact.metric_type == MetricType::Counter
        {
            return Some(fact);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The heuristic
// ---------------------------------------------------------------------------

/// Classifies a metadata survey into candidate widgets.
///
/// Fails rather than guesses on a malformed survey: a duplicate metric name
/// means the caller's collection is wrong, and silently keeping one of the two
/// would produce a dashboard nobody can explain.
pub fn survey(facts: &[MetricFact], options: &SurveyOptions) -> Result<Survey, String> {
    options.validate()?;
    let mut seen = BTreeSet::new();
    for fact in facts {
        if !seen.insert(fact.name.as_str()) {
            return Err(format!("duplicate metric in survey: {}", fact.name));
        }
    }

    let groups = panel_groups(facts);
    let mut candidates = Vec::new();
    let mut reported = Vec::new();
    for family in families(facts) {
        let group = groups
            .get(&family.primary().name)
            .cloned()
            .unwrap_or_else(|| family.base.clone());
        match classify(&family, &group, options) {
            Ok(candidate) => candidates.push(candidate),
            Err(reason) => reported.push(reason),
        }
    }
    candidates.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    reported.sort_by(|a, b| a.metric.cmp(&b.metric));

    let mut ids = BTreeSet::new();
    for candidate in &candidates {
        if !ids.insert(candidate.source_id.as_str()) {
            return Err(format!(
                "two candidates claim the same source id: {}",
                candidate.source_id
            ));
        }
        canvas_metrics::stream_identity(&candidate.source_id)?;
    }

    Ok(Survey {
        schema_version: SURVEY_SCHEMA_VERSION,
        options: options.clone(),
        candidates,
        reported,
    })
}

/// Everything a rule decides, before it becomes two files.
struct Draft {
    kind: WidgetKind,
    rule: Rule,
    query: String,
    qualifier: &'static str,
    unit: Option<String>,
    scale: Option<f64>,
    alternatives: Vec<WidgetKind>,
    caveats: Vec<String>,
    /// Appended to the title when a query drops series the viewer would
    /// otherwise assume were included.
    disclosure: Option<String>,
}

impl Draft {
    fn new(kind: WidgetKind, rule: Rule, query: String) -> Self {
        Self {
            kind,
            rule,
            query,
            qualifier: "",
            unit: None,
            scale: None,
            alternatives: Vec::new(),
            caveats: Vec::new(),
            disclosure: None,
        }
    }
    fn qualified(mut self, qualifier: &'static str) -> Self {
        self.qualifier = qualifier;
        self
    }
    fn unit(mut self, unit: Option<impl Into<String>>) -> Self {
        self.unit = unit.map(Into::into);
        self
    }
    fn instead(mut self, alternatives: &[WidgetKind]) -> Self {
        self.alternatives = alternatives.to_vec();
        self
    }
    fn caveat(mut self, caveat: impl Into<String>) -> Self {
        self.caveats.push(caveat.into());
        self
    }
    fn disclose(mut self, disclosure: impl Into<String>) -> Self {
        self.disclosure = Some(disclosure.into());
        self
    }
}

fn classify(family: &FamilyGroup, group: &str, options: &SurveyOptions) -> Result<Candidate, Reported> {
    let primary = family.primary();
    let base = family.base.as_str();
    let rate = options.rate_window.as_str();
    let windowed = options.windowed_dashboard();

    let draft = match family.family {
        Family::Histogram => {
            let bucket = format!("{base}_bucket");
            let latency = display_tokens(base)
                .iter()
                .any(|token| matches!(*token, "latency" | "duration" | "time" | "delay"));
            let mut draft = if latency {
                Draft::new(
                    WidgetKind::TimeSeries,
                    Rule::LatencyQuantile,
                    format!(
                        "histogram_quantile({LATENCY_QUANTILE}, sum by (le) (rate({bucket}[{rate}])))"
                    ),
                )
                .qualified("p95")
                .unit(trailing_unit(base))
                .instead(&[WidgetKind::Heatmap, WidgetKind::Stat])
                .caveat(
                    "quantile is aggregated across every label but le; add labels to the \
                     by-clause to split it",
                )
            } else {
                Draft::new(
                    WidgetKind::Heatmap,
                    Rule::BucketHeatmap,
                    format!("sum by (le) (rate({bucket}[{rate}]))"),
                )
                .qualified("distribution")
                .unit(trailing_unit(base))
                .instead(&[WidgetKind::Histogram, WidgetKind::TimeSeries])
            };
            let absorbed: Vec<String> = family
                .members
                .iter()
                .skip(1)
                .map(|member| member.name.clone())
                .collect();
            if !absorbed.is_empty() {
                draft = draft.caveat(format!(
                    "absorbed {}; the mean is rate(_sum)/rate(_count) if a quantile is not what \
                     you want",
                    absorbed.join(" and ")
                ));
            }
            draft
        }
        Family::Summary => {
            let kind = if windowed {
                WidgetKind::TimeSeries
            } else {
                WidgetKind::Stat
            };
            Draft::new(
                kind,
                Rule::SummaryAverage,
                format!("sum(rate({base}_sum[{rate}])) / sum(rate({base}_count[{rate}]))"),
            )
            .qualified("mean")
            .unit(trailing_unit(base))
            .instead(&[WidgetKind::TimeSeries, WidgetKind::Stat])
            .caveat("a mean hides the tail; prefer a histogram if this metric has buckets")
        }
        Family::Utilization => {
            let used = &family.members[0].name;
            let total = &family.members[1].name;
            Draft::new(
                WidgetKind::Gauge,
                Rule::Utilization,
                format!("100 * sum({used}) / sum({total})"),
            )
            .qualified("utilization")
            .unit(Some("%"))
            .instead(&[WidgetKind::TimeSeries, WidgetKind::Stat])
            .caveat(format!(
                "summed across all {} series; drop the sums to see one bar per device",
                primary.series
            ))
        }
        Family::ErrorRate => {
            let errors = &family.members[0].name;
            let total = &family.members[1].name;
            let kind = if windowed {
                WidgetKind::TimeSeries
            } else {
                WidgetKind::Gauge
            };
            Draft::new(
                kind,
                Rule::ErrorRate,
                format!("100 * sum(rate({errors}[{rate}])) / sum(rate({total}[{rate}]))"),
            )
            .qualified("error-rate")
            .unit(Some("%"))
            .instead(&[WidgetKind::TimeSeries, WidgetKind::Gauge])
            .caveat("thresholds are policy, not metadata: set warn_above before adopting")
        }
        Family::Singleton => singleton(primary, options)?,
    };

    Ok(build(family, group, draft, options))
}

/// One metric, no relatives.
fn singleton(fact: &MetricFact, options: &SurveyOptions) -> Result<Draft, Reported> {
    let name = fact.name.as_str();
    let rate = options.rate_window.as_str();
    let windowed = options.windowed_dashboard();
    let label = fact.labels.first().cloned();

    if name.ends_with("_info") || name == "info" {
        return Err(Reported {
            metric: name.to_owned(),
            metric_type: fact.metric_type,
            series: fact.series,
            disposition: Disposition::LabelEnrichment,
            reason: "an _info metric is a constant 1 carrying labels; join it onto another \
                     metric to enrich that widget's series names rather than drawing it"
                .to_owned(),
        });
    }

    // Up-ness first: it is a gauge by type, but summing it yields a number that
    // is identical whether one target is down or eleven are.
    if name == "up" || name.ends_with("_up") {
        let mut draft = Draft::new(WidgetKind::NodeSummary, Rule::UpStatusList, name.to_owned())
            .instead(&[WidgetKind::Stat, WidgetKind::Heatmap])
            .caveat(format!(
                "sum({name}) is the common alternative and reads as a single number, which hides \
                 which of the {} targets is down",
                fact.series
            ));
        if fact.series > RANKED_MAX_SERIES {
            draft = draft.caveat(format!(
                "{} targets is more than a status list reads well at; consider filtering by job",
                fact.series
            ));
        }
        return Ok(draft);
    }

    match fact.metric_type {
        // The headline rule. Every counter, at every cardinality, becomes a
        // rate: the raw value is a ramp whose slope is the only information in
        // it, and whose height is an artifact of process restarts.
        MetricType::Counter => {
            let mut query = format!("rate({name}[{rate}])");
            let mut draft_caveats: Vec<String> = Vec::new();
            let mut disclosure = None;
            if fact.series > 1 {
                match &label {
                    Some(label) => query = format!("sum by ({label}) ({query})"),
                    None => draft_caveats.push(format!(
                        "{} series and no label recorded; every series draws as its own line",
                        fact.series
                    )),
                }
            }
            if fact.series > TIME_SERIES_MAX {
                query = format!("topk({TIME_SERIES_MAX}, {query})");
                disclosure = Some(format!("top {TIME_SERIES_MAX} of {}", fact.series));
                draft_caveats.push(format!(
                    "cardinality is counted across every label, so the by-clause may collapse \
                     below {TIME_SERIES_MAX} and the topk become a no-op"
                ));
            }
            let unit = match trailing_unit(&strip_total(name)) {
                Some(unit) => format!("{unit}/s"),
                None => "/s".to_owned(),
            };
            let mut draft = Draft::new(WidgetKind::TimeSeries, Rule::CounterRate, query)
                .unit(Some(unit))
                .instead(&[WidgetKind::RankedBar, WidgetKind::Stat]);
            if let Some(disclosure) = disclosure {
                draft = draft.disclose(disclosure);
            }
            for caveat in draft_caveats {
                draft = draft.caveat(caveat);
            }
            Ok(draft)
        }

        MetricType::Gauge => {
            // Already a proportion: a gauge draws 0..100, so a ratio is scaled
            // and a percentage is left alone.
            if let Some(scale) = percent_scale(name) {
                let aggregated = fact.series > 1;
                let query = if aggregated {
                    format!("max({name})")
                } else {
                    name.to_owned()
                };
                let mut draft = Draft::new(WidgetKind::Gauge, Rule::PercentGauge, query)
                    .unit(Some("%"))
                    .instead(&[WidgetKind::RankedBar, WidgetKind::TimeSeries]);
                draft.scale = scale;
                if aggregated {
                    draft = draft.caveat(format!(
                        "a gauge shows one number, so {} series are reduced with max(); a ranked \
                         bar keeps them apart",
                        fact.series
                    ));
                }
                return Ok(draft);
            }

            let unit = fact
                .unit
                .clone()
                .or_else(|| trailing_unit(name).map(str::to_owned));
            let unknown_unit = unit.is_none();
            let mut draft = match (fact.series, windowed) {
                (0 | 1, _) => Draft::new(WidgetKind::Stat, Rule::SingleSeriesStat, name.to_owned())
                    .instead(&[WidgetKind::Gauge, WidgetKind::TimeSeries]),
                (series, false) if series <= RANKED_MAX_SERIES => {
                    Draft::new(WidgetKind::RankedBar, Rule::RankedGauge, name.to_owned())
                        .instead(&[WidgetKind::TimeSeries, WidgetKind::Heatmap])
                }
                (_, false) => Draft::new(
                    WidgetKind::RankedBar,
                    Rule::TopNGauge,
                    format!("topk({RANKED_MAX_SERIES}, {name})"),
                )
                .instead(&[WidgetKind::Heatmap])
                .disclose(format!("top {RANKED_MAX_SERIES} of {}", fact.series)),
                (series, true) if series <= RANKED_MAX_SERIES => {
                    let query = match &label {
                        Some(label) => format!("sum by ({label}) ({name})"),
                        None => name.to_owned(),
                    };
                    Draft::new(WidgetKind::TimeSeries, Rule::WindowedGauge, query)
                        .instead(&[WidgetKind::RankedBar, WidgetKind::Heatmap])
                }
                (_, true) => Draft::new(
                    WidgetKind::Heatmap,
                    Rule::WideGaugeHeatmap,
                    format!("topk({HEATMAP_MAX_ROWS}, {name})"),
                )
                .instead(&[WidgetKind::RankedBar])
                .disclose(format!("top {HEATMAP_MAX_ROWS} of {}", fact.series)),
            };
            draft = draft.unit(unit);
            if unknown_unit {
                draft = draft.caveat(
                    "no unit is inferable from the name and none is declared; set one before \
                     adopting, because an unlabelled magnitude is unreadable on a wall",
                );
            }
            Ok(draft)
        }

        // A summary's stem carries quantile labels rather than a value this
        // heuristic can place, and an untyped metric is exactly the case that
        // must not be defaulted into a stat.
        MetricType::Summary | MetricType::Histogram | MetricType::Untyped => Err(Reported {
            metric: name.to_owned(),
            metric_type: fact.metric_type,
            series: fact.series,
            disposition: Disposition::Unclassified,
            reason: match fact.metric_type {
                MetricType::Untyped => "no TYPE is declared, so neither a rate nor a level can be \
                                        assumed; declare the type in the exporter, or bind this \
                                        one by hand"
                    .to_owned(),
                _ => format!(
                    "a {} with no _bucket, _sum or _count in the survey: nothing here says what \
                     its value means",
                    fact.metric_type.as_str()
                ),
            },
        }),
    }
}

fn strip_total(name: &str) -> String {
    name.strip_suffix("_total").unwrap_or(name).to_owned()
}

/// Turns a decided draft into the two files plus its provenance.
fn build(family: &FamilyGroup, group: &str, draft: Draft, options: &SurveyOptions) -> Candidate {
    let primary = family.primary();
    // `_total` is the counter suffix, and the widget is a rate: keeping it in
    // the id would name the widget after the ramp nobody is drawing.
    let base = match primary.metric_type {
        MetricType::Counter => family.base.strip_suffix("_total").unwrap_or(&family.base),
        _ => family.base.as_str(),
    };
    let group_id = sanitize(group);
    let name = source_name(base, group, draft.qualifier);
    let source_id = format!("{group_id}/{name}");
    let widget_id = format!("{group_id}-{name}");

    let mut text = display_text(base, primary.help.as_deref(), draft.qualifier);
    if let Some(disclosure) = &draft.disclosure {
        text = format!("{text} ({disclosure})");
    }

    let (window_seconds, step_seconds) = if draft.kind.range_query() {
        let (window, step) = options.range();
        (Some(window), Some(step))
    } else {
        (None, None)
    };

    let source = SourceSpec {
        kind: draft.kind.as_str().to_owned(),
        query: draft.query.clone(),
        window_seconds,
        step_seconds,
        label: Some(text.clone()),
        unit: draft.unit.clone(),
        // `le` is a bucket boundary, never a series identity: naming a series
        // after it would label a quantile line "0.5".
        series_labels: primary
            .labels
            .iter()
            .filter(|label| label.as_str() != "le")
            .cloned()
            .collect(),
        warn_above: None,
        critical_above: None,
        warn_below: None,
        critical_below: None,
        scale: draft.scale,
        unavailable: None,
    };

    let widget = WidgetSpec {
        schema_version: WIDGET_SCHEMA_VERSION,
        id: widget_id,
        kind: draft.kind.body(text, draft.unit.clone()),
        theme: None,
        refresh_ms: Some(options.poll_seconds * 1000),
        chrome: Default::default(),
        data: WidgetData {
            // A candidate has never been fed. Saying so in the widget's own
            // words is rule 3 of docs/widget-quality.md: a zero that looks like
            // a reading is worse than a blank panel.
            state: WidgetDataState::NoData,
            source: Some(format!("prometheus:{source_id}")),
            observed_at: None,
            window: Some(window_label(window_seconds)),
            detail: Some(format!(
                "candidate from {:?}; no snapshot yet",
                draft.rule
            )),
            provenance: Provenance::Live,
        },
    };

    Candidate {
        source_id,
        group: group_id,
        widget,
        source,
        rationale: Rationale {
            rule: draft.rule,
            why: draft.rule.why().to_owned(),
            evidence: Evidence {
                metric_type: primary.metric_type,
                series: primary.series,
                family: family.family,
                members: family.names(),
                labels: primary.labels.clone(),
            },
            alternatives: draft.alternatives,
            caveats: draft.caveats,
        },
    }
}

fn window_label(window_seconds: Option<u64>) -> String {
    match window_seconds {
        None => "now".into(),
        Some(seconds) if seconds % 3600 == 0 => format!("{}h", seconds / 3600),
        Some(seconds) => format!("{}m", seconds / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(name: &str, metric_type: MetricType, series: usize, labels: &[&str]) -> MetricFact {
        MetricFact {
            name: name.into(),
            metric_type,
            help: None,
            unit: None,
            series,
            labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        }
    }

    /// Metric names observed on this installation, with the cardinality and
    /// labels each carries. The fixture is the point: every assertion below is
    /// about names a person actually has to build a dashboard for.
    fn observed() -> Vec<MetricFact> {
        use MetricType::{Counter, Gauge, Histogram};
        vec![
            fact("bifrost_backend_up", Gauge, 14, &["backend"]),
            fact(
                "bifrost_provider_price_usd_per_million_tokens",
                Gauge,
                9,
                &["provider", "model"],
            ),
            fact(
                "bifrost_yggdrasil_accelerator_memory_bytes",
                Gauge,
                4,
                &["node", "gpu"],
            ),
            fact(
                "bifrost_yggdrasil_accelerator_memory_used_bytes",
                Gauge,
                4,
                &["node", "gpu"],
            ),
            fact(
                "bifrost_yggdrasil_accelerator_power_watts",
                Gauge,
                4,
                &["node", "gpu"],
            ),
            fact(
                "bifrost_yggdrasil_accelerator_utilization_pct",
                Gauge,
                4,
                &["node", "gpu"],
            ),
            fact(
                "bifrost_yggdrasil_runtime_inflight_requests",
                Gauge,
                6,
                &["node"],
            ),
            fact(
                "bifrost_yggdrasil_runtime_latency_ms",
                Gauge,
                6,
                &["runtime"],
            ),
            fact("palisade_active_connections", Gauge, 1, &[]),
            fact("palisade_bytes_transferred_total", Counter, 2, &["direction"]),
            fact("palisade_request_failures_total", Counter, 3, &["code"]),
            fact(
                "palisade_request_latency_seconds_bucket",
                Histogram,
                36,
                &["le", "route"],
            ),
            fact(
                "palisade_request_latency_seconds_count",
                Counter,
                3,
                &["route"],
            ),
            fact(
                "palisade_request_latency_seconds_sum",
                Counter,
                3,
                &["route"],
            ),
            fact("invoker_cold_starts_total", Counter, 2, &["function"]),
            fact("invoker_errors_total", Counter, 5, &["function"]),
            fact("invoker_requests_total", Counter, 5, &["function"]),
            fact("fpl_auth_active_sessions", Gauge, 1, &[]),
            fact("fpl_auth_vouch_requests_total", Counter, 4, &["outcome"]),
            fact("up", Gauge, 19, &["job", "instance"]),
        ]
    }

    fn board() -> Survey {
        survey(&observed(), &SurveyOptions::default()).expect("survey")
    }

    fn wall() -> Survey {
        survey(&observed(), &SurveyOptions::windowed(1800)).expect("survey")
    }

    /// Cloned rather than borrowed so a test can read one candidate out of a
    /// survey it does not otherwise need to name.
    fn find(survey: &Survey, source_id: &str) -> Candidate {
        survey
            .candidates
            .iter()
            .find(|candidate| candidate.source_id == source_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no candidate {source_id}; have {:?}",
                    survey
                        .candidates
                        .iter()
                        .map(|c| c.source_id.as_str())
                        .collect::<Vec<_>>()
                )
            })
    }

    /// Every candidate that consumed `metric`. More than one is a bug: a metric
    /// drawn twice is a metric one of whose widgets is wrong.
    fn covering<'a>(survey: &'a Survey, metric: &str) -> Vec<&'a Candidate> {
        survey
            .candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .rationale
                    .evidence
                    .members
                    .iter()
                    .any(|member| member == metric)
            })
            .collect()
    }

    fn covered_by(survey: &Survey, metric: &str) -> Vec<String> {
        covering(survey, metric)
            .iter()
            .map(|candidate| candidate.source_id.clone())
            .collect()
    }

    #[test]
    fn accelerator_memory_pair_becomes_one_utilization_gauge() {
        let survey = board();
        let used = "bifrost_yggdrasil_accelerator_memory_used_bytes";
        let total = "bifrost_yggdrasil_accelerator_memory_bytes";
        let covers = covering(&survey, used);
        assert_eq!(covers.len(), 1, "the pair must not produce two widgets");
        let candidate = covers[0];
        assert_eq!(covered_by(&survey, total), vec![candidate.source_id.clone()]);

        assert_eq!(candidate.rationale.rule, Rule::Utilization);
        assert_eq!(candidate.rationale.evidence.family, Family::Utilization);
        assert_eq!(candidate.kind(), "gauge");
        assert_eq!(candidate.source.unit.as_deref(), Some("%"));
        // The shape config/metrics.json already hand-encodes for VRAM, found
        // from the names alone.
        assert_eq!(
            candidate.source.query,
            format!("100 * sum({used}) / sum({total})")
        );
        assert_eq!(
            candidate.source_id,
            "bifrost/yggdrasil-accelerator-memory-utilization"
        );
    }

    #[test]
    fn latency_histogram_family_becomes_one_latency_widget() {
        let survey = board();
        let members = [
            "palisade_request_latency_seconds_bucket",
            "palisade_request_latency_seconds_sum",
            "palisade_request_latency_seconds_count",
        ];
        let candidate = find(&survey, "palisade/request-latency-p95");
        for member in members {
            assert_eq!(
                covered_by(&survey, member),
                vec![candidate.source_id.clone()],
                "{member} must belong to the one latency widget"
            );
        }
        assert_eq!(candidate.rationale.rule, Rule::LatencyQuantile);
        assert_eq!(candidate.rationale.evidence.members.len(), 3);
        assert_eq!(
            candidate.source.query,
            "histogram_quantile(0.95, sum by (le) (rate(palisade_request_latency_seconds_bucket[5m])))"
        );
        assert_eq!(candidate.source.unit.as_deref(), Some("s"));
        assert!(
            candidate
                .rationale
                .caveats
                .iter()
                .any(|caveat| caveat.contains("_sum") || caveat.contains("absorbed")),
            "absorbing _sum and _count is disclosed: {:?}",
            candidate.rationale.caveats
        );
    }

    #[test]
    fn every_counter_becomes_a_rate() {
        let survey = board();
        for fact in observed() {
            if fact.metric_type != MetricType::Counter {
                continue;
            }
            for candidate in covering(&survey, &fact.name) {
                assert!(
                    candidate.source.query.contains(&format!("rate({}", fact.name))
                        || candidate.source.query.contains(&format!("rate({}[", fact.name)),
                    "{} is drawn raw by {}: {}",
                    fact.name,
                    candidate.source_id,
                    candidate.source.query
                );
            }
        }
        // The trap this rule exists for: a bare counter name as the whole query.
        for candidate in &survey.candidates {
            assert_ne!(candidate.source.query, "palisade_bytes_transferred_total");
            assert_ne!(candidate.source.query, "invoker_requests_total");
        }
    }

    #[test]
    fn a_lone_counter_is_a_rate_time_series_not_a_stat() {
        let candidate = find(&board(), "palisade/bytes-transferred");
        assert_eq!(candidate.rationale.rule, Rule::CounterRate);
        assert_eq!(candidate.kind(), "time_series");
        assert_eq!(
            candidate.source.query,
            "sum by (direction) (rate(palisade_bytes_transferred_total[5m]))"
        );
        assert_eq!(candidate.source.unit.as_deref(), Some("/s"));
        // A rate needs a range even on an instant board.
        assert!(candidate.source.range().is_some());
    }

    #[test]
    fn error_pair_becomes_one_error_rate() {
        let survey = board();
        let candidate = find(&survey, "invoker/error-rate");
        assert_eq!(candidate.rationale.rule, Rule::ErrorRate);
        assert_eq!(
            candidate.rationale.evidence.members,
            vec!["invoker_errors_total", "invoker_requests_total"]
        );
        assert_eq!(
            candidate.source.query,
            "100 * sum(rate(invoker_errors_total[5m])) / sum(rate(invoker_requests_total[5m]))"
        );
        assert_eq!(candidate.source.unit.as_deref(), Some("%"));
        assert_eq!(covering(&survey, "invoker_requests_total").len(), 1);
        assert!(
            candidate
                .rationale
                .caveats
                .iter()
                .any(|caveat| caveat.contains("thresholds")),
            "an error rate with no threshold says so"
        );
    }

    #[test]
    fn an_errors_counter_with_no_total_stays_a_rate_not_an_invented_ratio() {
        let candidate = find(&board(), "palisade/request-failures");
        assert_eq!(candidate.rationale.rule, Rule::CounterRate);
        assert_eq!(candidate.rationale.evidence.family, Family::Singleton);
        assert!(
            !candidate.source.query.contains('/'),
            "no denominator exists, so none is invented: {}",
            candidate.source.query
        );
    }

    #[test]
    fn up_is_a_status_list_not_a_number() {
        let survey = board();
        for id in ["up/up", "bifrost/backend-up"] {
            let candidate = find(&survey, id);
            assert_eq!(candidate.rationale.rule, Rule::UpStatusList);
            assert_eq!(candidate.kind(), "node_summary");
            assert!(
                !candidate.source.query.starts_with("sum("),
                "{id} must not collapse to a number: {}",
                candidate.source.query
            );
            assert!(
                candidate
                    .rationale
                    .caveats
                    .iter()
                    .any(|caveat| caveat.contains("sum(")),
                "the number alternative is named so a reviewer can choose it"
            );
            assert!(candidate.rationale.alternatives.contains(&WidgetKind::Stat));
        }
    }

    #[test]
    fn info_metrics_are_reported_as_enrichment_not_drawn() {
        let mut facts = observed();
        facts.push(fact("bifrost_build_info", MetricType::Gauge, 3, &["version"]));
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        assert!(covering(&survey, "bifrost_build_info").is_empty());
        let reported = survey
            .reported
            .iter()
            .find(|reported| reported.metric == "bifrost_build_info")
            .expect("_info is reported, not dropped");
        assert_eq!(reported.disposition, Disposition::LabelEnrichment);
        assert!(reported.reason.contains("labels"));
    }

    #[test]
    fn an_unclassifiable_metric_is_reported_and_never_defaulted_to_a_stat() {
        let mut facts = observed();
        facts.push(fact("elmers_mystery", MetricType::Untyped, 2, &[]));
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        assert!(
            covering(&survey, "elmers_mystery").is_empty(),
            "an untyped metric must not become a stat by default"
        );
        let reported = survey
            .reported
            .iter()
            .find(|reported| reported.metric == "elmers_mystery")
            .expect("reported");
        assert_eq!(reported.disposition, Disposition::Unclassified);
        assert!(reported.reason.contains("TYPE"));
    }

    #[test]
    fn nothing_is_silently_dropped() {
        let mut facts = observed();
        facts.push(fact("bifrost_build_info", MetricType::Gauge, 3, &["version"]));
        facts.push(fact("elmers_mystery", MetricType::Untyped, 2, &[]));
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();

        let mut accounted: BTreeMap<String, usize> = BTreeMap::new();
        for candidate in &survey.candidates {
            for member in &candidate.rationale.evidence.members {
                *accounted.entry(member.clone()).or_default() += 1;
            }
        }
        for reported in &survey.reported {
            *accounted.entry(reported.metric.clone()).or_default() += 1;
        }
        for fact in &facts {
            assert_eq!(
                accounted.get(&fact.name).copied().unwrap_or(0),
                1,
                "{} is claimed {:?} times; every metric is claimed exactly once",
                fact.name,
                accounted.get(&fact.name)
            );
        }
        assert_eq!(accounted.len(), facts.len(), "no metric is invented");
    }

    #[test]
    fn a_single_series_gauge_is_a_stat() {
        for id in ["palisade/active-connections", "fpl-auth/active-sessions"] {
            let candidate = find(&board(), id);
            assert_eq!(candidate.rationale.rule, Rule::SingleSeriesStat);
            assert_eq!(candidate.kind(), "stat");
            assert!(candidate.source.range().is_none(), "an instant query");
        }
    }

    #[test]
    fn a_mid_cardinality_gauge_follows_the_dashboard_intent() {
        let instant = find(&board(), "bifrost/yggdrasil-runtime-inflight-requests");
        assert_eq!(instant.rationale.rule, Rule::RankedGauge);
        assert_eq!(instant.kind(), "ranked_bar");
        assert!(instant.source.range().is_none());

        let trend = find(&wall(), "bifrost/yggdrasil-runtime-inflight-requests");
        assert_eq!(trend.rationale.rule, Rule::WindowedGauge);
        assert_eq!(trend.kind(), "time_series");
        assert_eq!(
            trend.source.query,
            "sum by (node) (bifrost_yggdrasil_runtime_inflight_requests)"
        );
        assert_eq!(trend.source.range(), Some((1800, 30)));
        // Each names the other, so flipping the guess is a one-field edit.
        assert!(
            instant
                .rationale
                .alternatives
                .contains(&WidgetKind::TimeSeries)
        );
        assert!(trend.rationale.alternatives.contains(&WidgetKind::RankedBar));
    }

    #[test]
    fn a_percentage_gauge_is_a_gauge_and_a_ratio_is_scaled() {
        let candidate = find(&board(), "bifrost/yggdrasil-accelerator-utilization");
        assert_eq!(candidate.rationale.rule, Rule::PercentGauge);
        assert_eq!(candidate.kind(), "gauge");
        assert_eq!(candidate.source.scale, None, "_pct is already 0..100");
        assert_eq!(
            candidate.source.query,
            "max(bifrost_yggdrasil_accelerator_utilization_pct)"
        );

        let ratio = vec![fact("cache_hit_ratio", MetricType::Gauge, 1, &[])];
        let survey = survey(&ratio, &SurveyOptions::default()).unwrap();
        let candidate = &survey.candidates[0];
        assert_eq!(candidate.rationale.rule, Rule::PercentGauge);
        assert_eq!(candidate.source.scale, Some(100.0), "a ratio is x100");
        assert_eq!(candidate.source.unit.as_deref(), Some("%"));
    }

    #[test]
    fn a_wide_gauge_discloses_the_tail_it_drops() {
        let facts = vec![fact("mesh_node_temperature_celsius", MetricType::Gauge, 37, &["node"])];
        let instant = survey(&facts, &SurveyOptions::default()).unwrap();
        let candidate = &instant.candidates[0];
        assert_eq!(candidate.rationale.rule, Rule::TopNGauge);
        assert_eq!(candidate.kind(), "ranked_bar");
        assert_eq!(
            candidate.source.query,
            "topk(12, mesh_node_temperature_celsius)"
        );
        assert!(
            candidate
                .source
                .label
                .as_deref()
                .is_some_and(|label| label.contains("top 12 of 37")),
            "the dropped tail is disclosed in the widget's own title: {:?}",
            candidate.source.label
        );

        let trend = survey(&facts, &SurveyOptions::windowed(900)).unwrap();
        let candidate = &trend.candidates[0];
        assert_eq!(candidate.rationale.rule, Rule::WideGaugeHeatmap);
        assert_eq!(candidate.kind(), "heatmap");
        assert!(
            candidate
                .source
                .label
                .as_deref()
                .is_some_and(|label| label.contains("top 12 of 37"))
        );
    }

    #[test]
    fn a_summary_pair_becomes_one_average() {
        let facts = vec![
            fact("queue_wait_seconds_sum", MetricType::Counter, 2, &["queue"]),
            fact("queue_wait_seconds_count", MetricType::Counter, 2, &["queue"]),
        ];
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        assert_eq!(survey.candidates.len(), 1, "one average, not two counters");
        let candidate = &survey.candidates[0];
        assert_eq!(candidate.rationale.rule, Rule::SummaryAverage);
        assert_eq!(
            candidate.source.query,
            "sum(rate(queue_wait_seconds_sum[5m])) / sum(rate(queue_wait_seconds_count[5m]))"
        );
        assert_eq!(candidate.source.unit.as_deref(), Some("s"));
    }

    #[test]
    fn panel_groups_follow_the_longest_common_prefix() {
        let groups = board().groups();
        let names: Vec<&str> = groups.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec!["bifrost", "fpl-auth", "invoker", "palisade", "up"],
            "fpl_auth_* share two tokens; bifrost_* share one"
        );
        assert_eq!(groups["invoker"].len(), 2, "the error pair collapsed one");
    }

    #[test]
    fn every_source_id_routes_to_a_canvas_stream() {
        // canvas-metrics owns the owner/name split Canvas performs, so the ids
        // are checked against it rather than against a copy of its rules.
        let (board, wall) = (board(), wall());
        for candidate in board.candidates.iter().chain(wall.candidates.iter()) {
            canvas_metrics::stream_identity(&candidate.source_id)
                .unwrap_or_else(|error| panic!("{}: {error}", candidate.source_id));
        }
    }

    #[test]
    fn the_emitted_mapping_reloads_as_a_connector_mapping() {
        let survey = wall();
        let json = survey.mapping_json();
        let reloaded: Mapping = serde_json::from_value(json.clone())
            .unwrap_or_else(|error| panic!("{error}: {json:#}"));
        assert_eq!(reloaded.sources.len(), survey.candidates.len());
        assert_eq!(reloaded.poll_seconds, 30);
        for candidate in &survey.candidates {
            let spec = &reloaded.sources[&candidate.source_id];
            assert_eq!(spec.query, candidate.source.query);
            assert!(spec.unavailable.is_none());
        }
    }

    #[test]
    fn every_emitted_widget_reloads_as_a_widget_spec() {
        let survey = board();
        for (filename, json) in survey.widget_files() {
            let spec: WidgetSpec = serde_json::from_value(json.clone())
                .unwrap_or_else(|error| panic!("{filename}: {error}: {json:#}"));
            assert_eq!(spec.data.state, WidgetDataState::NoData);
            assert!(
                spec.data.detail.is_some(),
                "{filename} says why it has no data"
            );
            assert!(spec.data.source.as_deref().is_some_and(|source| source
                .starts_with("prometheus:")));
        }
    }

    #[test]
    fn every_proposed_kind_is_one_this_build_can_draw() {
        for kind in [
            WidgetKind::Stat,
            WidgetKind::Gauge,
            WidgetKind::TimeSeries,
            WidgetKind::RankedBar,
            WidgetKind::Heatmap,
            WidgetKind::Histogram,
            WidgetKind::NodeSummary,
        ] {
            assert!(
                canvas_widgets::is_known_widget_kind(kind.as_str()),
                "{} is not a kind canvas-widgets renders",
                kind.as_str()
            );
        }
    }

    #[test]
    fn every_candidate_carries_its_provenance() {
        for candidate in board().candidates {
            let rationale = &candidate.rationale;
            assert!(!rationale.why.is_empty());
            assert_eq!(rationale.why, rationale.rule.why());
            assert!(
                !rationale.evidence.members.is_empty(),
                "{} names no metric",
                candidate.source_id
            );
            assert_eq!(
                rationale.evidence.members.len() > 1,
                rationale.evidence.family != Family::Singleton,
                "{} disagrees about whether it is a family",
                candidate.source_id
            );
        }
    }

    #[test]
    fn a_help_string_wins_over_a_reconstructed_name() {
        let mut facts = vec![fact("fpl_auth_active_sessions", MetricType::Gauge, 1, &[])];
        facts[0].help = Some("Signed-in SSO sessions.".into());
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        assert_eq!(
            survey.candidates[0].source.label.as_deref(),
            Some("Signed-in SSO sessions")
        );
    }

    #[test]
    fn a_declared_unit_wins_over_an_inferred_one() {
        let mut facts = vec![fact("palisade_active_connections", MetricType::Gauge, 1, &[])];
        facts[0].unit = Some("connections".into());
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        assert_eq!(
            survey.candidates[0].source.unit.as_deref(),
            Some("connections")
        );
        assert!(survey.candidates[0].rationale.caveats.is_empty());
    }

    #[test]
    fn an_unguessable_unit_is_a_caveat_rather_than_a_guess() {
        let candidate = find(&board(), "bifrost/provider-price-usd-per-million-tokens");
        assert!(candidate.source.unit.is_none());
        assert!(
            candidate
                .rationale
                .caveats
                .iter()
                .any(|caveat| caveat.contains("unit")),
            "{:?}",
            candidate.rationale.caveats
        );
    }

    #[test]
    fn a_duplicate_metric_fails_loudly() {
        let facts = vec![
            fact("up", MetricType::Gauge, 3, &[]),
            fact("up", MetricType::Gauge, 4, &[]),
        ];
        let error = survey(&facts, &SurveyOptions::default()).unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
    }

    #[test]
    fn a_malformed_rate_window_fails_loudly() {
        let options = SurveyOptions {
            rate_window: "five minutes".into(),
            ..SurveyOptions::default()
        };
        let error = survey(&observed(), &options).unwrap_err();
        assert!(error.contains("rate_window"), "{error}");
    }

    #[test]
    fn the_survey_is_deterministic_and_ordered() {
        let once = serde_json::to_value(board()).unwrap();
        let twice = serde_json::to_value(board()).unwrap();
        assert_eq!(once, twice, "two runs over one fixture must agree");

        let ids: Vec<String> = board()
            .candidates
            .iter()
            .map(|candidate| candidate.source_id.clone())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "candidates are emitted in id order");
    }

    #[test]
    fn the_report_names_every_candidate_and_everything_it_refused() {
        let mut facts = observed();
        facts.push(fact("elmers_mystery", MetricType::Untyped, 2, &[]));
        let survey = survey(&facts, &SurveyOptions::default()).unwrap();
        let report = survey.report();
        for candidate in &survey.candidates {
            assert!(report.contains(&candidate.source_id), "{}", candidate.source_id);
        }
        assert!(report.contains("elmers_mystery"));
    }
}
