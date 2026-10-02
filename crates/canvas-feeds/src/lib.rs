//! Host-side producers that surface public endpoints as Canvas widget snapshots.
//!
//! The Lua sandbox is deliberately offline, so fetching network data is done
//! here and only rendering is left to a binder. Each public source normalizes to
//! a single [`Headline`] shape, which means one reusable binder
//! (`feed-headlines-v1`) renders Hacker News, Reddit and RSS identically.
//!
//! Fetching is split from parsing: [`Http`] is the seam, so every normalizer is
//! unit-tested with canned JSON/XML and no network.
//!
//! ## Layout discipline (docs/widget-quality.md)
//!
//! A datagram that exceeds the AF_UNIX cap (2048 B on macOS) is rejected, not
//! truncated, so [`fit`] trims trailing rows until the snapshot fits instead of
//! fabricating a partial list that reads as complete.

use canvas_core::stream_socket_path;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::net::UnixDatagram,
    time::{SystemTime, UNIX_EPOCH},
};

/// Minimal Prometheus text projection shared by owner-side Canvas connectors.
/// It intentionally keeps only decoded samples; connectors decide which labels
/// are safe to expose in their bounded Canvas contracts.
#[derive(Default)]
pub struct PrometheusSamples(pub Vec<PrometheusSample>);

#[derive(Default)]
pub struct PrometheusSample {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

impl PrometheusSamples {
    pub fn parse(text: &str) -> Self {
        Self(text.lines().filter_map(parse_prometheus_sample).collect())
    }

    pub fn scalar(&self, name: &str) -> Option<f64> {
        self.0
            .iter()
            .find(|sample| sample.name == name && sample.labels.is_empty())
            .map(|sample| sample.value)
    }

    pub fn labeled(&self, name: &str, key: &str, value: &str) -> Option<f64> {
        self.0
            .iter()
            .find(|sample| {
                sample.name == name && sample.labels.get(key).is_some_and(|label| label == value)
            })
            .map(|sample| sample.value)
    }

    pub fn named(&self, name: &str) -> impl Iterator<Item = &PrometheusSample> {
        self.0.iter().filter(move |sample| sample.name == name)
    }

    pub fn sum(&self, name: &str) -> f64 {
        self.named(name).map(|sample| sample.value).sum()
    }
}

fn parse_prometheus_sample(line: &str) -> Option<PrometheusSample> {
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (metric, value) = line.rsplit_once(char::is_whitespace)?;
    let value = value.parse().ok()?;
    let (name, labels) = metric
        .split_once('{')
        .map_or((metric, ""), |(name, labels)| {
            (name, labels.trim_end_matches('}'))
        });
    let labels = labels
        .split(',')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_string(), value.trim_matches('"').to_string()))
        })
        .collect();
    Some(PrometheusSample {
        name: name.into(),
        labels,
        value,
    })
}

pub mod bsky_trends;
pub mod comics;
pub mod reddit_trends;
pub mod x_trends;

pub const CONTRACT: &str = "canvas.widget.snapshot.v1";
pub const SCHEMA_VERSION: u32 = 1;
/// macOS caps `AF_UNIX` datagrams at this size; a larger send fails with
/// `EMSGSIZE` rather than truncating. Canvas enforces a 64 KB ceiling too, but
/// the kernel cap is the one that actually bites.
pub const MAX_DATAGRAM: usize = 2048;
/// Envelope bytes around the widget, leaving headroom so a fully-fitted row set
/// lands reliably under `MAX_DATAGRAM`.
pub const ENVELOPE_OVERHEAD: usize = 200;
/// Rows are capped both for the datagram and for a single readable screen.
pub const MAX_ROWS: usize = 12;

/// A normalized headline. `score`/`by`/`age_secs`/`url` may be absent depending
/// on the source (RSS has none of them); the binder renders the gaps honestly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Headline {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
    pub source: &'static str,
}

/// The raw feed a binder consumes. The producer puts this verbatim in the
/// envelope payload; `feed-headlines-v1` turns it into a `Table` widget.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Feed {
    pub title: String,
    pub source: &'static str,
    pub rows: Vec<Headline>,
}

