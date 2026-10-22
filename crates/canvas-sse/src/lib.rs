//! Turns an SSE subscription into Canvas widget snapshots.
//!
//! The same programme as `canvas-metrics` over a different source. This
//! connector holds no credential either: it names an owner and a topic, the
//! transport attaches the owner's bearer, and only parsed frames come back.
//! Everything here is a pure transform over those frames, so the mapping is
//! testable without a network or a keyring.
//!
//! One difference shapes the whole crate. A poll returns the entire answer; a
//! subscription returns increments. So the connector keeps a bounded [`Window`]
//! of recent observations per source and re-renders the whole widget from it on
//! every tick. A snapshot is therefore self-contained, a dropped frame costs
//! detail rather than correctness, and a stream that has gone quiet still emits
//! — reporting its own freshness instead of leaving the last value looking live.
//!
//! The subscription's own lifecycle — connect, resume, frame, back off,
//! reconnect — is [`SseSubscription`], above the [`Connector`] boundary and
//! driven by a `now_ms` it is handed rather than a clock it reads. A real
//! transport supplies sockets and credentials; every state it can be in is
//! reachable in a test without either.
use canvas_metrics::placeholder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, time::Duration};

pub use canvas_metrics::{
    CONTRACT, ENVELOPE_OVERHEAD, MAX_DATAGRAM, MAX_STREAM_MESSAGE, STREAM_SCHEMA_VERSION, rfc3339,
};

/// Observations retained per source. Bounds memory independently of what fits
/// in a datagram (widget-quality rule 6); the datagram budget usually bites
/// first, but memory must not depend on that.
pub const RETAIN_DEFAULT: usize = 64;
/// Characters of a rendered string before the fitting ladder starts shrinking.
const TEXT_DEFAULT: usize = 160;
/// Lanes a timeline shows. More than this on a wall is a texture, not a reading.
const MAX_LANES: usize = 8;
/// A state that began this second still has to be visible. This is a render
/// floor, not a claim that the state has held for a second.
const MIN_SEGMENT_SECONDS: f64 = 1.0;
/// Only used by a spec that failed [`SourceSpec::validate`], which rejects a
/// timeline with no window.
const SPAN_FALLBACK_SECONDS: u64 = 3600;

/// Frame fields read when a source does not name its own, in order. Elmer's and
/// the MCP contracts both spell the same idea several ways; taking the first hit
/// keeps one mapping working across all of them.
const SUBJECT_KEYS: [&str; 5] = ["subject", "workflow", "capability", "name", "id"];
const MESSAGE_KEYS: [&str; 4] = ["message", "summary", "detail", "text"];
const LEVEL_KEYS: [&str; 4] = ["level", "severity", "outcome", "state"];
const TIME_KEYS: [&str; 4] = ["at_ms", "timestamp", "time", "at"];

/// Widget kinds this connector renders. A view may only point an `sse:` source
/// at one of these; anything else is a config error, not a silent placeholder.
pub const SUPPORTED_KINDS: [&str; 3] = ["event_log", "table", "state_timeline"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub schema_version: u32,
    /// How often the connector re-renders and emits, whether or not a frame
    /// arrived. A subscription that has gone quiet must still say so.
    #[serde(default = "default_tick")]
    pub tick_seconds: u64,
    /// Logical widget source id (`elmers/activity`) to its subscription.
    pub sources: BTreeMap<String, SourceSpec>,
}

fn default_tick() -> u64 {
    5
}

/// How one logical widget source is produced. The topic lives here, host-side,
/// deliberately not in the view JSON: views are shared and reconfigurable, and
/// an endpoint is an operational detail that must not enter a saved layout.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// Widget kind this feeds; see [`SUPPORTED_KINDS`].
    pub kind: String,
    /// Topic the transport subscribes to on the owner's behalf.
    pub topic: String,
    /// SSE `event:` names to keep. Empty keeps every frame.
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Table headings, from the vocabulary [`cell`] can answer.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub fields: Fields,
    #[serde(default)]
    pub retain: Option<usize>,
    /// Horizon a timeline spans and a table ages against. Absent means the
    /// source is a live tail with no horizon.
    #[serde(default)]
    pub window_seconds: Option<u64>,
    /// Silence after which the widget is reported stale rather than ready. A
    /// subscription cannot tell "nothing happened" from "nothing arrived", so
    /// the operator declares how long quiet is still healthy.
    #[serde(default)]
    pub stale_after_seconds: Option<u64>,
    /// Recorded when a view declares a source no stream backs yet, so the widget
    /// says why instead of showing an indefinite Loading.
    #[serde(default)]
    pub unavailable: Option<String>,
}

/// Frame keys to read for each part of an observation, in order, first hit wins.
/// Empty falls back to the built-in candidates.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    #[serde(default)]
    pub subject: Vec<String>,
    #[serde(default)]
    pub message: Vec<String>,
    #[serde(default)]
    pub level: Vec<String>,
    #[serde(default)]
    pub timestamp: Vec<String>,
}

impl SourceSpec {
    pub fn retention(&self) -> usize {
        self.retain.unwrap_or(RETAIN_DEFAULT).clamp(1, 512)
    }

    pub fn span_seconds(&self) -> u64 {
        self.window_seconds.unwrap_or(SPAN_FALLBACK_SECONDS).max(1)
    }

    pub fn title(&self, source_id: &str) -> String {
        self.label.clone().unwrap_or_else(|| source_id.to_owned())
    }

    fn headings(&self) -> Vec<String> {
        if self.columns.is_empty() {
            ["Item", "State", "Age"].map(String::from).to_vec()
        } else {
            self.columns.clone()
        }
    }

    /// Rejects a source the connector could only serve by inventing something.
    ///
    /// Called before the first tick, so a mistyped kind or an unanswerable
    /// column fails at startup rather than as a widget that never populates.
    pub fn validate(&self, source_id: &str) -> Result<(), String> {
        if !SUPPORTED_KINDS.contains(&self.kind.as_str()) {
            return Err(format!(
                "{source_id}: sse cannot feed a {} widget; supported: {}",
                self.kind,
                SUPPORTED_KINDS.join(", ")
            ));
        }
        if self.topic.is_empty() {
            return Err(format!("{source_id}: source has no topic"));
        }
        if self.kind == "state_timeline" && self.window_seconds.is_none() {
            return Err(format!(
                "{source_id}: a state timeline needs window_seconds to span"
            ));
        }
        if self.kind == "table" {
            let headings = self.headings();
            if headings.len() > 6 {
                return Err(format!(
                    "{source_id}: {} columns is unreadable",
                    headings.len()
                ));
            }
            let probe = Observation {
                at_ms: 0,
                subject: String::new(),
                message: String::new(),
                level: Level::Neutral,
            };
            for heading in headings {
                if cell(&heading, &probe, 0, 8).is_none() {
                    return Err(format!("{source_id}: no sse column answers \"{heading}\""));
                }
            }
        }
        Ok(())
    }
}

/// Canvas derives `owner` and `stream` from the widget's own source string, so
/// the emitter must produce exactly the same split or the snapshot lands on no
/// widget at all. The rule being mirrored is `widget_source` in
/// `crates/canvas/src/operational_views.rs`.
pub fn stream_identity(source_id: &str) -> Result<(String, String), String> {
    let (owner, name) = source_id
        .split_once('/')
        .ok_or_else(|| format!("source id must be owner/name: {source_id}"))?;
    let stream = format!("sse-{name}");
    let valid = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    // `name` is checked separately: an empty one still yields the valid-looking
    // stream "sse-", which would bind to no widget while looking correct.
    if !valid(owner) || !valid(name) || !valid(&stream) {
        return Err(format!(
            "source id is not a valid stream identity: {source_id}"
        ));
    }
    Ok((owner.to_owned(), stream))
}

/// One SSE frame as the wire defines it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
}

