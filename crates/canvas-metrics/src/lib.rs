//! Turns Prometheus results into Canvas widget snapshots.
//!
//! This connector never holds a credential. It names an operation and a PromQL
//! expression, the desktop bridge attaches the owner's bearer, and only a bounded
//! JSON result comes back. Everything here is a pure transform over that result,
//! so the mapping is testable without a network or a keyring.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, time::Duration};

pub const STREAM_SCHEMA_VERSION: u32 = 1;
pub const CONTRACT: &str = "canvas.widget.snapshot.v1";
/// Canvas's own ceiling on a stream event.
pub const MAX_STREAM_MESSAGE: usize = 64 * 1024;

/// The limit that actually binds. macOS caps an AF_UNIX datagram at
/// `net.local.dgram.maxdgram`, 2048 bytes by default, so Canvas's 64KB ceiling
/// is never reached in practice: an oversized send fails with EMSGSIZE rather
/// than being truncated. Snapshots are fitted to this, with headroom for the
/// envelope around the widget.
pub const MAX_DATAGRAM: usize = 2048;

/// Bytes the envelope adds around the widget: identity, contract, sequence,
/// timestamps and the data block. Measured against the widest ids in use.
pub const ENVELOPE_OVERHEAD: usize = 420;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub schema_version: u32,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    /// Logical widget source id (`bifrost/health`) to its query.
    pub sources: BTreeMap<String, SourceSpec>,
}

fn default_poll() -> u64 {
    30
}

/// Bounds a configured poll interval is held to. The floor is what the send
/// pacing below is budgeted against: a burst has to finish well inside the
/// shortest interval anyone can ask for.
pub const MIN_POLL_SECONDS: u64 = 5;
pub const MAX_POLL_SECONDS: u64 = 3600;

/// How one logical widget source is produced. The PromQL lives here, host-side,
/// deliberately not in the view JSON: views are shared and reconfigurable, and a
/// query is an operational detail that must not become part of a saved layout.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// Widget kind this feeds: stat, gauge, time_series, heatmap, ranked_bar.
    pub kind: String,
    pub query: String,
    /// Present for range kinds; absent means an instant query.
    #[serde(default)]
    pub window_seconds: Option<u64>,
    #[serde(default)]
    pub step_seconds: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    /// Series label, taken from these metric labels in order. Falls back to the
    /// metric name so a series is never rendered as an empty string.
    #[serde(default)]
    pub series_labels: Vec<String>,
    #[serde(default)]
    pub warn_above: Option<f64>,
    #[serde(default)]
    pub critical_above: Option<f64>,
    #[serde(default)]
    pub warn_below: Option<f64>,
    #[serde(default)]
    pub critical_below: Option<f64>,
    /// Multiply raw samples, for unit changes such as ratio to percent.
    #[serde(default)]
    pub scale: Option<f64>,
    /// Recorded when a view declares a source no metric backs yet, so the widget
    /// says why instead of showing an indefinite Loading.
    #[serde(default)]
    pub unavailable: Option<String>,
}

impl SourceSpec {
    pub fn range(&self) -> Option<(u64, u64)> {
        match (self.window_seconds, self.step_seconds) {
            (Some(window), Some(step)) => Some((window, step)),
            _ => None,
        }
    }
    fn scaled(&self, value: f64) -> f64 {
        value * self.scale.unwrap_or(1.0)
    }
    fn status(&self, value: f64) -> &'static str {
        let over = |limit: Option<f64>| limit.is_some_and(|limit| value >= limit);
        let under = |limit: Option<f64>| limit.is_some_and(|limit| value <= limit);
        if over(self.critical_above) || under(self.critical_below) {
            "critical"
        } else if over(self.warn_above) || under(self.warn_below) {
            "warning"
        } else {
            "good"
        }
    }
}

