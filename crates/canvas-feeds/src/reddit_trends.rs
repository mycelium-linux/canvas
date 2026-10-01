//! Reddit RSS adapter for the shared Canvas social-trends contract.
use crate::x_trends::{FeedStatus, Health, Payload, TrendItem};
use crate::{FeedKind, Http, fetch};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "one")]
    pub schema_version: u32,
    #[serde(default = "owner")]
    pub owner: String,
    #[serde(default = "stream")]
    pub stream: String,
    #[serde(default = "refresh")]
    pub refresh_seconds: u64,
    #[serde(default = "limit")]
    pub max_items: usize,
    pub subreddits: Vec<String>,
}
fn one() -> u32 {
    1
}
fn owner() -> String {
    "feeds".into()
}
fn stream() -> String {
    "reddit-war-room".into()
}
fn refresh() -> u64 {
    60
}
fn limit() -> usize {
    12
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.subreddits.is_empty() || self.subreddits.len() > 12 {
            return Err("Reddit config requires schema_version 1 and 1-12 subreddits".into());
        }
        if !(30..=3600).contains(&self.refresh_seconds) || !(1..=50).contains(&self.max_items) {
            return Err("Reddit refresh/max_items are out of range".into());
        }
        if self
            .subreddits
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err("invalid subreddit name".into());
        }
        Ok(())
    }
}

pub fn refresh_feed(
    config: &Config,
    http: &dyn Http,
    now_ms: u64,
    previous: &[TrendItem],
) -> Payload {
    let mut unique: BTreeMap<_, _> = previous
        .iter()
        .cloned()
        .map(|item| (item.id.clone(), item))
        .collect();
    let cycle = now_ms / (config.refresh_seconds * 1000);
    let sub = &config.subreddits[cycle as usize % config.subreddits.len()];
    let mut failures = Vec::new();
    let mut successful = 0;
    match fetch(&FeedKind::Reddit { sub: sub.clone() }, http) {
        Ok(feed) => {
            successful = 1;
            for (index, row) in feed.rows.into_iter().enumerate() {
                let id = row
                    .url
                    .clone()
                    .unwrap_or_else(|| format!("reddit:{sub}:{}", row.title));
                unique.entry(id.clone()).or_insert(TrendItem {
                    id,
                    text: row.title,
                    author: row.by.unwrap_or_else(|| "reddit".into()),
                    url: row
                        .url
                        .unwrap_or_else(|| format!("https://reddit.com/r/{sub}")),
                    created_at_ms: now_ms,
                    age_secs: row.age_secs.unwrap_or(0),
                    engagement_score: row.score.unwrap_or(0),
                    rank_score: 100_000u64.saturating_sub(index as u64),
                    matches: vec![format!("r-{sub}")],
                });
            }
        }
        Err(error) => failures.push(format!("r/{sub}: {error}")),
    }
    let mut items: Vec<_> = if successful == 0 {
        previous.to_vec()
    } else {
        unique.into_values().collect()
    };
    items.sort_by(|a, b| {
        b.rank_score
            .cmp(&a.rank_score)
            .then_with(|| a.id.cmp(&b.id))
    });
    items.truncate(config.max_items);
    Payload {
        title: "Reddit / War Room".into(),
        status: FeedStatus {
            health: if failures.is_empty() {
                Health::Healthy
            } else {
                Health::Degraded
            },
            mode: "polling",
            detail: if failures.is_empty() {
                format!("r/{sub} refreshed · {} configured", config.subreddits.len())
            } else {
                failures.join("; ")
            },
            successful_queries: successful,
            failed_queries: failures.len(),
            last_success_at_ms: (successful > 0).then_some(now_ms),
            next_refresh_at_ms: now_ms + config.refresh_seconds * 1000,
        },
        items,
    }
}