/// Parses one blank-line-delimited SSE block. `None` when the block carries no
/// `data:` line, which is what a comment keepalive looks like.
pub fn parse_block(block: &str) -> Option<Frame> {
    let mut frame = Frame::default();
    let mut data: Vec<&str> = Vec::new();
    for line in block.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, rest) = line.split_once(':').unwrap_or((line, ""));
        // The wire allows exactly one optional space after the colon; any
        // further whitespace is part of the value.
        let value = rest.strip_prefix(' ').unwrap_or(rest);
        match field {
            "event" => frame.event = Some(value.to_owned()),
            "id" => frame.id = Some(value.to_owned()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    frame.data = data.join("\n");
    Some(frame)
}

/// Splits a whole stream chunk into frames, dropping a trailing partial block.
///
/// A live subscription uses [`FrameBuffer`] instead, which keeps that remainder;
/// this is the one-shot form, for a chunk already known to be complete.
pub fn parse_stream(text: &str) -> Vec<Frame> {
    let mut buffer = FrameBuffer::default();
    buffer.push(text.as_bytes());
    buffer.take(usize::MAX)
}

/// Bytes one SSE block may occupy before it is refused rather than parsed. A
/// block this large cannot reach a widget in any case — a whole snapshot must
/// fit [`MAX_DATAGRAM`] — and a stream is a thing another process controls.
pub const MAX_EVENT_BYTES: usize = 16 * 1024;

/// Unparsed stream bytes held between drains. Reached only when a producer
/// outruns the tick; the oldest blocks go first, which is also what keeps the
/// reader near the live edge instead of working through a backlog.
pub const MAX_BUFFERED_BYTES: usize = 256 * 1024;

/// Frames parsed in one drain. Bounds the work a single tick can be made to do;
/// the remainder stays buffered and is read on the next one.
pub const MAX_FRAMES_PER_DRAIN: usize = 128;

/// Where the first blank line in `pending` is: the bytes before it, and where
/// the next block starts.
///
/// The wire ends a line with `\n` or `\r\n` and ends an event with a blank line,
/// so the scan looks for one line ending immediately followed by another. A
/// chunk that ends mid-terminator yields `None` and waits for more bytes, which
/// is the whole reason this is a buffer and not a `split`.
fn split_block(pending: &[u8]) -> Option<(usize, usize)> {
    let ending = |at: usize| match pending.get(at)? {
        b'\n' => Some(at + 1),
        b'\r' if pending.get(at + 1) == Some(&b'\n') => Some(at + 2),
        _ => None,
    };
    (0..pending.len()).find_map(|at| {
        let after = ending(at).and_then(ending)?;
        Some((at, after))
    })
}

/// Accumulates stream bytes and hands out whole frames.
///
/// A read almost never ends on an event boundary, so the remainder is kept and
/// prepended to the next chunk. Bytes are buffered rather than text because a
/// read can also split a multi-byte character; a complete block never can, so
/// decoding happens at the boundary.
#[derive(Debug, Default)]
pub struct FrameBuffer {
    pending: Vec<u8>,
    discarded: u64,
}

impl FrameBuffer {
    /// Blocks refused for size, ever. Counted rather than logged here so the
    /// caller decides how to be loud about it.
    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    pub fn buffered(&self) -> usize {
        self.pending.len()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while self.pending.len() > MAX_BUFFERED_BYTES {
            self.discarded += 1;
            match split_block(&self.pending) {
                Some((_, next)) => drop(self.pending.drain(..next)),
                // One partial block already over the ceiling: there is no
                // boundary to resynchronize on until more bytes arrive.
                None => self.pending.clear(),
            }
        }
    }

    /// Up to `limit` frames, oldest first. Blocks carrying no `data:` — comment
    /// keepalives — are consumed without counting against the limit, because
    /// they are not events and a stream of them must still make progress.
    pub fn take(&mut self, limit: usize) -> Vec<Frame> {
        let mut frames = Vec::new();
        while frames.len() < limit {
            let Some((end, next)) = split_block(&self.pending) else {
                break;
            };
            let block: Vec<u8> = self.pending.drain(..next).collect();
            let block = &block[..end];
            if block.len() > MAX_EVENT_BYTES {
                self.discarded += 1;
                continue;
            }
            if let Some(frame) = parse_block(&String::from_utf8_lossy(block)) {
                frames.push(frame);
            }
        }
        frames
    }
}

/// The four states Canvas renders. Mapping onto them is deliberately narrow:
/// an unrecognized severity becomes `Neutral`, never promoted to `Critical` or
/// demoted to `Good`, because both mistakes are read as a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Neutral,
    Good,
    Warning,
    Critical,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Good => "good",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ok" | "good" | "info" | "success" | "succeeded" | "complete" | "completed"
            | "healthy" | "pass" | "passed" | "up" => Self::Good,
            "warn" | "warning" | "degraded" | "retry" | "retrying" | "pending" | "waiting"
            | "throttled" | "partial" => Self::Warning,
            "error" | "err" | "fail" | "failed" | "failure" | "critical" | "fatal" | "denied"
            | "down" | "aborted" => Self::Critical,
            _ => Self::Neutral,
        }
    }
}

/// One frame, normalized. Every widget kind renders from these, so a new kind
/// needs no new parsing and a new event shape needs no new widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub at_ms: u64,
    pub subject: String,
    pub message: String,
    pub level: Level,
}

fn pick<'a>(data: &'a Value, configured: &[String], fallback: &[&str]) -> Option<&'a Value> {
    configured
        .iter()
        .map(String::as_str)
        .chain(fallback.iter().copied())
        .find_map(|key| data.get(key))
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Epoch seconds and epoch milliseconds are both common on these streams and
/// nothing labels which one it sent. They are eleven orders of magnitude apart,
/// so the magnitude decides: below this, the number cannot be milliseconds
/// without dating to 1973.
const MILLIS_THRESHOLD: f64 = 1e11;

fn instant(value: &Value) -> Option<u64> {
    let raw = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    if raw <= 0.0 {
        return None;
    }
    Some(if raw < MILLIS_THRESHOLD {
        (raw * 1000.0) as u64
    } else {
        raw as u64
    })
}

/// Normalizes one frame. `None` when the source filters this event type out.
///
/// `received_ms` is the arrival time, used when the payload carries no usable
/// timestamp of its own. That is a substitution, not an invention: the frame
/// really did arrive then, and the alternative is a zero epoch that renders as
/// 1970.
pub fn observe(spec: &SourceSpec, frame: &Frame, received_ms: u64) -> Option<Observation> {
    if !spec.event_types.is_empty() {
        let event = frame.event.as_deref().unwrap_or_default();
        if !spec.event_types.iter().any(|kept| kept == event) {
            return None;
        }
    }
    let named = frame.event.clone().unwrap_or_else(|| "event".into());
    // A payload that is not a JSON object is still a real payload; rendering the
    // raw line is honest, dropping it silently is not.
    let Some(data) = serde_json::from_str::<Value>(&frame.data)
        .ok()
        .filter(Value::is_object)
    else {
        return Some(Observation {
            at_ms: received_ms,
            subject: named.clone(),
            message: frame.data.clone(),
            level: Level::parse(&named),
        });
    };
    let subject = pick(&data, &spec.fields.subject, &SUBJECT_KEYS)
        .and_then(text_of)
        .unwrap_or_else(|| named.clone());
    let message = pick(&data, &spec.fields.message, &MESSAGE_KEYS)
        .and_then(text_of)
        .unwrap_or_else(|| subject.clone());
    let level = pick(&data, &spec.fields.level, &LEVEL_KEYS)
        .and_then(text_of)
        .map_or(Level::Neutral, |raw| Level::parse(&raw));
    let at_ms = pick(&data, &spec.fields.timestamp, &TIME_KEYS)
        .and_then(instant)
        .unwrap_or(received_ms);
    Some(Observation {
        at_ms,
        subject,
        message,
        level,
    })
}

/// A bounded, arrival-ordered record of what a subscription has delivered.
///
/// This is the connector's only mutable state and the reason the transport can
/// be swapped without touching a transform.
#[derive(Debug, Clone)]
pub struct Window {
    observations: Vec<Observation>,
    retain: usize,
}

impl Window {
    pub fn new(retain: usize) -> Self {
        Self {
            observations: Vec::new(),
            retain: retain.max(1),
        }
    }

    pub fn push(&mut self, observation: Observation) {
        self.observations.push(observation);
        if self.observations.len() > self.retain {
            let excess = self.observations.len() - self.retain;
            self.observations.drain(..excess);
        }
    }

    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    /// The newest event time held, which is what "observed at" means for a
    /// subscription: the emit time says nothing about the data's age.
    pub fn latest_ms(&self) -> Option<u64> {
        self.observations.iter().map(|o| o.at_ms).max()
    }
}

fn clip(text: &str, chars: usize) -> String {
    let chars = chars.max(4);
    let mut clipped: String = text.chars().take(chars).collect();
    if text.chars().count() > chars {
        clipped.push('…');
    }
    clipped
}