/// Public endpoints supported by a single normalized fetch.
#[derive(Debug, Clone)]
pub enum FeedKind {
    /// Hacker News front page (Firebase API, no auth).
    HackerNews,
    /// A subreddit hot list via the open RSS listing (www.reddit.com/r/SUB/hot.rss);
    /// the JSON API is login-walled for unauthenticated clients.
    Reddit { sub: String },
    /// An arbitrary RSS/Atom-ish feed (raw XML).
    Rss { url: String, title: String },
}

impl FeedKind {
    pub fn label(&self) -> &'static str {
        match self {
            FeedKind::HackerNews => "Hacker News",
            FeedKind::Reddit { .. } => "Reddit",
            FeedKind::Rss { .. } => "RSS",
        }
    }
    pub fn source(&self) -> &'static str {
        match self {
            FeedKind::HackerNews => "hn",
            FeedKind::Reddit { .. } => "reddit",
            FeedKind::Rss { .. } => "rss",
        }
    }
}

/// One recoverable failure mode for a producer, carried to the widget as a
/// source-health state instead of crashing the loop.
#[derive(Debug, Clone)]
pub enum Error {
    /// The HTTP fetch failed or returned non-200.
    Fetch { source: String, detail: String },
    /// The body parsed but had no rows.
    Empty { source: String },
    /// A row could not be normalized (skipped, not fatal).
    Row { source: String, detail: String },
    /// No AF_UNIX runtime dir to send to.
    NoSocket,
    /// The send failed (Canvas down, usually).
    Send { detail: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Fetch { source, detail } => {
                write!(f, "{source} fetch failed: {detail}")
            }
            Error::Empty { source } => write!(f, "{source} returned no items"),
            Error::Row { source, detail } => write!(f, "{source} row: {detail}"),
            Error::NoSocket => write!(f, "no Canvas stream socket home is configured"),
            Error::Send { detail } => write!(f, "send failed: {detail}"),
        }
    }
}
impl std::error::Error for Error {}

/// HTTP as a seam. The real implementation is [`ReqwestHttp`]; tests pass a stub
/// so parsing is exercised without touching the network.
pub trait Http {
    fn get_json(&self, url: &str) -> Result<Value, Error>;
    fn get_text(&self, url: &str) -> Result<String, Error>;
    /// Raw bytes, for a producer that embeds fetched images rather than
    /// rendering text. Kept on the same seam so tests stub all three together.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, Error>;
}

/// Boring choice: `reqwest` with `rustls-tls` (no native OpenSSL link).
pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
}
impl Default for ReqwestHttp {
    fn default() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .user_agent("canvas-feeds/0.1 (+https://futurepresentlabs.com)")
                .build()
                .expect("reqwest client builds"),
        }
    }
}
impl Http for ReqwestHttp {
    fn get_json(&self, url: &str) -> Result<Value, Error> {
        let resp = self.client.get(url).send().map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })?;
        if !resp.status().is_success() {
            return Err(Error::Fetch {
                source: url.into(),
                detail: format!("HTTP {}", resp.status()),
            });
        }
        resp.json::<Value>().map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })
    }
    fn get_text(&self, url: &str) -> Result<String, Error> {
        let resp = self.client.get(url).send().map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })?;
        if !resp.status().is_success() {
            return Err(Error::Fetch {
                source: url.into(),
                detail: format!("HTTP {}", resp.status()),
            });
        }
        resp.text().map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })
    }
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, Error> {
        let resp = self.client.get(url).send().map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })?;
        if !resp.status().is_success() {
            return Err(Error::Fetch {
                source: url.into(),
                detail: format!("HTTP {}", resp.status()),
            });
        }
        resp.bytes().map(|b| b.to_vec()).map_err(|e| Error::Fetch {
            source: url.into(),
            detail: e.to_string(),
        })
    }
}

/// Seconds since the UNIX epoch, with a sane zero on a wedged clock.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Fetch a feed from the public endpoint it names. Pure given an [`Http`].
pub fn fetch(kind: &FeedKind, http: &dyn Http) -> Result<Feed, Error> {
    match kind {
        FeedKind::HackerNews => fetch_hacker_news(http),
        FeedKind::Reddit { sub } => fetch_reddit(http, sub),
        FeedKind::Rss { url, title } => fetch_rss(http, url, title),
    }
}

const HN_BASE: &str = "https://hacker-news.firebaseio.com/v0";