/// Canvas derives `owner` and `stream` from the widget's own source string, so
/// the emitter must produce exactly the same split or the snapshot lands on no
/// widget at all.
pub fn stream_identity(source_id: &str) -> Result<(String, String), String> {
    let (owner, name) = source_id
        .split_once('/')
        .ok_or_else(|| format!("source id must be owner/name: {source_id}"))?;
    let stream = format!("prometheus-{name}");
    let valid = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    // `name` is checked separately: an empty one still yields the valid-looking
    // stream "prometheus-", which would bind to no widget while looking correct.
    if !valid(owner) || !valid(name) || !valid(&stream) {
        return Err(format!(
            "source id is not a valid stream identity: {source_id}"
        ));
    }
    Ok((owner.to_owned(), stream))
}

fn samples(result: &Value) -> &Vec<Value> {
    static EMPTY: &Vec<Value> = &Vec::new();
    result
        .pointer("/data/data/result")
        .and_then(Value::as_array)
        .unwrap_or(EMPTY)
}

fn label_for(spec: &SourceSpec, metric: &Value) -> String {
    for key in &spec.series_labels {
        if let Some(value) = metric.get(key).and_then(Value::as_str) {
            return value.to_owned();
        }
    }
    metric
        .get("__name__")
        .and_then(Value::as_str)
        .unwrap_or("series")
        .to_owned()
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// Builds the `widget` half of a snapshot payload. Returns `None` when the query
/// succeeded but matched nothing, which is a no-data state rather than an error.
pub fn widget(spec: &SourceSpec, result: &Value) -> Option<Value> {
    widget_within(spec, result, 180)
}

/// `points` caps samples per series so a snapshot can be shrunk to fit the
/// datagram limit without changing what it means.
pub fn widget_within(spec: &SourceSpec, result: &Value, points: usize) -> Option<Value> {
    let samples = samples(result);
    if samples.is_empty() {
        return None;
    }
    match spec.kind.as_str() {
        "stat" => {
            let value = spec.scaled(number(samples[0].pointer("/value/1")?)?);
            Some(json!({
                "kind": "stat",
                "label": spec.label.clone().unwrap_or_else(|| "Value".into()),
                "value": value,
                "unit": spec.unit,
                "state": spec.status(value),
            }))
        }
        "gauge" => {
            let value = spec.scaled(number(samples[0].pointer("/value/1")?)?);
            Some(json!({
                "kind": "gauge",
                "value": value.clamp(0.0, 100.0),
                "label": spec.label,
            }))
        }
        "ranked_bar" => {
            let mut bars: Vec<Value> = samples
                .iter()
                .filter_map(|sample| {
                    let value = spec.scaled(number(sample.pointer("/value/1")?)?);
                    Some(json!({"label": label_for(spec, sample.get("metric")?), "value": value}))
                })
                .collect();
            bars.sort_by(|a, b| {
                b["value"]
                    .as_f64()
                    .partial_cmp(&a["value"].as_f64())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            bars.truncate(12);
            Some(json!({
                "kind": "ranked_bar",
                "title": spec.label.clone().unwrap_or_else(|| "Ranked".into()),
                "bars": bars,
                "unit": spec.unit,
            }))
        }
        "time_series" => {
            let series: Vec<Value> = samples
                .iter()
                .take(points.clamp(1, 8))
                .filter_map(|sample| {
                    let raw = sample.get("values")?.as_array()?;
                    // Canvas caps a datagram at 64KB; thin long ranges rather
                    // than emit a snapshot the socket will reject.
                    let stride = raw.len().div_ceil(points.max(2)).max(1);
                    let points: Vec<Value> = raw
                        .iter()
                        .step_by(stride)
                        .filter_map(|point| {
                            let pair = point.as_array()?;
                            Some(json!({
                                "x": number(pair.first()?)?,
                                "y": spec.scaled(number(pair.get(1)?)?),
                            }))
                        })
                        .collect();
                    Some(json!({"label": label_for(spec, sample.get("metric")?), "points": points}))
                })
                .collect();
            if series.is_empty() {
                return None;
            }
            Some(json!({
                "kind": "time_series",
                "title": spec.label.clone().unwrap_or_else(|| "Series".into()),
                "series": series,
                "unit": spec.unit,
            }))
        }
        "heatmap" => {
            let rows: Vec<String> = samples
                .iter()
                .take(points.clamp(1, 12))
                .filter_map(|sample| Some(label_for(spec, sample.get("metric")?)))
                .collect();
            let first = samples.first()?.get("values")?.as_array()?;
            let stride = first.len().div_ceil(points.clamp(2, 48)).max(1);
            let columns: Vec<String> = first
                .iter()
                .step_by(stride)
                .filter_map(|point| Some(format!("{}", number(point.as_array()?.first()?)? as i64)))
                .collect();
            let values: Vec<Vec<f64>> = samples
                .iter()
                .take(points.clamp(1, 12))
                .map(|sample| {
                    sample
                        .get("values")
                        .and_then(Value::as_array)
                        .map(|points| {
                            points
                                .iter()
                                .step_by(stride)
                                .filter_map(|point| {
                                    Some(spec.scaled(number(point.as_array()?.get(1)?)?))
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect();
            Some(json!({
                "kind": "heatmap",
                "title": spec.label.clone().unwrap_or_else(|| "Heatmap".into()),
                "rows": rows,
                "columns": columns,
                "values": values,
                "unit": spec.unit,
            }))
        }
        _ => None,
    }
}

/// Produces the largest snapshot that still fits `budget` bytes.
///
/// Shrinking is preferable to dropping: a thinner series still shows the shape
/// of the data, whereas an EMSGSIZE leaves the widget on its previous value with
/// no indication anything failed.
pub fn widget_fitted(spec: &SourceSpec, result: &Value, budget: usize) -> Option<(Value, bool)> {
    let mut thinned = false;
    for points in [180usize, 90, 45, 20, 10, 5, 2] {
        let widget = widget_within(spec, result, points)?;
        let size = serde_json::to_vec(&widget).map(|bytes| bytes.len()).ok()?;
        if size <= budget {
            return Some((widget, thinned));
        }
        thinned = true;
    }
    None
}

/// An empty widget of the target's own kind.
///
/// Canvas rejects a snapshot whose widget kind differs from the surface it
/// targets, so a placeholder must not default to `stat`: a gauge that goes
/// no-data has to stay a gauge or the mutation is dropped entirely.
pub fn placeholder(kind: &str, label: &str) -> Value {
    match kind {
        "gauge" => json!({"kind": "gauge", "value": 0.0, "label": label}),
        "time_series" => json!({"kind": "time_series", "title": label, "series": []}),
        "heatmap" => {
            json!({"kind": "heatmap", "title": label, "rows": [], "columns": [], "values": []})
        }
        "ranked_bar" => json!({"kind": "ranked_bar", "title": label, "bars": []}),
        "event_log" => json!({"kind": "event_log", "title": label, "entries": []}),
        "rate_bars" => {
            json!({"kind": "rate_bars", "title": label, "objective": 0.0, "windows": []})
        }
        "table" => json!({"kind": "table", "title": label, "columns": [], "rows": []}),
        "state_timeline" => json!({"kind": "state_timeline", "title": label, "lanes": []}),
        "histogram" => json!({"kind": "histogram", "title": label, "buckets": []}),
        _ => json!({"kind": "stat", "label": label, "value": 0.0, "state": "neutral"}),
    }
}

/// The reason a widget is not showing data, in the vocabulary Canvas renders.
/// `unauthorized` is deliberately distinct: it is the only one that should cause
/// a re-authentication rather than a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    Unauthorized,
    Stale,
    NoData,
    NotBacked,
}

impl Unavailable {
    pub fn state(self) -> &'static str {
        match self {
            Self::Unauthorized => "error",
            Self::Stale => "stale",
            Self::NoData | Self::NotBacked => "no_data",
        }
    }
    pub fn detail(self) -> &'static str {
        match self {
            Self::Unauthorized => "FPL session expired; re-authenticating",
            Self::Stale => "Metrics source unreachable",
            Self::NoData => "Query matched no series",
            Self::NotBacked => "No metric backs this source yet",
        }
    }
}

/// Classifies a bridge response. An absent credential and an unreachable server
/// must not collapse into one state: only the first should trigger re-auth.
pub fn classify(result: &Value) -> Option<Unavailable> {
    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    match result.pointer("/error/code").and_then(Value::as_str) {
        Some("authorization_denied") => Some(Unavailable::Unauthorized),
        _ => Some(Unavailable::Stale),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn envelope(
    source_id: &str,
    spec: &SourceSpec,
    result: Option<&Value>,
    sequence: u64,
    now_ms: u64,
    observed_at: &str,
    window: &str,
) -> Result<Value, String> {
    let (owner, stream) = stream_identity(source_id)?;
    let source = format!("prometheus:{source_id}");
    let (state, detail) = match (spec.unavailable.as_deref(), result) {
        (Some(reason), _) => (Unavailable::NotBacked, Some(reason.to_owned())),
        (None, None) => (Unavailable::Stale, None),
        (None, Some(result)) => match classify(result) {
            Some(reason) => (reason, None),
            None => {
                match widget_fitted(spec, result, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD)) {
                    Some((widget, thinned)) => {
                        let _ = thinned;
                        let data = json!({
                            "state": "ready",
                            "source": source,
                            "observed_at": observed_at,
                            "window": window,
                        });
                        return Ok(json!({
                            "schema_version": STREAM_SCHEMA_VERSION,
                            "owner": owner,
                            "stream": stream,
                            "contract": CONTRACT,
                            "sequence": sequence,
                            "emitted_at_ms": now_ms,
                            "payload": {"widget": widget, "data": data},
                        }));
                    }
                    None => (Unavailable::NoData, None),
                }
            }
        },
    };
    // A widget with no fresh sample keeps whatever surface it already has; only
    // the source-health metadata changes, so the layout never flickers.
    Ok(json!({
        "schema_version": STREAM_SCHEMA_VERSION,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": now_ms,
        "payload": {
            "widget": placeholder(&spec.kind, &spec.label.clone().unwrap_or_else(|| source_id.into())),
            "data": {
                "state": state.state(),
                "source": source,
                "observed_at": observed_at,
                "window": window,
                "detail": detail.unwrap_or_else(|| state.detail().to_owned()),
            }
        },
    }))
}

/// Formats epoch seconds as RFC 3339 UTC without pulling in a date library for
/// one field. Civil-date arithmetic only; no timezone or leap-second handling.
pub fn rfc3339(epoch_seconds: u64) -> String {
    let (days, rem) = (epoch_seconds / 86_400, epoch_seconds % 86_400);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut year = 1970;
    let mut day = days as i64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let length = if leap { 366 } else { 365 };
        if day < length {
            break;
        }
        day -= length;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while month < 12 && day >= months[month] {
        day -= months[month];
        month += 1;
    }
    format!(
        "{year:04}-{:02}-{:02}T{hour:02}:{minute:02}:{second:02}Z",
        month + 1,
        day + 1
    )
}

/// Wait between datagrams in one poll's burst.
///
/// AF_UNIX SOCK_DGRAM has no flow control. Canvas drains its stream socket
/// continuously, but a poll emits every source back to back, and that burst can
/// fill the receive buffer faster than it empties: the surplus sends fail with
/// `ENOBUFS` and those snapshots are gone. Losing one is worse than delivering
/// it late, because the widget keeps its previous value and stays `ready` —
/// stale while reading as current, which is the failure `docs/widget-quality.md`
/// rule 2 exists to prevent. At a 30 s poll a few milliseconds per datagram is
/// invisible, so the burst is spread rather than raced.
pub const SEND_SPACING: Duration = Duration::from_millis(3);

/// Wait after the first `ENOBUFS`; each further retry doubles it.
pub const SEND_BACKOFF: Duration = Duration::from_millis(10);

/// Retries after the first attempt. Bounded on purpose: a buffer still full
/// after this is not a burst, and waiting longer would trade a reported loss
/// for an unreported stall.
pub const SEND_RETRIES: u32 = 4;

/// `ENOBUFS` as the host kernel numbers it. std decodes it as
/// `ErrorKind::Uncategorized`, so the raw errno is the only stable way to
/// recognise it, and the BSD-derived kernels and Linux disagree on the number.
#[cfg(any(target_os = "linux", target_os = "android"))]
const ENOBUFS: i32 = 105;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const ENOBUFS: i32 = 55;

/// Whether a send failed because the socket buffer is full — the one failure
/// that clears on its own, since the receiver is draining the whole time.
pub fn is_buffer_full(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ENOBUFS)
}

/// How hard a burst is held back. Injectable so the pacing can be exercised
/// without waiting through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    pub spacing: Duration,
    pub backoff: Duration,
    pub retries: u32,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            spacing: SEND_SPACING,
            backoff: SEND_BACKOFF,
            retries: SEND_RETRIES,
        }
    }
}

/// A snapshot that never reached Canvas. Carries the attempts behind it so the
/// report can distinguish a buffer that stayed full from a Canvas that is not
/// running at all.
#[derive(Debug)]
pub struct SendFailure {
    pub error: io::Error,
    pub attempts: u32,
}

impl std::fmt::Display for SendFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plural = if self.attempts == 1 { "" } else { "s" };
        write!(
            formatter,
            "{} (after {} attempt{plural})",
            self.error, self.attempts
        )
    }
}

impl std::error::Error for SendFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Emits datagrams at a pace the receiver can drain, retrying the transient
/// full buffer a burst provokes.
///
/// The socket and the clock are both injected: the pacing is ordinary logic and
/// is tested as such, with no socket to bind and no wall time to spend.
pub struct PacedSender<E, W> {
    emit: E,
    wait: W,
    pacing: Pacing,
    /// Spacing goes *between* datagrams, so the very first one never waits.
    /// One sender spans every poll, which costs a later poll's first datagram a
    /// spacing it did not need — cheaper than tracking a cycle boundary the
    /// socket itself has no notion of.
    started: bool,
}

impl<E, W> PacedSender<E, W>
where
    E: FnMut(&[u8]) -> io::Result<()>,
    W: FnMut(Duration),
{
    pub fn new(emit: E, wait: W) -> Self {
        Self::with_pacing(emit, wait, Pacing::default())
    }

    pub fn with_pacing(emit: E, wait: W, pacing: Pacing) -> Self {
        Self {
            emit,
            wait,
            pacing,
            started: false,
        }
    }

    /// Places one datagram, reporting how many retries it took — zero on the
    /// ordinary path, which is what the caller stays quiet about.
    pub fn send(&mut self, bytes: &[u8]) -> Result<u32, SendFailure> {
        if self.started {
            (self.wait)(self.pacing.spacing);
        }
        self.started = true;
        let mut backoff = self.pacing.backoff;
        let mut retries = 0;
        loop {
            match (self.emit)(bytes) {
                Ok(()) => return Ok(retries),
                // Every other errno — Canvas absent, socket path gone — fails
                // identically on a retry, so it is reported now instead of
                // several waits from now.
                Err(error) if is_buffer_full(&error) && retries < self.pacing.retries => {
                    (self.wait)(backoff);
                    backoff = backoff.saturating_mul(2);
                    retries += 1;
                }
                Err(error) => {
                    return Err(SendFailure {
                        error,
                        attempts: retries + 1,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn spec(kind: &str) -> SourceSpec {
        SourceSpec {
            kind: kind.into(),
            query: "up".into(),
            window_seconds: None,
            step_seconds: None,
            label: Some("Backends up".into()),
            unit: None,
            series_labels: vec!["instance".into()],
            warn_above: None,
            critical_above: None,
            warn_below: None,
            critical_below: None,
            scale: None,
            unavailable: None,
        }
    }

    fn instant(value: &str) -> Value {
        json!({"ok":true,"data":{"data":{"resultType":"vector","result":[
            {"metric":{"__name__":"up","instance":"a:1"},"value":[1788949087.0, value]}
        ]}}})
    }

    fn range(points: usize) -> Value {
        let values: Vec<Value> = (0..points)
            .map(|i| json!([1788949000.0 + i as f64, format!("{}", i)]))
            .collect();
        json!({"ok":true,"data":{"data":{"resultType":"matrix","result":[
            {"metric":{"__name__":"q","instance":"a:1"},"values": values}
        ]}}})
    }

    #[test]
    fn stream_identity_matches_the_split_canvas_performs() {
        assert_eq!(
            stream_identity("bifrost/health").unwrap(),
            ("bifrost".to_string(), "prometheus-health".to_string())
        );
        for bad in [
            "nohost",
            "Bifrost/health",
            "bifrost/He_alth",
            "/health",
            "bifrost/",
        ] {
            assert!(stream_identity(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stat_carries_value_and_threshold_state() {
        let mut spec = spec("stat");
        spec.critical_below = Some(1.0);
        let low = widget(&spec, &instant("0")).unwrap();
        assert_eq!(low["kind"], "stat");
        assert_eq!(low["value"], 0.0);
        assert_eq!(low["state"], "critical");
        let high = widget(&spec, &instant("3")).unwrap();
        assert_eq!(high["state"], "good");
    }

    #[test]
    fn scale_converts_units_at_the_edge() {
        let mut spec = spec("gauge");
        spec.scale = Some(100.0);
        let scaled = widget(&spec, &instant("0.42")).unwrap();
        assert_eq!(scaled["value"], 42.0);
        // A gauge renders 0..100; a source that overshoots is clamped, not
        // allowed to draw outside the dial.
        let clamped = widget(&spec, &instant("3")).unwrap();
        assert_eq!(clamped["value"], 100.0);
    }

    #[test]
    fn long_ranges_are_fitted_to_the_real_datagram_limit() {
        // macOS caps AF_UNIX datagrams at 2048 bytes, far below Canvas's 64KB
        // constant. Sizing against the constant sends EMSGSIZE, not a snapshot.
        let spec = spec("time_series");
        let budget = MAX_DATAGRAM - ENVELOPE_OVERHEAD;
        let (fitted, thinned) = widget_fitted(&spec, &range(2000), budget).unwrap();
        assert!(thinned, "a 2000-point range must be thinned");
        let bytes = serde_json::to_vec(&fitted).unwrap().len();
        assert!(bytes <= budget, "{bytes} bytes exceeds the {budget} budget");
    }

    #[test]
    fn a_whole_envelope_fits_the_datagram_for_every_kind() {
        for (kind, result) in [
            ("stat", instant("1")),
            ("gauge", instant("1")),
            ("time_series", range(2000)),
            ("heatmap", range(2000)),
            ("ranked_bar", instant("1")),
        ] {
            let event = envelope(
                "bifrost/some-long-source-name",
                &spec(kind),
                Some(&result),
                1,
                0,
                "2026-09-09T00:00:00Z",
                "30m",
            )
            .unwrap();
            let bytes = serde_json::to_vec(&event).unwrap().len();
            assert!(
                bytes <= MAX_DATAGRAM,
                "{kind}: {bytes} bytes exceeds {MAX_DATAGRAM}"
            );
        }
    }

    #[test]
    fn ranked_bars_are_ordered_and_bounded() {
        let spec = spec("ranked_bar");
        let result = json!({"ok":true,"data":{"data":{"result":[
            {"metric":{"instance":"low"},"value":[0.0,"1"]},
            {"metric":{"instance":"high"},"value":[0.0,"9"]}
        ]}}});
        let ranked = widget(&spec, &result).unwrap();
        assert_eq!(ranked["bars"][0]["label"], "high");
        assert_eq!(ranked["bars"][1]["label"], "low");
    }

    #[test]
    fn an_empty_result_is_no_data_rather_than_a_fabricated_zero() {
        let empty = json!({"ok":true,"data":{"data":{"result":[]}}});
        assert!(widget(&spec("stat"), &empty).is_none());
        let event = envelope(
            "bifrost/health",
            &spec("stat"),
            Some(&empty),
            1,
            0,
            "t",
            "now",
        )
        .unwrap();
        assert_eq!(event["payload"]["data"]["state"], "no_data");
        assert_eq!(
            event["payload"]["data"]["detail"],
            "Query matched no series"
        );
    }

    #[test]
    fn expired_authorization_is_distinct_from_an_unreachable_source() {
        let denied = json!({"ok":false,"error":{"code":"authorization_denied"}});
        let down = json!({"ok":false,"error":{"code":"owner_unavailable"}});
        assert_eq!(classify(&denied), Some(Unavailable::Unauthorized));
        assert_eq!(classify(&down), Some(Unavailable::Stale));
        assert_eq!(classify(&json!({"ok":true})), None);
        let event = envelope(
            "bifrost/health",
            &spec("stat"),
            Some(&denied),
            1,
            0,
            "t",
            "now",
        )
        .unwrap();
        assert_eq!(event["payload"]["data"]["state"], "error");
    }

    #[test]
    fn a_source_no_metric_backs_says_so_instead_of_loading_forever() {
        let mut spec = spec("stat");
        spec.unavailable = Some("Bifrost exposes no cache metric".into());
        let event = envelope("bifrost/cache", &spec, None, 1, 0, "t", "5m").unwrap();
        assert_eq!(event["payload"]["data"]["state"], "no_data");
        assert_eq!(
            event["payload"]["data"]["detail"],
            "Bifrost exposes no cache metric"
        );
    }

    #[test]
    fn a_no_data_placeholder_keeps_the_target_widget_kind() {
        // Canvas drops a mutation whose kind differs from the surface, so a
        // gauge going no-data must stay a gauge.
        let empty = json!({"ok":true,"data":{"data":{"result":[]}}});
        for kind in [
            "stat",
            "gauge",
            "time_series",
            "heatmap",
            "ranked_bar",
            "event_log",
            "rate_bars",
        ] {
            let mut s = spec(kind);
            s.kind = kind.into();
            let event = envelope("bifrost/x", &s, Some(&empty), 1, 0, "t", "now").unwrap();
            assert_eq!(event["payload"]["widget"]["kind"], kind, "{kind}");
            assert_eq!(event["payload"]["data"]["state"], "no_data");
        }
    }

    #[test]
    fn envelope_matches_the_contract_canvas_validates() {
        let event = envelope(
            "bifrost/health",
            &spec("stat"),
            Some(&instant("2")),
            7,
            99,
            "2026-09-09T00:00:00Z",
            "now",
        )
        .unwrap();
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["owner"], "bifrost");
        assert_eq!(event["stream"], "prometheus-health");
        assert_eq!(event["contract"], "canvas.widget.snapshot.v1");
        assert_eq!(event["sequence"], 7);
        assert_eq!(event["emitted_at_ms"], 99);
        assert_eq!(
            event["payload"]["data"]["source"],
            "prometheus:bifrost/health"
        );
        assert_eq!(event["payload"]["widget"]["value"], 2.0);
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_788_949_087), "2026-09-09T10:18:07Z");
        // A leap day must not shift the calendar by one.
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    /// Stands in for the socket and the clock: counts sends and records every
    /// wait the pacer asks for, so a burst can be replayed instantly.
    #[derive(Default)]
    struct Recorder {
        sends: Cell<u32>,
        waits: RefCell<Vec<Duration>>,
    }

    impl Recorder {
        /// A send that fails `failures` times with `error`, then succeeds.
        fn failing(&self, failures: u32, error: i32) -> impl FnMut(&[u8]) -> io::Result<()> {
            move |_bytes| {
                let attempt = self.sends.get();
                self.sends.set(attempt + 1);
                if attempt < failures {
                    Err(io::Error::from_raw_os_error(error))
                } else {
                    Ok(())
                }
            }
        }
        fn wait(&self) -> impl FnMut(Duration) {
            move |waited| self.waits.borrow_mut().push(waited)
        }
        fn waits(&self) -> Vec<Duration> {
            self.waits.borrow().clone()
        }
    }

    fn pacing() -> Pacing {
        Pacing {
            spacing: Duration::from_millis(3),
            backoff: Duration::from_millis(10),
            retries: 4,
        }
    }

    #[test]
    fn the_errno_the_pacer_retries_is_this_platforms_full_buffer() {
        // The retry hinges on one number that differs between kernels, so check
        // it against the host's own errno table rather than trusting a constant.
        let full = io::Error::from_raw_os_error(ENOBUFS);
        assert!(
            full.to_string().to_lowercase().contains("no buffer space"),
            "{ENOBUFS} is not this platform's ENOBUFS: {full}"
        );
        assert!(is_buffer_full(&full));
        assert!(!is_buffer_full(&io::Error::from(
            io::ErrorKind::ConnectionRefused
        )));
    }

    #[test]
    fn a_burst_is_paced_behind_the_previous_datagram() {
        let log = Recorder::default();
        let pacing = pacing();
        let mut sender = PacedSender::with_pacing(log.failing(0, ENOBUFS), log.wait(), pacing);
        let burst = 21;
        for _ in 0..burst {
            assert_eq!(sender.send(b"{}").unwrap(), 0);
        }
        // The first datagram leaves immediately; every later one waits behind
        // the one before it, which is the whole point at a 30 s poll.
        assert_eq!(log.sends.get(), burst);
        assert_eq!(log.waits().len() as u32, burst - 1);
        assert!(log.waits().iter().all(|waited| *waited == pacing.spacing));
    }

    #[test]
    fn a_full_socket_buffer_is_retried_until_it_drains() {
        let log = Recorder::default();
        let pacing = pacing();
        let mut sender = PacedSender::with_pacing(log.failing(2, ENOBUFS), log.wait(), pacing);
        assert_eq!(sender.send(b"{}").unwrap(), 2);
        assert_eq!(log.sends.get(), 3, "the snapshot must actually be resent");
        let waits = log.waits();
        assert_eq!(waits.len(), 2, "one wait per retry, none before the first");
        assert_eq!(waits[0], pacing.backoff);
        assert!(waits[1] > waits[0], "backoff must grow, not repeat");
    }

    #[test]
    fn a_buffer_that_never_drains_is_reported_rather_than_retried_forever() {
        let log = Recorder::default();
        let pacing = pacing();
        let mut sender =
            PacedSender::with_pacing(log.failing(u32::MAX, ENOBUFS), log.wait(), pacing);
        let failure = sender.send(b"{}").unwrap_err();
        assert_eq!(failure.attempts, pacing.retries + 1);
        assert_eq!(log.sends.get(), pacing.retries + 1);
        assert!(is_buffer_full(&failure.error));
        // A retry must never make a persistent loss quiet: the report says how
        // hard it tried, because the widget is now stale but still reads ready.
        assert!(
            failure
                .to_string()
                .contains(&format!("{} attempts", failure.attempts)),
            "{failure}"
        );
    }

    #[test]
    fn an_error_that_is_not_a_full_buffer_is_reported_at_once() {
        // Canvas not running fails identically on every attempt; backing off
        // would only delay the report, once per widget, every poll.
        let log = Recorder::default();
        let mut sender =
            PacedSender::with_pacing(log.failing(u32::MAX, econnrefused()), log.wait(), pacing());
        let failure = sender.send(b"{}").unwrap_err();
        assert_eq!(failure.attempts, 1);
        assert_eq!(log.sends.get(), 1);
        assert!(log.waits().is_empty());
        assert!(!is_buffer_full(&failure.error));
    }

    /// `ECONNREFUSED`, what a socket with no Canvas behind it gives. Any
    /// non-`ENOBUFS` errno would do; this is the one seen in practice.
    fn econnrefused() -> i32 {
        if cfg!(any(target_os = "linux", target_os = "android")) {
            111
        } else {
            61
        }
    }

    #[test]
    fn the_send_budget_stays_small_against_the_shortest_poll() {
        // Pacing protects a poll; it must never be able to outlast one. Both
        // halves are checked against the configured floor rather than a literal
        // so the constants cannot drift apart unnoticed.
        let pacing = Pacing::default();
        let interval = Duration::from_secs(MIN_POLL_SECONDS);
        let mut worst = Duration::ZERO;
        let mut backoff = pacing.backoff;
        for _ in 0..pacing.retries {
            worst += backoff;
            backoff *= 2;
        }
        assert!(
            worst < interval / 10,
            "a stuck buffer holds the burst for {worst:?}"
        );
        // Ordinary spacing has to be invisible even for a view far larger than
        // the 21 sources shipped today.
        assert!(pacing.spacing * 64 < interval / 10);
    }
}