fn age(from_ms: u64, now_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(from_ms) / 1000;
    match seconds {
        seconds if seconds < 60 => format!("{seconds}s"),
        seconds if seconds < 3_600 => format!("{}m", seconds / 60),
        seconds if seconds < 86_400 => format!("{}h", seconds / 3_600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

/// Time of day, dated only when the event is not from today.
///
/// The full date is already on the snapshot's `observed_at`, and bytes are
/// scarce (rule 6) — but "14:02" on a day-old line is a lie by omission, so an
/// older event keeps enough date to be read correctly.
fn stamp(at_ms: u64, now_ms: u64) -> String {
    let at = rfc3339(at_ms / 1000);
    let today = rfc3339(now_ms / 1000);
    if at[..10] == today[..10] {
        at[11..19].to_owned()
    } else {
        at[5..16].replace('T', " ")
    }
}

/// The table cells a heading can ask for.
///
/// Naming the vocabulary is what lets [`SourceSpec::validate`] reject a column
/// the connector would otherwise have to fill with an empty string — which
/// reads as "nothing is wrong" rather than "nobody answered".
fn cell(column: &str, observation: &Observation, now_ms: u64, chars: usize) -> Option<String> {
    match column.trim().to_ascii_lowercase().as_str() {
        "item" | "subject" | "name" | "workflow" | "capability" => {
            Some(clip(&observation.subject, chars))
        }
        "state" | "status" | "level" | "severity" => Some(observation.level.tag().to_owned()),
        "age" => Some(age(observation.at_ms, now_ms)),
        "time" | "when" => Some(stamp(observation.at_ms, now_ms)),
        "message" | "detail" | "summary" => Some(clip(&observation.message, chars)),
        _ => None,
    }
}

fn newest_first(window: &Window, limit: usize) -> Vec<&Observation> {
    let mut recent: Vec<&Observation> = window.observations().iter().collect();
    recent.sort_by_key(|observation| std::cmp::Reverse(observation.at_ms));
    recent.truncate(limit.max(1));
    recent
}

/// Builds the `widget` half of a snapshot payload. `None` when the window holds
/// nothing this kind can draw, which is a no-data state rather than an error.
pub fn widget(spec: &SourceSpec, source_id: &str, window: &Window, now_ms: u64) -> Option<Value> {
    widget_within(
        spec,
        source_id,
        window,
        now_ms,
        RETAIN_DEFAULT,
        TEXT_DEFAULT,
    )
    .map(|(widget, _)| widget)
}

/// `entries` caps rows, log lines, lanes and segments; `chars` caps every
/// rendered string. Both are needed to shrink a snapshot to fit the datagram:
/// one four-kilobyte message overflows even a single-entry log.
///
/// The second return is `complete`: whether every observation the window holds
/// is represented. Clipped text needs no such flag — the ellipsis discloses
/// itself, in the widget, where it is read — but a log showing eight of two
/// hundred lines looks exactly like a log of eight lines, so the caller has to
/// be able to say so.
pub fn widget_within(
    spec: &SourceSpec,
    source_id: &str,
    window: &Window,
    now_ms: u64,
    entries: usize,
    chars: usize,
) -> Option<(Value, bool)> {
    if window.is_empty() {
        return None;
    }
    let title = clip(&spec.title(source_id), chars.min(48));
    let held = window.observations().len();
    match spec.kind.as_str() {
        "event_log" => {
            let recent = newest_first(window, entries);
            let complete = recent.len() == held;
            let lines: Vec<Value> = recent
                .into_iter()
                .map(|observation| {
                    json!({
                        "timestamp": stamp(observation.at_ms, now_ms),
                        "message": clip(&observation.message, chars),
                        "level": observation.level.tag(),
                    })
                })
                .collect();
            Some((
                json!({"kind": "event_log", "title": title, "entries": lines}),
                complete,
            ))
        }
        "table" => {
            let headings = spec.headings();
            let recent = newest_first(window, entries);
            let complete = recent.len() == held;
            let rows: Vec<Value> = recent
                .into_iter()
                .map(|observation| {
                    let cells: Vec<String> = headings
                        .iter()
                        // Validation guarantees every heading answers; an
                        // unvalidated spec loses the cell, never the row shape.
                        .map(|heading| {
                            cell(heading, observation, now_ms, chars).unwrap_or_default()
                        })
                        .collect();
                    json!(cells)
                })
                .collect();
            let columns: Vec<String> = headings
                .iter()
                .map(|heading| clip(heading, chars.min(24)))
                .collect();
            Some((
                json!({"kind": "table", "title": title, "columns": columns, "rows": rows}),
                complete,
            ))
        }
        "state_timeline" => {
            let span = spec.span_seconds();
            let horizon_ms = now_ms.saturating_sub(span * 1_000);
            let mut grouped: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
            for observation in window.observations() {
                if observation.at_ms >= horizon_ms {
                    grouped
                        .entry(observation.subject.as_str())
                        .or_default()
                        .push(observation);
                }
            }
            // Lanes that do not fit are dropped by staleness, not alphabetically:
            // the capabilities that moved most recently are the ones being read.
            let mut ordered: Vec<(&str, Vec<&Observation>)> = grouped.into_iter().collect();
            ordered.sort_by(|left, right| {
                let recency = |marks: &[&Observation]| {
                    marks
                        .iter()
                        .map(|mark| mark.at_ms)
                        .max()
                        .unwrap_or_default()
                };
                recency(&right.1)
                    .cmp(&recency(&left.1))
                    .then_with(|| left.0.cmp(right.0))
            });
            let lane_cap = entries.clamp(1, MAX_LANES);
            let segment_cap = entries.max(1);
            // Events older than the declared window are outside what this widget
            // claims to show, so dropping them is not an omission.
            let complete = ordered.len() <= lane_cap
                && ordered.iter().all(|(_, marks)| marks.len() <= segment_cap);
            let span = span as f64;
            let offset = |at_ms: u64| (at_ms.saturating_sub(horizon_ms) as f64 / 1000.0).min(span);
            let lanes: Vec<Value> = ordered
                .into_iter()
                .take(lane_cap)
                .filter_map(|(subject, mut marks)| {
                    marks.sort_by_key(|observation| observation.at_ms);
                    // A lane keeps its most recent states: the tail is what a
                    // capability is doing now, and the head is already history.
                    if marks.len() > segment_cap {
                        let excess = marks.len() - segment_cap;
                        marks.drain(..excess);
                    }
                    let segments: Vec<Value> = marks
                        .iter()
                        .enumerate()
                        .filter_map(|(index, observation)| {
                            let mut start = offset(observation.at_ms);
                            let end = match marks.get(index + 1) {
                                Some(next) => offset(next.at_ms),
                                // The newest state runs to now by definition.
                                None => span,
                            };
                            if index + 1 == marks.len() {
                                start = start.min(span - MIN_SEGMENT_SECONDS).max(0.0);
                            } else if end <= start {
                                // A state superseded within the same tick had no
                                // duration; drawing one would invent it.
                                return None;
                            }
                            Some(json!({
                                "start": start,
                                "end": end,
                                "state": observation.level.tag(),
                                "label": clip(&observation.message, chars.min(32)),
                            }))
                        })
                        .collect();
                    if segments.is_empty() {
                        return None;
                    }
                    Some(json!({"label": clip(subject, chars.min(32)), "segments": segments}))
                })
                .collect();
            if lanes.is_empty() {
                return None;
            }
            Some((
                json!({"kind": "state_timeline", "title": title, "lanes": lanes}),
                complete,
            ))
        }
        _ => None,
    }
}

/// Produces the largest snapshot that still fits `budget` bytes, and whether
/// anything was left out of it.
///
/// Shrinking is preferable to dropping: fewer, shorter lines still show what is
/// happening, whereas an EMSGSIZE leaves the widget on its previous value with
/// no indication anything failed.
pub fn widget_fitted(
    spec: &SourceSpec,
    source_id: &str,
    window: &Window,
    now_ms: u64,
    budget: usize,
) -> Option<(Value, bool)> {
    for (entries, chars) in [
        (RETAIN_DEFAULT, TEXT_DEFAULT),
        (32, 120),
        (16, 96),
        (8, 72),
        (4, 56),
        (2, 40),
        (1, 24),
    ] {
        let (widget, complete) = widget_within(spec, source_id, window, now_ms, entries, chars)?;
        let size = serde_json::to_vec(&widget).map(|bytes| bytes.len()).ok()?;
        if size <= budget {
            return Some((widget, !complete));
        }
    }
    None
}

/// The reason a widget is not showing fresh data, in the vocabulary Canvas
/// renders. `Disconnected` is the one a poller has no equivalent for and the one
/// that matters most here: a subscription that is not established looks exactly
/// like a quiet stream unless it says otherwise. `Unauthorized` is the only one
/// that should cause a re-authentication rather than a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    Unauthorized,
    Disconnected,
    /// The stream was open and ended, and the connector is waiting to try again.
    /// Distinct from `Disconnected` because a viewer reading a gap needs to know
    /// whether anything is being done about it.
    Reconnecting,
    /// Reconnects were attempted to the bound and none held.
    Unreachable,
    Stale,
    NoData,
    NotBacked,
}

impl Unavailable {
    /// One of `canvas_widgets::WidgetDataState`'s own tags. A tag outside that
    /// vocabulary fails to deserialize and the whole snapshot is discarded, so
    /// `Reconnecting` reports as `disconnected` and says the rest in its detail.
    pub fn state(self) -> &'static str {
        match self {
            Self::Unauthorized | Self::Unreachable => "error",
            Self::Disconnected | Self::Reconnecting => "disconnected",
            Self::Stale => "stale",
            Self::NoData | Self::NotBacked => "no_data",
        }
    }
    pub fn detail(self) -> &'static str {
        match self {
            Self::Unauthorized => "FPL session expired; re-authenticating",
            Self::Disconnected => "Not subscribed to this stream",
            Self::Reconnecting => "Stream ended; reconnecting",
            Self::Unreachable => "Stream unreachable; reconnects exhausted, retrying later",
            Self::Stale => "No event within the freshness window",
            Self::NoData => "Subscribed; no event has arrived yet",
            Self::NotBacked => "No stream backs this source yet",
        }
    }
}