fn fetch_hacker_news(http: &dyn Http) -> Result<Feed, Error> {
    let ids: Vec<u64> = http
        .get_json(&format!("{HN_BASE}/topstories.json?print=compact"))?
        .as_array()
        .ok_or(Error::Empty {
            source: "hn".into(),
        })?
        .iter()
        .take(MAX_ROWS)
        .map(|v| v.as_u64().unwrap_or(0))
        .collect();
    if ids.is_empty() {
        return Err(Error::Empty {
            source: "hn".into(),
        });
    }
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        let item = http.get_json(&format!("{HN_BASE}/item/{id}.json"))?;
        if let Some(h) = normalize_hn_item(&item, "hn") {
            rows.push(h);
        }
    }
    rows.truncate(MAX_ROWS);
    Ok(Feed {
        title: "Hacker News".into(),
        source: "hn",
        rows,
    })
}

fn normalize_hn_item(item: &Value, source: &'static str) -> Option<Headline> {
    let title = item.get("title")?.as_str()?;
    if title.is_empty() {
        return None;
    }
    let id = item.get("id").and_then(|v| v.as_u64());
    let url = item
        .get("url")
        .and_then(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .or_else(|| id.map(|id| format!("https://news.ycombinator.com/item?id={id}")));
    let age_secs = id.and_then(|_| {
        item.get("time")
            .and_then(|t| t.as_u64())
            .map(|posted| now_ms() / 1000 - posted)
    });
    Some(Headline {
        title: trim_title(title),
        url,
        score: item.get("score").and_then(|v| v.as_u64()),
        by: item.get("by").and_then(|v| v.as_str().map(str::to_owned)),
        age_secs,
        source,
    })
}

fn fetch_reddit(http: &dyn Http, sub: &str) -> Result<Feed, Error> {
    // Reddit gates its JSON API behind a login wall (302 to /login) for
    // unauthenticated clients; the RSS view of the same listing stays open.
    let url = format!("https://www.reddit.com/r/{sub}/hot.rss?raw_json=1");
    let body = http.get_text(&url)?;
    let items = parse_rss_items(&body);
    if items.is_empty() {
        return Err(Error::Empty {
            source: "reddit".into(),
        });
    }
    let rows = items
        .into_iter()
        .take(MAX_ROWS)
        .map(|(title, url, author)| Headline {
            title: trim_title(&title),
            url,
            score: None,
            by: author
                .map(|a| a.trim_start_matches("/u/").to_owned())
                .filter(|a| !a.is_empty()),
            age_secs: None,
            source: "reddit",
        })
        .collect();
    Ok(Feed {
        title: format!("r/{sub}"),
        source: "reddit",
        rows,
    })
}

fn fetch_rss(http: &dyn Http, url: &str, title: &str) -> Result<Feed, Error> {
    let body = http.get_text(url)?;
    let items = parse_rss_items(&body);
    if items.is_empty() {
        return Err(Error::Empty {
            source: "rss".into(),
        });
    }
    let rows = items
        .into_iter()
        .take(MAX_ROWS)
        .map(|(t, link, author)| Headline {
            title: trim_title(&t),
            url: link,
            score: None,
            by: author,
            age_secs: None,
            source: "rss",
        })
        .collect();
    Ok(Feed {
        title: title.to_owned(),
        source: "rss",
        rows,
    })
}

/// Extract headline rows from RSS 2.0 **or** Atom feeds.
///
/// The two families disagree on every tag we need: RSS uses
/// `<item><title>…</title><link>URL</link></item>`; Reddit serves Atom as
/// `<entry><title>…</title><link href="URL"/><author><name>…</name></author></entry>`.
/// Accepting only `<item>` reads a valid Atom feed as empty and reports a
/// source fault — which is exactly how r/programming looked dead. Both shapes
/// normalize to (title, url, author), best-effort per row.
fn parse_rss_items(xml: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    for block in item_blocks(xml) {
        let title = field(block, "title").unwrap_or_else(untitled);
        let link = field(block, "link").or_else(|| atom_link(block));
        let author = author_of(block);
        out.push((title, link, author));
    }
    out
}

/// Yield the inner text of each `<item>` (RSS) or `<entry>` (Atom) block, in
/// document order, tolerating self-closing and unmatched tags by skipping them.
pub(crate) fn item_blocks(xml: &str) -> Vec<&str> {
    const TAGS: [(&str, &str); 2] = [("<item>", "</item>"), ("<entry>", "</entry>")];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (open, close) in TAGS {
        let mut cursor = 0usize;
        while let Some(start) = xml[cursor..].find(open) {
            let from = cursor + start + open.len();
            match xml[from..].find(close) {
                Some(end) => {
                    spans.push((from, from + end));
                    cursor = from + end + close.len();
                }
                None => break,
            }
        }
    }
    spans.sort();
    spans.into_iter().map(|(a, b)| &xml[a..b]).collect()
}

/// An Atom `<link href="…"/>` — the URL is an attribute, not element text.
fn atom_link(block: &str) -> Option<String> {
    let tag = block.find("<link")?;
    let href = block[tag..].find("href=\"")? + tag + "href=\"".len();
    let end = block[href..].find('"')? + href;
    let url = &block[href..end];
    (!url.is_empty()).then(|| unescape(url))
}

/// Author as either RSS `<author>name</author>` text or Atom's nested
/// `<author><name>…</name></author>`; Atom's raw `<author>` is a block of
/// tags, never a plain name, so `<name>` must win when present.
fn author_of(block: &str) -> Option<String> {
    if let Some(name) = field(block, "name") {
        return Some(name);
    }
    field(block, "author")
}

pub(crate) fn untitled() -> String {
    "(untitled)".into()
}
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}
fn trim_title(t: &str) -> String {
    let t = t.trim();
    if t.len() > 96 {
        // char-not-byte: slicing `&t[..93]` lands mid-sequence on emoji or
        // curly quotes and panics the whole producer.
        format!("{}…", t.chars().take(90).collect::<String>())
    } else {
        t.to_owned()
    }
}

