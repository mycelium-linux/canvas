//! Official X API recent-search producer and narrow Canvas trend contract.
//! X credentials stay in the host process; neither configuration snapshots nor
//! emitted events contain a token.

use chrono::DateTime;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CONTRACT: &str = "canvas.social.trends.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "one")]
    pub schema_version: u32,
    #[serde(default = "default_owner")]
    pub owner: String,
    #[serde(default = "default_stream")]
    pub stream: String,
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    #[serde(default = "default_limit")]
    pub max_items: usize,
    #[serde(default = "default_half_life")]
    pub recency_half_life_minutes: u64,
    #[serde(default)]
    pub weights: EngagementWeights,
    pub saved: Vec<SavedFeed>,
}

fn one() -> u32 {
    1
}
fn default_owner() -> String {
    "feeds".into()
}
fn default_stream() -> String {
    "x-war-room".into()
}
fn default_refresh() -> u64 {
    30
}
fn default_limit() -> usize {
    12
}
fn default_half_life() -> u64 {
    90
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedFeed {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngagementWeights {
    pub likes: u64,
    pub reposts: u64,
    pub replies: u64,
    pub quotes: u64,
    pub bookmarks: u64,
}
impl Default for EngagementWeights {
    fn default() -> Self {
        Self {
            likes: 1,
            reposts: 3,
            replies: 2,
            quotes: 3,
            bookmarks: 1,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("schema_version must be 1".into());
        }
        if !(10..=3600).contains(&self.refresh_seconds) {
            return Err("refresh_seconds must be 10..=3600".into());
        }
        if !(1..=50).contains(&self.max_items) {
            return Err("max_items must be 1..=50".into());
        }
        if self.recency_half_life_minutes == 0 {
            return Err("recency_half_life_minutes must be positive".into());
        }
        if self.saved.is_empty() {
            return Err("at least one saved feed is required".into());
        }
        let mut ids = BTreeSet::new();
        for saved in &self.saved {
            if saved.id.is_empty()
                || !saved
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(format!("invalid saved feed id: {}", saved.id));
            }
            if !ids.insert(&saved.id) {
                return Err(format!("duplicate saved feed id: {}", saved.id));
            }
            if saved.query.trim().is_empty() && saved.topics.is_empty() && saved.accounts.is_empty()
            {
                return Err(format!(
                    "saved feed {} has no query, topics, or accounts",
                    saved.id
                ));
            }
        }
        Ok(())
    }
}

pub fn compile_query(saved: &SavedFeed) -> String {
    let mut groups = Vec::new();
    if !saved.query.trim().is_empty() {
        groups.push(format!("({})", saved.query.trim()));
    }
    if !saved.topics.is_empty() {
        groups.push(format!(
            "({})",
            saved
                .topics
                .iter()
                .map(|v| quote_term(v))
                .collect::<Vec<_>>()
                .join(" OR ")
        ));
    }
    if !saved.accounts.is_empty() {
        groups.push(format!(
            "({})",
            saved
                .accounts
                .iter()
                .map(|v| format!("from:{}", v.trim_start_matches('@')))
                .collect::<Vec<_>>()
                .join(" OR ")
        ));
    }
    if let Some(lang) = saved.language.as_deref().filter(|v| !v.is_empty()) {
        groups.push(format!("lang:{lang}"));
    }
    groups.join(" ")
}

fn quote_term(value: &str) -> String {
    let clean = value.trim().replace('"', "");
    if clean.contains(char::is_whitespace) {
        format!("\"{clean}\"")
    } else {
        clean
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Metrics {
    #[serde(default)]
    pub like_count: u64,
    #[serde(default, rename = "retweet_count")]
    pub repost_count: u64,
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub quote_count: u64,
    #[serde(default)]
    pub bookmark_count: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiPost {
    pub id: String,
    pub text: String,
    pub author_id: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub public_metrics: Metrics,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ApiUser {
    pub id: String,
    pub username: String,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Includes {
    #[serde(default)]
    pub users: Vec<ApiUser>,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiResponse {
    #[serde(default)]
    pub data: Vec<ApiPost>,
    #[serde(default)]
    pub includes: Includes,
}

pub trait XApi {
    fn recent(&self, saved: &SavedFeed) -> Result<ApiResponse, String>;
}

pub struct OfficialXApi {
    client: reqwest::blocking::Client,
    bearer: String,
}
impl OfficialXApi {
    pub fn new(bearer: String) -> Result<Self, String> {
        if bearer.trim().is_empty() {
            return Err("X bearer token is empty".into());
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent("canvas-x-trends/0.1")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, bearer })
    }
}
impl XApi for OfficialXApi {
    fn recent(&self, saved: &SavedFeed) -> Result<ApiResponse, String> {
        let mut url =
            Url::parse("https://api.x.com/2/tweets/search/recent").expect("static X API URL");
        url.query_pairs_mut()
            .append_pair("query", &compile_query(saved))
            .append_pair("max_results", "100")
            .append_pair("tweet.fields", "author_id,created_at,public_metrics")
            .append_pair("expansions", "author_id")
            .append_pair("user.fields", "username");
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.bearer)
            .send()
            .map_err(|e| format!("X request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("X API returned HTTP {}", response.status()));
        }
        response
            .json()
            .map_err(|e| format!("invalid X response: {e}"))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrendItem {
    pub id: String,
    pub text: String,
    pub author: String,
    pub url: String,
    pub created_at_ms: u64,
    pub age_secs: u64,
    pub engagement_score: u64,
    pub rank_score: u64,
    pub matches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    Degraded,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FeedStatus {
    pub health: Health,
    pub mode: &'static str,
    pub detail: String,
    pub successful_queries: usize,
    pub failed_queries: usize,
    pub last_success_at_ms: Option<u64>,
    pub next_refresh_at_ms: u64,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Payload {
    pub title: String,
    pub status: FeedStatus,
    pub items: Vec<TrendItem>,
}

pub fn refresh(config: &Config, api: &dyn XApi, now_ms: u64, previous: &[TrendItem]) -> Payload {
    refresh_named(config, api, now_ms, previous, "X / War Room")
}

pub fn refresh_named(
    config: &Config,
    api: &dyn XApi,
    now_ms: u64,
    previous: &[TrendItem],
    title: &str,
) -> Payload {
    let mut items: BTreeMap<String, TrendItem> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut successful = 0;
    for saved in &config.saved {
        match api.recent(saved) {
            Ok(response) => {
                successful += 1;
                let users: BTreeMap<_, _> = response
                    .includes
                    .users
                    .into_iter()
                    .map(|u| (u.id, u.username))
                    .collect();
                for post in response.data {
                    if !passes(&post.text, saved) {
                        continue;
                    }
                    let author = post
                        .author_id
                        .as_ref()
                        .and_then(|id| users.get(id))
                        .cloned()
                        .unwrap_or_else(|| "unknown".into());
                    let created_at_ms = DateTime::parse_from_rfc3339(&post.created_at)
                        .map(|d| d.timestamp_millis().max(0) as u64)
                        .unwrap_or(now_ms);
                    let age_secs = now_ms.saturating_sub(created_at_ms) / 1000;
                    let engagement = engagement(&post.public_metrics, &config.weights);
                    let rank_score = rank(engagement, age_secs, config.recency_half_life_minutes);
                    let url = post
                        .url
                        .clone()
                        .unwrap_or_else(|| format!("https://x.com/{author}/status/{}", post.id));
                    items
                        .entry(post.id.clone())
                        .and_modify(|item| item.matches.push(saved.id.clone()))
                        .or_insert(TrendItem {
                            url,
                            id: post.id,
                            text: compact(&post.text, 220),
                            author,
                            created_at_ms,
                            age_secs,
                            engagement_score: engagement,
                            rank_score,
                            matches: vec![saved.id.clone()],
                        });
                }
            }
            Err(error) => failures.push(format!("{}: {error}", saved.id)),
        }
    }
    let mut ranked: Vec<_> = items.into_values().collect();
    if successful == 0 {
        ranked = previous.to_vec();
        for item in &mut ranked {
            item.age_secs = now_ms.saturating_sub(item.created_at_ms) / 1000;
            item.rank_score = rank(
                item.engagement_score,
                item.age_secs,
                config.recency_half_life_minutes,
            );
        }
    }
    ranked.sort_by(|a, b| {
        b.rank_score
            .cmp(&a.rank_score)
            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    ranked.truncate(config.max_items);
    let degraded = !failures.is_empty();
    Payload {
        title: title.into(),
        status: FeedStatus {
            health: if degraded {
                Health::Degraded
            } else {
                Health::Healthy
            },
            mode: "polling",
            detail: if failures.is_empty() {
                format!("{} saved feeds live", successful)
            } else {
                failures.join("; ")
            },
            successful_queries: successful,
            failed_queries: failures.len(),
            last_success_at_ms: (successful > 0).then_some(now_ms),
            next_refresh_at_ms: now_ms + config.refresh_seconds * 1000,
        },
        items: ranked,
    }
}

fn passes(text: &str, saved: &SavedFeed) -> bool {
    let haystack = text.to_lowercase();
    (saved.include.is_empty()
        || saved
            .include
            .iter()
            .any(|v| haystack.contains(&v.to_lowercase())))
        && !saved
            .exclude
            .iter()
            .any(|v| haystack.contains(&v.to_lowercase()))
}
fn engagement(m: &Metrics, w: &EngagementWeights) -> u64 {
    m.like_count
        .saturating_mul(w.likes)
        .saturating_add(m.repost_count.saturating_mul(w.reposts))
        .saturating_add(m.reply_count.saturating_mul(w.replies))
        .saturating_add(m.quote_count.saturating_mul(w.quotes))
        .saturating_add(m.bookmark_count.saturating_mul(w.bookmarks))
}
fn rank(engagement: u64, age_secs: u64, half_life_minutes: u64) -> u64 {
    let half_life = half_life_minutes.saturating_mul(60).max(1);
    let recency = 100_000u64.saturating_mul(half_life) / half_life.saturating_add(age_secs);
    engagement.saturating_mul(1_000).saturating_add(recency)
}
fn compact(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    one_line
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

pub fn envelope(
    config: &Config,
    payload: &Payload,
    sequence: u64,
    emitted_at_ms: u64,
) -> serde_json::Value {
    serde_json::json!({"schema_version":1,"owner":config.owner,"stream":config.stream,"contract":CONTRACT,"sequence":sequence,"emitted_at_ms":emitted_at_ms,"payload":payload})
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        results: BTreeMap<String, Result<ApiResponse, String>>,
    }
    impl XApi for Fake {
        fn recent(&self, saved: &SavedFeed) -> Result<ApiResponse, String> {
            self.results.get(&compile_query(saved)).cloned().unwrap()
        }
    }
    fn saved(id: &str) -> SavedFeed {
        SavedFeed {
            id: id.into(),
            label: id.into(),
            query: "rust".into(),
            topics: vec![],
            accounts: vec![],
            include: vec![],
            exclude: vec![],
            language: Some("en".into()),
        }
    }
    fn config(saved: Vec<SavedFeed>) -> Config {
        Config {
            schema_version: 1,
            owner: "feeds".into(),
            stream: "x".into(),
            refresh_seconds: 30,
            max_items: 12,
            recency_half_life_minutes: 90,
            weights: EngagementWeights::default(),
            saved,
        }
    }
    fn response(id: &str, text: &str, likes: u64) -> ApiResponse {
        ApiResponse {
            data: vec![ApiPost {
                id: id.into(),
                text: text.into(),
                author_id: Some("u1".into()),
                created_at: "2026-09-30T12:00:00Z".into(),
                url: None,
                public_metrics: Metrics {
                    like_count: likes,
                    ..Default::default()
                },
            }],
            includes: Includes {
                users: vec![ApiUser {
                    id: "u1".into(),
                    username: "alice".into(),
                }],
            },
        }
    }
    #[test]
    fn query_compiles_topics_accounts_and_language() {
        let mut s = saved("a");
        s.query.clear();
        s.topics = vec!["machine learning".into()];
        s.accounts = vec!["@OpenAI".into()];
        assert_eq!(
            compile_query(&s),
            "(\"machine learning\") (from:OpenAI) lang:en"
        );
    }
    #[test]
    fn deduplicates_and_records_all_matches() {
        let a = saved("a");
        let b = saved("b");
        let q = compile_query(&a);
        let api = Fake {
            results: BTreeMap::from([(q, Ok(response("1", "Rust news", 5)))]),
        };
        let out = refresh(&config(vec![a, b]), &api, 1_780_228_000_000, &[]);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].matches, vec!["a", "b"]);
    }
    #[test]
    fn include_exclude_and_partial_failure_are_visible() {
        let mut a = saved("a");
        a.include = vec!["launch".into()];
        a.exclude = vec!["crypto".into()];
        let mut b = saved("b");
        b.query = "fail".into();
        let api = Fake {
            results: BTreeMap::from([
                (
                    compile_query(&a),
                    Ok(response("1", "launch without crypto", 5)),
                ),
                (compile_query(&b), Err("429".into())),
            ]),
        };
        let out = refresh(&config(vec![a, b]), &api, 1_780_228_000_000, &[]);
        assert!(out.items.is_empty());
        assert_eq!(out.status.health, Health::Degraded);
        assert_eq!(out.status.failed_queries, 1);
    }
    #[test]
    fn total_failure_keeps_last_snapshot() {
        let a = saved("a");
        let api = Fake {
            results: BTreeMap::from([(compile_query(&a), Err("offline".into()))]),
        };
        let old = TrendItem {
            id: "old".into(),
            text: "cached".into(),
            author: "a".into(),
            url: "u".into(),
            created_at_ms: 1,
            age_secs: 1,
            engagement_score: 1,
            rank_score: 1,
            matches: vec!["a".into()],
        };
        let out = refresh(&config(vec![a]), &api, 10, &[old.clone()]);
        assert_eq!(out.items[0].id, old.id);
        assert_eq!(out.items[0].text, old.text);
        assert!(out.items[0].rank_score > old.rank_score);
        assert_eq!(out.status.health, Health::Degraded);
    }
    #[test]
    fn ranking_balances_engagement_and_recency_deterministically() {
        let fresh = rank(1, 0, 60);
        let stale = rank(1, 86_400, 60);
        let popular_stale = rank(200, 86_400, 60);
        assert!(fresh > stale);
        assert!(popular_stale > fresh);
    }
    #[test]
    fn config_rejects_duplicate_ids() {
        assert!(
            config(vec![saved("a"), saved("a")])
                .validate()
                .unwrap_err()
                .contains("duplicate")
        );
    }
}