/// What a transport handed back this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Frames received since the last call, possibly none.
    Frames(Vec<Frame>),
    /// The subscription is not delivering, and why.
    Interrupted(Unavailable),
}

/// What the emit loop reads each tick.
///
/// `now_ms` is passed rather than read, so a subscription's lifecycle — every
/// backoff, every deadline — is exercised in a test without a clock or a sleep.
pub trait Subscription {
    /// Frames delivered since the last call. Must not block past the tick.
    fn drain(&mut self, topic: &str, now_ms: u64) -> Delivery;
}

/// The transport that exists today.
///
/// It reports `Disconnected` for every topic, which is the truth: nothing is
/// subscribed yet. Widgets fed by it say so instead of sitting on Loading
/// forever (widget-quality rule 3). It is deliberately not an always-failing
/// [`Connector`]: that would report `Reconnecting`, and claiming to be
/// reconnecting when no transport is compiled in is exactly the kind of
/// plausible lie this crate exists to avoid.
#[derive(Debug, Default, Clone, Copy)]
pub struct Unsubscribed;

impl Subscription for Unsubscribed {
    fn drain(&mut self, _topic: &str, _now_ms: u64) -> Delivery {
        Delivery::Interrupted(Unavailable::Disconnected)
    }
}

/// One live byte stream. Dropping it closes the connection.
pub trait EventStream {
    /// Bytes that have arrived since the last call. Empty means open and quiet,
    /// which is not the same as ended. `Err` ends the connection and says why —
    /// `Disconnected` for a server that closed cleanly, `Unauthorized` for a
    /// credential the transport could not renew.
    ///
    /// Must not block past the tick: a real transport reads on its own thread
    /// and this takes whatever that thread has queued.
    fn read(&mut self) -> Result<Vec<u8>, Unavailable>;
}

/// Opens byte streams for a topic.
///
/// This is the whole transport boundary, and the only part a real transport has
/// to write. Everything above it — reconnect, backoff, resume, framing and every
/// bound — is [`SseSubscription`], which is why all of it is tested here with no
/// network and no credential.
pub trait Connector {
    type Stream: EventStream;

    /// `last_event_id` is the newest `id:` seen on this topic, to be sent as
    /// `Last-Event-ID` so a server that supports resume sends what was missed
    /// instead of replaying from the top.
    fn connect(
        &mut self,
        topic: &str,
        last_event_id: Option<&str>,
    ) -> Result<Self::Stream, Unavailable>;
}

/// Reconnects attempted at growing intervals before a topic reports itself
/// unreachable. Bounded because an endpoint that is gone must not be hammered,
/// and because an unbounded ladder is indistinguishable from a hang.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 6;
/// Wait before the first reconnect; doubles per attempt.
const RECONNECT_BACKOFF_MS: u64 = 500;
/// Ceiling on one wait in the ladder.
const RECONNECT_BACKOFF_CAP_MS: u64 = 30_000;
/// Wait after the ladder is exhausted before starting it again.
///
/// Exhaustion is loud (`Unreachable`, rendered as a source error) but not
/// terminal: a wall display that needs a human to restart it after a router
/// blip is a worse failure than the blip.
pub const RECONNECT_COOLDOWN_MS: u64 = 300_000;

/// Milliseconds to wait before reconnect attempt `attempt`, 1-based.
pub fn reconnect_delay_ms(attempt: u32) -> u64 {
    let doublings = attempt.saturating_sub(1).min(u32::BITS - 1);
    RECONNECT_BACKOFF_MS
        .saturating_mul(1u64 << doublings)
        .min(RECONNECT_BACKOFF_CAP_MS)
}

/// A wait is over when its deadline passes — or when the deadline is further out
/// than the longest this connector sets, which means the clock moved rather than
/// the deadline. A backwards jump must not wedge a subscription shut.
fn due(now_ms: u64, until_ms: u64) -> bool {
    now_ms >= until_ms || until_ms - now_ms > RECONNECT_COOLDOWN_MS
}

enum Link<S> {
    /// Nothing is open and there is nothing to wait for.
    Idle,
    Open(S),
    /// Not connected, and not yet due to try again. Carries what a viewer is
    /// being told meanwhile, so the answer does not change between ticks.
    Waiting {
        until_ms: u64,
        reason: Unavailable,
    },
}

struct Topic<S> {
    link: Link<S>,
    buffer: FrameBuffer,
    last_event_id: Option<String>,
    /// Consecutive failures since the stream last said anything.
    attempts: u32,
}

impl<S> Default for Topic<S> {
    fn default() -> Self {
        Self {
            link: Link::Idle,
            buffer: FrameBuffer::default(),
            last_event_id: None,
            attempts: 0,
        }
    }
}

impl<S> Topic<S> {
    /// Ends the connection, schedules the next attempt, and returns what the
    /// viewer is told until then.
    fn fail(&mut self, reason: Unavailable, now_ms: u64) -> Delivery {
        // Dropping the stream is what closes the connection; nothing else does.
        self.link = Link::Idle;
        self.attempts += 1;
        let exhausted = self.attempts >= MAX_RECONNECT_ATTEMPTS;
        let wait = if exhausted {
            // The cooldown replaces the ladder, so the ladder starts over after
            // it rather than reporting unreachable forever after one bad night.
            self.attempts = 0;
            RECONNECT_COOLDOWN_MS
        } else {
            reconnect_delay_ms(self.attempts)
        };
        // An expired session outlives the ladder: retrying does not fix it, and
        // this is the only reason that drives re-authentication.
        let reason = match reason {
            Unavailable::Unauthorized => Unavailable::Unauthorized,
            _ if exhausted => Unavailable::Unreachable,
            _ => Unavailable::Reconnecting,
        };
        self.link = Link::Waiting {
            until_ms: now_ms.saturating_add(wait),
            reason,
        };
        Delivery::Interrupted(reason)
    }
}

/// Holds a subscription open per topic and hands the emit loop whole frames.
///
/// The lifecycle is deliberately all on this side of [`Connector`]: connect,
/// read, resume from the last event id, back off when the stream ends, and say
/// which of those is happening. A transport that only opens sockets inherits all
/// of it, and none of it needs a network to test.
pub struct SseSubscription<C: Connector> {
    connector: C,
    topics: BTreeMap<String, Topic<C::Stream>>,
}

impl<C: Connector> SseSubscription<C> {
    pub fn new(connector: C) -> Self {
        Self {
            connector,
            topics: BTreeMap::new(),
        }
    }

    /// Blocks refused for size across every topic. The emit loop reports a rise
    /// rather than letting a producer quietly lose events (rule 6).
    pub fn discarded(&self) -> u64 {
        self.topics
            .values()
            .map(|topic| topic.buffer.discarded())
            .sum()
    }
}

impl<C: Connector> Subscription for SseSubscription<C> {
    fn drain(&mut self, topic: &str, now_ms: u64) -> Delivery {
        let Self { connector, topics } = self;
        let state = topics.entry(topic.to_owned()).or_default();

        if let Link::Waiting { until_ms, reason } = state.link {
            if !due(now_ms, until_ms) {
                return Delivery::Interrupted(reason);
            }
            state.link = Link::Idle;
        }

        if matches!(state.link, Link::Idle) {
            match connector.connect(topic, state.last_event_id.as_deref()) {
                Ok(stream) => state.link = Link::Open(stream),
                Err(reason) => return state.fail(reason, now_ms),
            }
        }

        let Link::Open(stream) = &mut state.link else {
            unreachable!("a link is open or has just been scheduled to retry");
        };
        let chunk = match stream.read() {
            Ok(chunk) => chunk,
            Err(reason) => return state.fail(reason, now_ms),
        };
        // A stream that said anything at all — a keepalive counts — is reachable,
        // so the ladder starts from the top the next time it ends. Only silence
        // followed by a drop keeps climbing it.
        if !chunk.is_empty() {
            state.attempts = 0;
        }
        state.buffer.push(&chunk);
        let frames = state.buffer.take(MAX_FRAMES_PER_DRAIN);
        if let Some(id) = frames.iter().rev().find_map(|frame| frame.id.clone()) {
            state.last_event_id = Some(id);
        }
        Delivery::Frames(frames)
    }
}