/// Build the raw-feed payload a binder consumes. The producer hands the binder
/// the normalized shape so HN/RSS/Reddit are interchangeable at render time.
pub fn payload(feed: &Feed) -> Value {
    json!({
        "title": feed.title,
        "source": feed.source,
        "rows": feed.rows,
    })
}

/// A ready snapshot envelope (raw payload — the binder renders the widget).
pub fn envelope(owner: &str, stream: &str, feed: &Feed, sequence: u64, emitted_ms: u64) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": emitted_ms,
        "payload": payload(feed),
    })
}

/// A no-data/error envelope: the source is unhealthy, so the widget is marked
/// rather than fabricated. Honest staleness, per widget-quality rule 2.
pub fn error_envelope(
    owner: &str,
    stream: &str,
    detail: &str,
    sequence: u64,
    emitted_ms: u64,
) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": emitted_ms,
        "payload": {
            "title": "headlines",
            "source": stream,
            "rows": [],
            "error": detail,
        },
    })
}

/// Trim trailing rows until the JSON fits `budget` bytes. Returns whether the
/// set was thinned, so the caller can log that the list is partial.
pub fn fit(mut event: Value, budget: usize) -> (Value, bool) {
    let mut thinned = false;
    loop {
        let over = serde_json::to_vec(&event)
            .map(|b| b.len())
            .unwrap_or(usize::MAX)
            > budget;
        let count = event
            .pointer("/payload/rows")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        if !over || count <= 1 {
            break;
        }
        if let Some(rows) = event
            .pointer_mut("/payload/rows")
            .and_then(|v| v.as_array_mut())
        {
            rows.pop();
        }
        thinned = true;
    }
    (event, thinned)
}

/// Reads the text between `<tag>…</tag>`, unescaping entities. CDATA and nested
/// tags are not expected in headline titles; anything we cannot decode is left
/// empty so the widget shows a gap instead of garbled markup.
pub(crate) fn field(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let begin = block.find(&open).map(|i| i + open.len())?;
    let end = block[begin..].find(&format!("</{tag}>"))? + begin;
    Some(unescape(&block[begin..end]))
}

/// Locate Canvas's stream socket (same home the shell finds) and send one
/// snapshot envelope for a freshly fetched feed. The datagram is fitted to the
/// AF_UNIX budget by trimming trailing rows; a partial list is still honest.
pub fn emit(
    owner: &str,
    stream: &str,
    feed: &Feed,
    sequence: u64,
    emitted_ms: u64,
) -> Result<(), Error> {
    let event = envelope(owner, stream, feed, sequence, emitted_ms);
    send_raw(&fit(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD)).0)
}

/// Send an already-built envelope (e.g. an error/no-data snapshot).
pub fn send_raw(event: &Value) -> Result<(), Error> {
    let path = stream_socket_path().ok_or(Error::NoSocket)?;
    let bytes = serde_json::to_vec(event).map_err(|e| Error::Send {
        detail: e.to_string(),
    })?;
    let Ok(sock) = UnixDatagram::unbound() else {
        return Err(Error::Send {
            detail: "cannot create datagram socket".into(),
        });
    };
    sock.send_to(&bytes, &path).map_err(|e| Error::Send {
        detail: e.to_string(),
    })?;
    Ok(())
}

/// Send one logical stream envelope, splitting it into independently bounded
/// Canvas-owned chunks when the local AF_UNIX datagram ceiling requires it.
/// Canvas publishes the logical event only after complete digest-verified
/// reassembly, so a dropped chunk leaves the previous retained snapshot live.
pub fn send_chunked(event: &Value, datagram_limit: usize) -> Result<(), Error> {
    for envelope in chunk_envelopes(event, datagram_limit)? {
        send_raw(&envelope)?;
    }
    Ok(())
}

/// Encode a logical event as one raw envelope or a sequence of independently
/// bounded chunk envelopes. This pure half of [`send_chunked`] is shared by
/// tests and producers that need to inspect the transport projection.
pub fn chunk_envelopes(event: &Value, datagram_limit: usize) -> Result<Vec<Value>, Error> {
    const CHUNK_BYTES: usize = 700;
    let bytes = serde_json::to_vec(event).map_err(|error| Error::Send {
        detail: error.to_string(),
    })?;
    if bytes.len() <= datagram_limit {
        return Ok(vec![event.clone()]);
    }
    if bytes.len() > canvas_core::chunk::MAX_GENERATION_BYTES {
        return Err(Error::Send {
            detail: format!(
                "logical stream event is {} bytes; chunk generations are limited to {}",
                bytes.len(),
                canvas_core::chunk::MAX_GENERATION_BYTES
            ),
        });
    }
    let count = bytes.len().div_ceil(CHUNK_BYTES);
    if count > canvas_core::chunk::MAX_CHUNKS {
        return Err(Error::Send {
            detail: format!("logical stream event requires {count} chunks"),
        });
    }
    let owner = event["owner"]
        .as_str()
        .ok_or_else(|| Error::Send {
            detail: "logical stream event has no owner".into(),
        })?
        .to_owned();
    let stream = event["stream"]
        .as_str()
        .ok_or_else(|| Error::Send {
            detail: "logical stream event has no stream".into(),
        })?
        .to_owned();
    let sequence = event["sequence"].as_u64().unwrap_or_else(now_ms);
    let digest = canvas_core::chunk::sha256_hex(&bytes);
    let generation_id = format!("{owner}-{stream}-{sequence}-{}", &digest[..16]);
    let expires_at_ms = now_ms().saturating_add(canvas_core::chunk::DEFAULT_EXPIRY_MS);
    let mut envelopes = Vec::with_capacity(count);
    for (index, part) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let chunk = canvas_core::chunk::StreamChunk {
            generation_id: generation_id.clone(),
            index: index as u16,
            count: count as u16,
            payload_sha256: digest.clone(),
            expires_at_ms,
            payload_hex: canvas_core::chunk::encode_hex(part),
        };
        let envelope = json!({
            "schema_version": SCHEMA_VERSION,
            "owner": &owner,
            "stream": &stream,
            "contract": canvas_core::chunk::CONTRACT,
            "sequence": sequence,
            "emitted_at_ms": now_ms(),
            "payload": chunk,
        });
        let encoded = serde_json::to_vec(&envelope).map_err(|error| Error::Send {
            detail: error.to_string(),
        })?;
        if encoded.len() > datagram_limit {
            return Err(Error::Send {
                detail: format!("chunk envelope is {} bytes", encoded.len()),
            });
        }
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeHttp {
        json: std::collections::BTreeMap<String, Value>,
        text: std::collections::BTreeMap<String, String>,
        bytes: std::collections::BTreeMap<String, Vec<u8>>,
    }
    impl Http for FakeHttp {
        fn get_json(&self, url: &str) -> Result<Value, Error> {
            self.json
                .get(url)
                .cloned()
                .ok_or(Error::Empty { source: url.into() })
        }
        fn get_text(&self, url: &str) -> Result<String, Error> {
            self.text
                .get(url)
                .cloned()
                .ok_or(Error::Empty { source: url.into() })
        }
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, Error> {
            self.bytes
                .get(url)
                .cloned()
                .ok_or(Error::Empty { source: url.into() })
        }
    }

    fn hn_item(id: u64, title: &str, score: u64, by: &str, time: u64) -> Value {
        json!({ "id": id, "title": title, "score": score, "by": by, "time": time, "url": "" })
    }

    #[test]
    fn hn_normalizes_to_a_headline() {
        let item = hn_item(1, "Show HN: a thing", 42, "alice", 0);
        let h = normalize_hn_item(&item, "hn").unwrap();
        assert_eq!(h.title, "Show HN: a thing");
        assert_eq!(h.score, Some(42));
        assert_eq!(h.by.as_deref(), Some("alice"));
        assert_eq!(
            h.url.as_deref(),
            Some("https://news.ycombinator.com/item?id=1")
        );
        assert!(
            h.age_secs.is_some(),
            "age should be derivable from posted time"
        );
    }

    #[test]
    fn hn_fetch_normalizes_and_caps_rows() {
        let mut json = std::collections::BTreeMap::new();
        json.insert(
            "https://hacker-news.firebaseio.com/v0/topstories.json?print=compact".into(),
            json!([100, 101, 102]),
        );
        json.insert(
            "https://hacker-news.firebaseio.com/v0/item/100.json".into(),
            hn_item(100, "Hundred", 10, "a", 0),
        );
        json.insert(
            "https://hacker-news.firebaseio.com/v0/item/101.json".into(),
            hn_item(101, "Hundred One", 20, "b", 0),
        );
        json.insert(
            "https://hacker-news.firebaseio.com/v0/item/102.json".into(),
            hn_item(102, "Hundred Two", 30, "c", 0),
        );
        let http = FakeHttp {
            json,
            text: std::collections::BTreeMap::new(),
            ..Default::default()
        };
        let feed = fetch(&FeedKind::HackerNews, &http).unwrap();
        assert_eq!(feed.source, "hn");
        assert_eq!(feed.rows.len(), 3);
        assert_eq!(feed.rows[0].by.as_deref(), Some("a"));
    }

    #[test]
    fn rss_fetch_extracts_items() {
        let xml = "<rss><channel><item><title>t1</title><link>https://e.com/1</link></item>\
                   <item><title>t2</title><link>https://e.com/2</link></item></channel></rss>";
        let mut text = std::collections::BTreeMap::new();
        text.insert("https://example.com/rss".into(), xml.to_string());
        let http = FakeHttp {
            json: std::collections::BTreeMap::new(),
            text,
            ..Default::default()
        };
        let feed = fetch(
            &FeedKind::Rss {
                url: "https://example.com/rss".into(),
                title: "X".into(),
            },
            &http,
        )
        .unwrap();
        assert_eq!(feed.rows.len(), 2);
        assert_eq!(feed.rows[0].title, "t1");
        assert_eq!(feed.rows[1].url.as_deref(), Some("https://e.com/2"));
        assert_eq!(feed.rows[0].score, None);
    }

    #[test]
    fn reddit_fetch_uses_the_open_atom_listing() {
        // Mirrors the real r/programming/hot.rss shape: Atom <entry>, author
        // nested in <name>, link as an href attribute.
        let xml = "<feed><entry><title>Post &gt; one</title><author><name>/u/bob</name></author>\
                   <link href=\"https://reddit.com/comments/abc\"/></entry>\
                   <entry><title>Post two</title><link href=\"https://e.com/2\"/></entry></feed>";
        let mut text = std::collections::BTreeMap::new();
        text.insert(
            "https://www.reddit.com/r/programming/hot.rss?raw_json=1".into(),
            xml.to_string(),
        );
        let http = FakeHttp {
            json: std::collections::BTreeMap::new(),
            text,
            ..Default::default()
        };
        let feed = fetch(
            &FeedKind::Reddit {
                sub: "programming".into(),
            },
            &http,
        )
        .unwrap();
        assert_eq!(feed.source, "reddit");
        assert_eq!(feed.title, "r/programming");
        assert_eq!(feed.rows.len(), 2);
        assert_eq!(feed.rows[0].title, "Post > one");
        assert_eq!(feed.rows[0].by.as_deref(), Some("bob"));
        assert_eq!(
            feed.rows[0].url.as_deref(),
            Some("https://reddit.com/comments/abc")
        );
        assert_eq!(feed.rows[0].score, None);
        assert_eq!(feed.rows[1].by, None);
    }

    #[test]
    fn atom_and_rss_blocks_keep_document_order_and_rss_wins_when_mixed() {
        // The split-on-tag approach must not reorder: this feed interleaves a
        // <item> between two <entry> blocks.
        let xml = "<feed><entry><title>a</title></entry>\
                   <item><title>b</title><link>https://e.com/b</link></item>\
                   <entry><title>c</title></entry></feed>";
        let items = parse_rss_items(xml);
        let titles: Vec<&str> = items.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(titles, vec!["a", "b", "c"]);
        assert_eq!(items[1].1.as_deref(), Some("https://e.com/b"));
    }

    #[test]
    fn rss_items_are_extracted_and_unescaped() {
        let xml = "<rss><channel><item><title>hi&gt;there</title><link>https://e.com</link></item>\
                   <item><title>second</title><link>https://e.com/2</link></item></channel></rss>";
        let items = parse_rss_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "hi>there");
        assert_eq!(items[0].1.as_deref(), Some("https://e.com"));
        assert_eq!(items[1].0, "second");
    }

    #[test]
    fn long_titles_with_emoji_trim_without_panic() {
        // Byte-slicing the old `&t[..93]` panics when 93 lands inside a
        // multi-byte sequence — exactly what emoji-bearing headlines do.
        let mut title = "a".repeat(89);
        title.push_str("😀😀 tail");
        let trimmed = trim_title(&title);
        assert!(trimmed.ends_with('…'));
        assert!(trimmed.chars().count() <= 92);
    }

    #[test]
    fn fit_trims_rows_until_the_datagram_is_small_enough() {
        // Realistic budget: Canvas rejects an oversized AF_UNIX datagram outright,
        // so the whole envelope must land under MAX_DATAGRAM with headroom.
        let budget = MAX_DATAGRAM - ENVELOPE_OVERHEAD;
        let mut rows = Vec::new();
        for n in 0..30 {
            rows.push(json!({
                "title": format!("a headline long enough to matter number {n} ----"),
                "source": "hn",
                "score": n,
                "by": "author",
                "url": "https://example.com/story",
                "age_secs": 1234,
            }));
        }
        let event = json!({ "schema_version": 1, "owner": "feeds", "stream": "hn",
            "contract": "canvas.widget.snapshot.v1", "sequence": 1, "emitted_at_ms": 0,
            "payload": { "title": "Headlines", "source": "hn", "rows": rows } });
        let full = serde_json::to_vec(&event).unwrap().len();
        assert!(full > budget, "precondition: {full} > {budget}");
        let (trimmed, thinned) = fit(event, budget);
        let bytes = serde_json::to_vec(&trimmed).unwrap().len();
        assert!(bytes <= budget, "fit left {bytes} bytes > {budget}");
        assert!(thinned, "dropped rows must be reported as partial");
        assert!(
            trimmed
                .pointer("payload/rows")
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len)
                < 30,
            "some rows must survive"
        );
    }

    #[test]
    fn envelope_matches_the_snapshot_contract_shape() {
        let feed = Feed {
            title: "HN".into(),
            source: "hn",
            rows: vec![Headline {
                title: "t".into(),
                url: None,
                score: Some(1),
                by: None,
                age_secs: None,
                source: "hn",
            }],
        };
        let ev = envelope("feeds", "hn-headlines", &feed, 3, 123);
        assert_eq!(ev["schema_version"], 1);
        assert_eq!(ev["owner"], "feeds");
        assert_eq!(ev["stream"], "hn-headlines");
        assert_eq!(ev["contract"], CONTRACT);
        assert_eq!(ev["sequence"], 3);
        assert_eq!(ev["emitted_at_ms"], 123);
        assert_eq!(ev["payload"]["source"], "hn");
        assert_eq!(ev["payload"]["rows"][0]["title"], "t");
    }
}