/// Gap left between consecutive datagrams in a burst.
///
/// `AF_UNIX` `SOCK_DGRAM` has no flow control: a connector that emits every
/// source back to back outruns Canvas's drain loop and overruns the socket
/// buffer, and the sends that lose that race fail with `ENOBUFS`. A couple of
/// milliseconds per source is nothing against a tick measured in seconds.
pub const SEND_SPACING: Duration = Duration::from_millis(2);

/// Attempts one datagram gets before it is reported lost. Bounded so a receiver
/// that has genuinely stopped cannot stall the tick.
pub const MAX_SEND_ATTEMPTS: u32 = 5;

/// Wait after the first congested send; doubles per attempt.
const SEND_BACKOFF: Duration = Duration::from_millis(4);

/// How long to wait after attempt `attempt`, 1-based.
pub fn send_backoff(attempt: u32) -> Duration {
    SEND_BACKOFF.saturating_mul(1u32 << attempt.saturating_sub(1).min(5))
}

/// Whether an error is the socket asking us to slow down rather than telling us
/// something is wrong.
///
/// `ENOBUFS` is transient by construction — the receiver drains continuously —
/// and `WouldBlock` is the same congestion reported by a non-blocking socket.
/// Anything else is a real failure and must not be retried into silence.
pub fn congested(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOBUFS) || error.kind() == io::ErrorKind::WouldBlock
}

/// Sends one datagram, retrying while the socket is merely congested.
///
/// Returns the number of attempts it took, or the attempt count and the error
/// the last one failed with. The error is returned rather than logged because
/// losing a snapshot has to be loud: the widget keeps its previous value at
/// `state: ready`, so a dropped send reads as current data (rule 2) — it is
/// precisely the failure that is invisible unless someone says it happened.
pub fn send_retrying(
    mut send: impl FnMut() -> io::Result<()>,
    mut wait: impl FnMut(Duration),
) -> Result<u32, (u32, io::Error)> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match send() {
            Ok(()) => return Ok(attempt),
            Err(error) if attempt >= MAX_SEND_ATTEMPTS || !congested(&error) => {
                return Err((attempt, error));
            }
            Err(_) => wait(send_backoff(attempt)),
        }
    }
}

/// The window label Canvas shows beside the source. Derived from the source's
/// own horizon so it cannot drift from what the widget actually spans.
pub fn window_label(spec: &SourceSpec) -> String {
    match spec.window_seconds {
        None => "live".into(),
        Some(seconds) if seconds % 86_400 == 0 => format!("{}d", seconds / 86_400),
        Some(seconds) if seconds % 3_600 == 0 => format!("{}h", seconds / 3_600),
        Some(seconds) => format!("{}m", seconds.div_ceil(60)),
    }
}

/// Why this source's snapshot is not a fresh reading. `None` is a healthy,
/// currently-subscribed stream.
fn assess(
    spec: &SourceSpec,
    window: &Window,
    interruption: Option<Unavailable>,
    now_ms: u64,
) -> Option<Unavailable> {
    if spec.unavailable.is_some() {
        return Some(Unavailable::NotBacked);
    }
    if interruption.is_some() {
        return interruption;
    }
    let latest = window.latest_ms()?;
    match spec.stale_after_seconds {
        Some(limit) if now_ms.saturating_sub(latest) > limit * 1_000 => Some(Unavailable::Stale),
        _ => None,
    }
}

pub fn envelope(
    source_id: &str,
    spec: &SourceSpec,
    window: &Window,
    interruption: Option<Unavailable>,
    sequence: u64,
    now_ms: u64,
) -> Result<Value, String> {
    let (owner, stream) = stream_identity(source_id)?;
    let source = format!("sse:{source_id}");
    let label = window_label(spec);
    let reason = assess(spec, window, interruption, now_ms);
    let stale = reason == Some(Unavailable::Stale);
    // A stale window still holds real events, so unlike a failed poll it keeps
    // its body: rule 2 wants the value present and its age visible, not blank.
    if (reason.is_none() || stale)
        && let Some((widget, omitted)) = widget_fitted(
            spec,
            source_id,
            window,
            now_ms,
            MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD),
        )
    {
        // Observed-at is the newest event's own time, never the emit time: a
        // snapshot must not make ten-minute-old data look like it just arrived.
        let observed = window.latest_ms().unwrap_or(now_ms);
        let mut data = json!({
            "state": if stale { "stale" } else { "ready" },
            "source": source,
            "observed_at": rfc3339(observed / 1000),
            "window": label,
        });
        // A log showing eight of two hundred lines looks exactly like a log of
        // eight lines. Saying so is the difference between a bounded collection
        // and a quiet lie.
        let notes = [
            stale.then_some(Unavailable::Stale.detail()),
            omitted.then_some("older entries omitted to fit the snapshot"),
        ];
        let detail = notes
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>()
            .join("; ");
        if !detail.is_empty() {
            data["detail"] = json!(detail);
        }
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
    // No fresh surface: the widget keeps its kind and only the source-health
    // metadata changes, so the layout never flickers.
    let reason = reason.unwrap_or(Unavailable::NoData);
    Ok(json!({
        "schema_version": STREAM_SCHEMA_VERSION,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": now_ms,
        "payload": {
            "widget": placeholder(&spec.kind, &spec.title(source_id)),
            "data": {
                "state": reason.state(),
                "source": source,
                "observed_at": rfc3339(window.latest_ms().unwrap_or(now_ms) / 1000),
                "window": label,
                "detail": spec
                    .unavailable
                    .clone()
                    .unwrap_or_else(|| reason.detail().to_owned()),
            }
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: u64 = 1_788_949_087_000;

    fn spec(kind: &str) -> SourceSpec {
        SourceSpec {
            kind: kind.into(),
            topic: "activity".into(),
            event_types: Vec::new(),
            label: Some("Recent outcomes".into()),
            columns: Vec::new(),
            fields: Fields::default(),
            retain: None,
            window_seconds: (kind == "state_timeline").then_some(3600),
            stale_after_seconds: Some(120),
            unavailable: None,
        }
    }

    fn frame(event: &str, data: &str) -> Frame {
        Frame {
            event: Some(event.into()),
            id: None,
            data: data.into(),
        }
    }

    fn window(count: usize, message_chars: usize) -> Window {
        let mut window = Window::new(512);
        for index in 0..count {
            window.push(Observation {
                at_ms: NOW_MS - (count - index) as u64 * 1_000,
                subject: format!("workflow-{index}"),
                message: "x".repeat(message_chars),
                level: Level::Warning,
            });
        }
        window
    }

    #[test]
    fn stream_identity_matches_the_split_canvas_performs() {
        assert_eq!(
            stream_identity("elmers/activity").unwrap(),
            ("elmers".to_string(), "sse-activity".to_string())
        );
        for bad in [
            "noowner",
            "Elmers/activity",
            "elmers/act_ivity",
            "/activity",
            "elmers/",
        ] {
            assert!(stream_identity(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sse_blocks_parse_to_frames_including_multiline_data() {
        let text = ": keepalive\n\nevent: run\nid: 7\ndata: {\"a\":1}\n\nevent: note\ndata: one\ndata: two\n\n";
        let frames = parse_stream(text);
        assert_eq!(frames.len(), 2, "a comment-only block carries no frame");
        assert_eq!(frames[0].event.as_deref(), Some("run"));
        assert_eq!(frames[0].id.as_deref(), Some("7"));
        assert_eq!(frames[0].data, "{\"a\":1}");
        // Multi-line data is one payload, joined the way the wire defines it.
        assert_eq!(frames[1].data, "one\ntwo");
    }

    #[test]
    fn a_source_keeps_only_the_event_types_it_asked_for() {
        let mut spec = spec("event_log");
        spec.event_types = vec!["run.finished".into()];
        assert!(observe(&spec, &frame("run.started", "{}"), NOW_MS).is_none());
        assert!(observe(&spec, &frame("run.finished", "{}"), NOW_MS).is_some());
    }

    #[test]
    fn an_unknown_severity_is_neutral_rather_than_guessed() {
        // Promoting an unrecognized word to critical cries wolf; demoting it to
        // good hides an incident. Both are read as claims.
        assert_eq!(Level::parse("Failed"), Level::Critical);
        assert_eq!(Level::parse("completed"), Level::Good);
        assert_eq!(Level::parse("quiescent"), Level::Neutral);
        assert_eq!(Level::parse(""), Level::Neutral);
    }

    #[test]
    fn observations_take_their_time_from_the_payload_in_either_unit() {
        let spec = spec("event_log");
        let seconds = observe(&spec, &frame("run", r#"{"timestamp":1788949000}"#), NOW_MS).unwrap();
        let millis = observe(
            &spec,
            &frame("run", r#"{"timestamp":1788949000000}"#),
            NOW_MS,
        )
        .unwrap();
        assert_eq!(seconds.at_ms, millis.at_ms);
        // Nothing usable in the payload falls back to arrival, not to epoch 0.
        let bare = observe(&spec, &frame("run", "{}"), NOW_MS).unwrap();
        assert_eq!(bare.at_ms, NOW_MS);
    }

    #[test]
    fn a_non_json_payload_is_rendered_rather_than_dropped() {
        let spec = spec("event_log");
        let observed = observe(&spec, &frame("tick", "not json at all"), NOW_MS).unwrap();
        assert_eq!(observed.message, "not json at all");
    }

    #[test]
    fn a_window_is_bounded_and_keeps_the_newest() {
        let mut window = Window::new(3);
        for index in 0..10u64 {
            window.push(Observation {
                at_ms: NOW_MS + index,
                subject: "s".into(),
                message: index.to_string(),
                level: Level::Neutral,
            });
        }
        assert_eq!(window.observations().len(), 3);
        assert_eq!(window.latest_ms(), Some(NOW_MS + 9));
        assert_eq!(window.observations()[0].message, "7");
    }

    #[test]
    fn an_event_log_reads_newest_first_and_is_bounded() {
        let spec = spec("event_log");
        let rendered = widget(&spec, "elmers/activity", &window(40, 20), NOW_MS).unwrap();
        let entries = rendered["entries"].as_array().unwrap();
        assert_eq!(rendered["kind"], "event_log");
        assert!(!entries.is_empty());
        let times: Vec<&str> = entries
            .iter()
            .map(|entry| entry["timestamp"].as_str().unwrap())
            .collect();
        let mut descending = times.clone();
        descending.sort_by(|a, b| b.cmp(a));
        assert_eq!(times, descending, "the newest outcome is read first");
    }

    #[test]
    fn every_table_row_is_as_wide_as_the_header() {
        let mut spec = spec("table");
        spec.columns = vec!["Item".into(), "State".into(), "Age".into()];
        let rendered = widget(&spec, "elmers/attention", &window(5, 10), NOW_MS).unwrap();
        let columns = rendered["columns"].as_array().unwrap().len();
        for row in rendered["rows"].as_array().unwrap() {
            assert_eq!(row.as_array().unwrap().len(), columns);
        }
    }

    #[test]
    fn a_table_column_nothing_answers_is_refused_at_startup() {
        let mut spec = spec("table");
        spec.columns = vec!["Item".into(), "Cost".into()];
        assert!(spec.validate("elmers/attention").is_err());
        spec.columns = vec!["Item".into(), "Age".into()];
        assert!(spec.validate("elmers/attention").is_ok());
    }

    #[test]
    fn a_timeline_stays_inside_its_own_span_and_never_runs_backwards() {
        let mut spec = spec("state_timeline");
        spec.window_seconds = Some(3600);
        let mut window = Window::new(64);
        for (index, subject) in ["orchestrator", "orchestrator", "scheduler"]
            .iter()
            .enumerate()
        {
            window.push(Observation {
                at_ms: NOW_MS - 1_800_000 + index as u64 * 600_000,
                subject: (*subject).into(),
                message: "running".into(),
                level: Level::Good,
            });
        }
        let rendered = widget(&spec, "elmers/state", &window, NOW_MS).unwrap();
        let span = 3600.0;
        let lanes = rendered["lanes"].as_array().unwrap();
        assert!(!lanes.is_empty());
        for lane in lanes {
            for segment in lane["segments"].as_array().unwrap() {
                let (start, end) = (
                    segment["start"].as_f64().unwrap(),
                    segment["end"].as_f64().unwrap(),
                );
                assert!(start < end, "{start} >= {end}");
                assert!((0.0..=span).contains(&start) && (0.0..=span).contains(&end));
            }
        }
    }

    #[test]
    fn a_state_that_just_began_is_still_visible() {
        // Zero-width would render as nothing, which reads as "no state" rather
        // than "this state just started".
        let mut spec = spec("state_timeline");
        spec.window_seconds = Some(60);
        let mut window = Window::new(8);
        window.push(Observation {
            at_ms: NOW_MS,
            subject: "orchestrator".into(),
            message: "waiting".into(),
            level: Level::Neutral,
        });
        let rendered = widget(&spec, "elmers/state", &window, NOW_MS).unwrap();
        let segment = &rendered["lanes"][0]["segments"][0];
        assert!(segment["start"].as_f64().unwrap() < segment["end"].as_f64().unwrap());
    }

    #[test]
    fn a_whole_envelope_fits_the_datagram_for_every_kind() {
        // macOS caps AF_UNIX datagrams at 2048 bytes, far below Canvas's 64KB
        // constant. Sizing against the constant sends EMSGSIZE, not a snapshot.
        for kind in SUPPORTED_KINDS {
            let event = envelope(
                "elmers/some-long-source-name",
                &spec(kind),
                &window(200, 400),
                None,
                1,
                NOW_MS,
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
    fn one_enormous_message_is_clipped_rather_than_dropped() {
        // Shrinking the entry count alone never fits a single 8KB line; the text
        // budget has to shrink with it. The line still appears, and its own
        // ellipsis is the disclosure, so nothing was omitted.
        let spec = spec("event_log");
        let budget = MAX_DATAGRAM - ENVELOPE_OVERHEAD;
        let (fitted, omitted) =
            widget_fitted(&spec, "elmers/activity", &window(1, 8_000), NOW_MS, budget).unwrap();
        assert!(!omitted, "the only event is still shown");
        let message = fitted["entries"][0]["message"].as_str().unwrap();
        assert!(message.chars().count() < 8_000 && message.ends_with('…'));
        assert!(serde_json::to_vec(&fitted).unwrap().len() <= budget);
    }

    #[test]
    fn a_snapshot_that_left_events_out_says_so() {
        let event = envelope(
            "elmers/activity",
            &spec("event_log"),
            &window(200, 400),
            None,
            1,
            NOW_MS,
        )
        .unwrap();
        assert_eq!(event["payload"]["data"]["state"], "ready");
        assert!(
            event["payload"]["data"]["detail"]
                .as_str()
                .unwrap()
                .contains("omitted")
        );
    }

    #[test]
    fn an_unconnected_subscription_says_so_instead_of_loading_forever() {
        let mut transport = Unsubscribed;
        let Delivery::Interrupted(reason) = transport.drain("activity", NOW_MS) else {
            panic!("the stub cannot deliver frames");
        };
        for kind in SUPPORTED_KINDS {
            let event = envelope(
                "elmers/activity",
                &spec(kind),
                &Window::new(8),
                Some(reason),
                1,
                NOW_MS,
            )
            .unwrap();
            assert_eq!(event["payload"]["data"]["state"], "disconnected");
            assert_eq!(
                event["payload"]["data"]["detail"],
                "Not subscribed to this stream"
            );
            // Canvas drops a mutation whose kind differs from the surface, so a
            // placeholder must keep the target's kind.
            assert_eq!(event["payload"]["widget"]["kind"], kind, "{kind}");
        }
    }

    #[test]
    fn an_empty_window_is_no_data_rather_than_a_fabricated_zero() {
        let event = envelope(
            "elmers/activity",
            &spec("event_log"),
            &Window::new(8),
            None,
            1,
            NOW_MS,
        )
        .unwrap();
        assert_eq!(event["payload"]["data"]["state"], "no_data");
        assert_eq!(
            event["payload"]["data"]["detail"],
            "Subscribed; no event has arrived yet"
        );
        assert_eq!(
            event["payload"]["widget"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_quiet_stream_goes_stale_but_keeps_its_body() {
        // Unlike a failed poll, a stale subscription still holds real events.
        // Blanking them would discard information the operator can still use.
        let spec = spec("event_log");
        let mut window = Window::new(8);
        window.push(Observation {
            at_ms: NOW_MS - 600_000,
            subject: "workflow-1".into(),
            message: "completed".into(),
            level: Level::Good,
        });
        let event = envelope("elmers/activity", &spec, &window, None, 1, NOW_MS).unwrap();
        assert_eq!(event["payload"]["data"]["state"], "stale");
        assert_eq!(event["payload"]["widget"]["entries"][0]["level"], "good");
        // Observed-at is the event's own time, not the emit time: a snapshot
        // must not make ten-minute-old data look like it arrived this second.
        assert_eq!(
            event["payload"]["data"]["observed_at"],
            rfc3339((NOW_MS - 600_000) / 1000)
        );
    }

    #[test]
    fn a_source_no_stream_backs_says_so() {
        let mut spec = spec("table");
        spec.unavailable = Some("Elmer's exposes no attention stream yet".into());
        let event = envelope("elmers/attention", &spec, &Window::new(8), None, 1, NOW_MS).unwrap();
        assert_eq!(event["payload"]["data"]["state"], "no_data");
        assert_eq!(
            event["payload"]["data"]["detail"],
            "Elmer's exposes no attention stream yet"
        );
    }

    #[test]
    fn envelope_matches_the_contract_canvas_validates() {
        let mut window = Window::new(8);
        window.push(Observation {
            at_ms: NOW_MS,
            subject: "workflow-1".into(),
            message: "completed".into(),
            level: Level::Good,
        });
        let event = envelope(
            "elmers/activity",
            &spec("event_log"),
            &window,
            None,
            7,
            NOW_MS,
        )
        .unwrap();
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["owner"], "elmers");
        assert_eq!(event["stream"], "sse-activity");
        assert_eq!(event["contract"], "canvas.widget.snapshot.v1");
        assert_eq!(event["sequence"], 7);
        assert_eq!(event["emitted_at_ms"], NOW_MS);
        assert_eq!(event["payload"]["data"]["source"], "sse:elmers/activity");
        assert_eq!(event["payload"]["data"]["window"], "live");
    }

    #[test]
    fn a_window_label_is_derived_from_the_span_it_draws() {
        let mut spec = spec("state_timeline");
        assert_eq!(window_label(&spec), "1h");
        spec.window_seconds = Some(900);
        assert_eq!(window_label(&spec), "15m");
        spec.window_seconds = None;
        assert_eq!(window_label(&spec), "live");
    }

    /// One `read` per entry, then the connection is over.
    /// What one connection hands back, read by read: bytes, or the reason it
    /// ended.
    type Reads = Vec<Result<Vec<u8>, Unavailable>>;

    struct ScriptedStream {
        reads: std::collections::VecDeque<Result<Vec<u8>, Unavailable>>,
    }

    impl EventStream for ScriptedStream {
        fn read(&mut self) -> Result<Vec<u8>, Unavailable> {
            self.reads
                .pop_front()
                .unwrap_or(Err(Unavailable::Disconnected))
        }
    }

    /// Hands out scripted connections in order and records what each one was
    /// asked to resume from. An exhausted script is an endpoint that has gone.
    #[derive(Default)]
    struct ScriptedConnector {
        connections: std::collections::VecDeque<Result<Reads, Unavailable>>,
        resumed: Vec<Option<String>>,
    }

    impl Connector for ScriptedConnector {
        type Stream = ScriptedStream;

        fn connect(
            &mut self,
            _topic: &str,
            last_event_id: Option<&str>,
        ) -> Result<Self::Stream, Unavailable> {
            self.resumed.push(last_event_id.map(str::to_owned));
            match self.connections.pop_front() {
                Some(Ok(reads)) => Ok(ScriptedStream {
                    reads: reads.into(),
                }),
                Some(Err(reason)) => Err(reason),
                None => Err(Unavailable::Disconnected),
            }
        }
    }

    fn block(event: &str, data: &str) -> Vec<u8> {
        format!("event: {event}\ndata: {data}\n\n").into_bytes()
    }

    #[test]
    fn a_partial_block_is_carried_to_the_next_read_rather_than_lost() {
        let mut buffer = FrameBuffer::default();
        buffer.push(b"event: run\ndata: {\"a\":1}\n\nevent: no");
        let first = buffer.take(MAX_FRAMES_PER_DRAIN);
        assert_eq!(first.len(), 1, "only the complete block is a frame");
        buffer.push(b"te\ndata: two\n\n");
        let second = buffer.take(MAX_FRAMES_PER_DRAIN);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].event.as_deref(), Some("note"));
        assert_eq!(second[0].data, "two");
        assert_eq!(buffer.buffered(), 0);
    }

    #[test]
    fn crlf_framing_yields_the_same_frames_as_lf() {
        // A CRLF stream framed on "\n\n" alone merges every event into one
        // block, and a "\r" left on the end of a value corrupts it silently.
        let lf = parse_stream("event: run\ndata: one\n\nevent: run\ndata: two\n\n");
        let crlf = parse_stream("event: run\r\ndata: one\r\n\r\nevent: run\r\ndata: two\r\n\r\n");
        assert_eq!(lf, crlf);
        assert_eq!(crlf.len(), 2);
        assert_eq!(crlf[0].data, "one");
    }

    #[test]
    fn a_drain_takes_a_bounded_number_of_frames_and_keeps_the_rest_in_order() {
        let mut buffer = FrameBuffer::default();
        for index in 0..MAX_FRAMES_PER_DRAIN + 5 {
            buffer.push(&block("run", &index.to_string()));
        }
        let first = buffer.take(MAX_FRAMES_PER_DRAIN);
        assert_eq!(first.len(), MAX_FRAMES_PER_DRAIN);
        assert_eq!(first[0].data, "0", "the oldest event is read first");
        let rest = buffer.take(MAX_FRAMES_PER_DRAIN);
        assert_eq!(rest.len(), 5, "nothing is dropped, only deferred");
        assert_eq!(rest[0].data, MAX_FRAMES_PER_DRAIN.to_string());
    }

    #[test]
    fn an_oversized_event_is_refused_and_parsing_resumes_at_the_next_one() {
        let mut buffer = FrameBuffer::default();
        buffer.push(&block("run", &"x".repeat(MAX_EVENT_BYTES + 1)));
        buffer.push(&block("run", "small"));
        let frames = buffer.take(MAX_FRAMES_PER_DRAIN);
        assert_eq!(frames.len(), 1, "the readable event still arrives");
        assert_eq!(frames[0].data, "small");
        assert_eq!(buffer.discarded(), 1, "and the refusal is countable");
    }

    #[test]
    fn a_producer_that_outruns_the_tick_cannot_grow_the_buffer_without_bound() {
        let mut buffer = FrameBuffer::default();
        let payload = "y".repeat(1_000);
        // Enough to overrun the ceiling; the buffer's own trimming means it can
        // never be observed above it, so the count is what has to exceed it.
        for _ in 0..MAX_BUFFERED_BYTES / payload.len() + 8 {
            buffer.push(&block("run", &payload));
        }
        buffer.push(&block("run", "newest"));
        assert!(buffer.buffered() <= MAX_BUFFERED_BYTES);
        assert!(buffer.discarded() > 0, "dropping bytes is not silent");
        // The oldest blocks go first, which is what keeps a live tail live.
        let frames = buffer.take(usize::MAX);
        assert_eq!(frames.last().unwrap().data, "newest");
    }

    #[test]
    fn a_subscription_connects_once_and_hands_up_the_frames_that_arrive() {
        let mut subscription = SseSubscription::new(ScriptedConnector {
            connections: [Ok(vec![Ok(block("run", "one")), Ok(Vec::new())])].into(),
            resumed: Vec::new(),
        });
        let Delivery::Frames(frames) = subscription.drain("events", NOW_MS) else {
            panic!("the first drain should deliver");
        };
        assert_eq!(frames.len(), 1);
        // A quiet connection is open, not ended: no reconnect, no frames.
        assert_eq!(
            subscription.drain("events", NOW_MS + 1_000),
            Delivery::Frames(Vec::new())
        );
        assert_eq!(subscription.connector.resumed.len(), 1, "one connect");
    }

    #[test]
    fn a_dropped_stream_reads_as_reconnecting_and_waits_before_trying_again() {
        // The gap itself is the thing a viewer must not be left to infer.
        let mut subscription = SseSubscription::new(ScriptedConnector {
            connections: [
                Ok(vec![
                    Ok(block("run", "one")),
                    Err(Unavailable::Disconnected),
                ]),
                Ok(vec![Ok(block("run", "two"))]),
            ]
            .into(),
            resumed: Vec::new(),
        });
        assert!(matches!(
            subscription.drain("events", NOW_MS),
            Delivery::Frames(frames) if frames.len() == 1
        ));
        assert_eq!(
            subscription.drain("events", NOW_MS),
            Delivery::Interrupted(Unavailable::Reconnecting)
        );
        let wait = reconnect_delay_ms(1);
        assert_eq!(
            subscription.drain("events", NOW_MS + wait - 1),
            Delivery::Interrupted(Unavailable::Reconnecting),
            "the answer does not change while the backoff runs"
        );
        assert_eq!(
            subscription.connector.resumed.len(),
            1,
            "and nothing reconnects inside it"
        );
        assert!(matches!(
            subscription.drain("events", NOW_MS + wait),
            Delivery::Frames(frames) if frames.len() == 1
        ));
        assert_eq!(subscription.connector.resumed.len(), 2);
    }

    #[test]
    fn reconnects_are_bounded_and_the_ladder_restarts_after_a_cooldown() {
        let mut subscription = SseSubscription::new(ScriptedConnector::default());
        let mut now = NOW_MS;
        for attempt in 1..MAX_RECONNECT_ATTEMPTS {
            assert_eq!(
                subscription.drain("events", now),
                Delivery::Interrupted(Unavailable::Reconnecting),
                "attempt {attempt} is still a retry"
            );
            now += reconnect_delay_ms(attempt);
        }
        assert_eq!(
            subscription.drain("events", now),
            Delivery::Interrupted(Unavailable::Unreachable),
            "the ladder ends loudly rather than climbing forever"
        );
        let attempts = subscription.connector.resumed.len() as u32;
        assert_eq!(attempts, MAX_RECONNECT_ATTEMPTS);
        assert_eq!(
            subscription.drain("events", now + RECONNECT_COOLDOWN_MS - 1),
            Delivery::Interrupted(Unavailable::Unreachable),
        );
        assert_eq!(
            subscription.connector.resumed.len() as u32,
            attempts,
            "an unreachable endpoint is not hammered during the cooldown"
        );
        // Unreachable is loud, not terminal: a stream that comes back is picked
        // up without anyone restarting the connector.
        assert_eq!(
            subscription.drain("events", now + RECONNECT_COOLDOWN_MS),
            Delivery::Interrupted(Unavailable::Reconnecting),
        );
    }

    #[test]
    fn a_stream_that_spoke_earns_a_fresh_ladder_when_it_ends() {
        // A blip on a healthy stream must not inherit the backoff of an endpoint
        // that was already failing, or the first drop after a bad night waits
        // half a minute for no reason.
        let mut subscription = SseSubscription::new(ScriptedConnector {
            connections: [
                Err(Unavailable::Disconnected),
                Err(Unavailable::Disconnected),
                Ok(vec![
                    Ok(block("run", "one")),
                    Err(Unavailable::Disconnected),
                ]),
                Ok(vec![Ok(block("run", "two"))]),
            ]
            .into(),
            resumed: Vec::new(),
        });
        let mut now = NOW_MS;
        subscription.drain("events", now);
        now += reconnect_delay_ms(1);
        subscription.drain("events", now);
        now += reconnect_delay_ms(2);
        assert!(matches!(
            subscription.drain("events", now),
            Delivery::Frames(frames) if frames.len() == 1
        ));
        assert_eq!(
            subscription.drain("events", now),
            Delivery::Interrupted(Unavailable::Reconnecting)
        );
        assert!(
            matches!(
                subscription.drain("events", now + reconnect_delay_ms(1)),
                Delivery::Frames(frames) if frames.len() == 1
            ),
            "the ladder restarted at its first rung"
        );
    }

    #[test]
    fn an_expired_session_is_reported_as_itself_so_re_authentication_can_happen() {
        // Reporting this as `Reconnecting` would retry an expired credential
        // until the cooldown and never open the re-auth path.
        let mut subscription = SseSubscription::new(ScriptedConnector {
            connections: [Err(Unavailable::Unauthorized)].into(),
            resumed: Vec::new(),
        });
        assert_eq!(
            subscription.drain("events", NOW_MS),
            Delivery::Interrupted(Unavailable::Unauthorized)
        );
        assert_eq!(
            subscription.drain("events", NOW_MS + 1),
            Delivery::Interrupted(Unavailable::Unauthorized),
            "and it stays that way while the backoff runs"
        );
    }

    #[test]
    fn a_reconnect_resumes_from_the_last_event_id_it_saw() {
        let mut subscription = SseSubscription::new(ScriptedConnector {
            connections: [
                Ok(vec![
                    Ok(b"id: 41\ndata: one\n\nid: 42\ndata: two\n\n".to_vec()),
                    Err(Unavailable::Disconnected),
                ]),
                Ok(vec![Ok(Vec::new())]),
            ]
            .into(),
            resumed: Vec::new(),
        });
        subscription.drain("events", NOW_MS);
        subscription.drain("events", NOW_MS);
        subscription.drain("events", NOW_MS + reconnect_delay_ms(1));
        assert_eq!(
            subscription.connector.resumed,
            vec![None, Some("42".to_string())],
            "the newest id seen, not the first"
        );
    }

    #[test]
    fn a_clock_that_jumps_backwards_does_not_wedge_a_subscription_shut() {
        let mut subscription = SseSubscription::new(ScriptedConnector::default());
        subscription.drain("events", NOW_MS);
        // A deadline further out than any this connector sets is a moved clock,
        // not a wait to honour.
        assert_eq!(
            subscription.drain("events", NOW_MS - RECONNECT_COOLDOWN_MS * 2),
            Delivery::Interrupted(Unavailable::Reconnecting)
        );
        assert_eq!(subscription.connector.resumed.len(), 2, "it tried again");
    }

    #[test]
    fn a_reconnecting_source_says_so_in_the_snapshot_a_widget_reads() {
        for (reason, state) in [
            (Unavailable::Reconnecting, "disconnected"),
            (Unavailable::Unreachable, "error"),
        ] {
            let event = envelope(
                "elmers/activity",
                &spec("event_log"),
                &window(4, 10),
                Some(reason),
                1,
                NOW_MS,
            )
            .unwrap();
            assert_eq!(event["payload"]["data"]["state"], state);
            assert_eq!(event["payload"]["data"]["detail"], reason.detail());
            // Canvas drops a mutation whose kind differs from the surface.
            assert_eq!(event["payload"]["widget"]["kind"], "event_log");
        }
    }

    fn congestion() -> io::Error {
        io::Error::from_raw_os_error(libc::ENOBUFS)
    }

    #[test]
    fn a_congested_send_is_waited_out_rather_than_dropped() {
        // A dropped snapshot leaves the widget on its previous value at
        // state=ready, so it reads as current while being a tick old.
        let mut attempts = 0;
        let mut waits = Vec::new();
        let sent = send_retrying(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(congestion())
                } else {
                    Ok(())
                }
            },
            |wait| waits.push(wait),
        );
        assert_eq!(sent.ok(), Some(3), "the snapshot still went out");
        assert_eq!(waits, vec![send_backoff(1), send_backoff(2)]);
    }

    #[test]
    fn a_send_that_stays_congested_is_reported_rather_than_retried_forever() {
        let mut waits = Vec::new();
        let outcome = send_retrying(|| Err(congestion()), |wait| waits.push(wait));
        let Err((attempts, error)) = outcome else {
            panic!("a socket that never drains cannot succeed");
        };
        assert_eq!(attempts, MAX_SEND_ATTEMPTS, "the retry is bounded");
        assert!(congested(&error), "and the caller is handed the reason");
        assert_eq!(waits.len() as u32, MAX_SEND_ATTEMPTS - 1);
        // The whole ladder has to stay well inside a tick, or pacing one source
        // delays every source behind it.
        assert!(waits.iter().sum::<Duration>() < Duration::from_millis(500));
    }

    #[test]
    fn an_error_that_is_not_congestion_fails_immediately() {
        // Retrying a socket that does not exist just delays the report.
        let mut waits = Vec::new();
        let outcome = send_retrying(
            || Err(io::Error::from(io::ErrorKind::NotFound)),
            |wait| waits.push(wait),
        );
        assert!(matches!(outcome, Err((1, _))));
        assert!(waits.is_empty());
    }

    #[test]
    fn every_wait_this_connector_takes_grows_and_stays_bounded() {
        let sends: Vec<Duration> = (1..=MAX_SEND_ATTEMPTS).map(send_backoff).collect();
        assert!(sends.windows(2).all(|pair| pair[0] <= pair[1]));
        let reconnects: Vec<u64> = (1..=MAX_RECONNECT_ATTEMPTS + 4)
            .map(reconnect_delay_ms)
            .collect();
        assert!(reconnects.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(reconnects.iter().all(|wait| *wait <= RECONNECT_COOLDOWN_MS));
    }

    /// The bug this crate exists for: `elmers-now` declares `sse:` sources that
    /// nothing emits, so those widgets sit on Loading forever. This fails if a
    /// view gains an SSE widget the shipped mapping does not back.
    #[test]
    fn every_sse_source_in_the_view_is_backed_by_the_shipped_mapping() {
        let view: Value =
            serde_json::from_str(include_str!("../../../config/views/elmers-now.json")).unwrap();
        let mapping: Mapping = serde_json::from_str(include_str!("../sse.example.json")).unwrap();
        let mut checked = 0;
        for (id, widget) in view["widgets"].as_object().unwrap() {
            let Some(source) = widget["spec"]["data"]["source"].as_str() else {
                continue;
            };
            let Some(source_id) = source.strip_prefix("sse:") else {
                continue;
            };
            checked += 1;
            let spec = mapping
                .sources
                .get(source_id)
                .unwrap_or_else(|| panic!("{id}: no sse mapping backs {source}"));
            assert_eq!(
                widget["spec"]["kind"], spec.kind,
                "{id}: mapping feeds the wrong widget kind"
            );
            spec.validate(source_id).unwrap();
            stream_identity(source_id).unwrap();
        }
        assert_eq!(checked, 3, "elmers-now declares three sse sources");
    }
}
