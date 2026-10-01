//! Bluesky AppView adapter for the shared Canvas social-trends contract.
use crate::x_trends::{ApiPost, ApiResponse, ApiUser, Includes, Metrics, SavedFeed, XApi};
use serde::Deserialize;

pub const DEFAULT_APPVIEW: &str = "https://api.bsky.app";

pub struct BlueskyApi {
    client: reqwest::blocking::Client,
    appview: String,
}
impl BlueskyApi {
    pub fn new(appview: &str) -> Result<Self, String> {
        let appview = appview.trim_end_matches('/');
        if !appview.starts_with("https://") {
            return Err("Bluesky AppView must use https".into());
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent("canvas-bsky-trends/0.1")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            appview: appview.into(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    posts: Vec<PostView>,
}
#[derive(Debug, Deserialize)]
struct PostView {
    uri: String,
    author: Author,
    record: Record,
    #[serde(default, rename = "likeCount")]
    likes: u64,
    #[serde(default, rename = "repostCount")]
    reposts: u64,
    #[serde(default, rename = "replyCount")]
    replies: u64,
    #[serde(default, rename = "quoteCount")]
    quotes: u64,
}
#[derive(Debug, Deserialize)]
struct Author {
    did: String,
    handle: String,
}
#[derive(Debug, Deserialize)]
struct Record {
    text: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

impl XApi for BlueskyApi {
    fn recent(&self, saved: &SavedFeed) -> Result<ApiResponse, String> {
        let mut terms = Vec::new();
        if !saved.query.trim().is_empty() {
            terms.push(saved.query.trim().to_owned());
        }
        terms.extend(saved.topics.iter().cloned());
        let query = if terms.is_empty() {
            "*".into()
        } else {
            terms.join(" OR ")
        };
        let authors: Vec<Option<&str>> = if saved.accounts.is_empty() {
            vec![None]
        } else {
            saved
                .accounts
                .iter()
                .map(|v| Some(v.trim_start_matches('@')))
                .collect()
        };
        let mut posts = Vec::new();
        for author in authors {
            let mut request = self
                .client
                .get(format!("{}/xrpc/app.bsky.feed.searchPosts", self.appview))
                .query(&[("q", query.as_str()), ("sort", "latest"), ("limit", "100")]);
            if let Some(author) = author {
                request = request.query(&[("author", author)]);
            }
            if let Some(language) = saved.language.as_deref() {
                request = request.query(&[("lang", language)]);
            }
            let response = request
                .send()
                .map_err(|e| format!("Bluesky request failed: {e}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "Bluesky AppView returned HTTP {}",
                    response.status()
                ));
            }
            let response: SearchResponse = response
                .json()
                .map_err(|e| format!("invalid Bluesky response: {e}"))?;
            posts.extend(response.posts);
        }
        let mut users = std::collections::BTreeMap::new();
        let data = posts
            .into_iter()
            .map(|post| {
                users.insert(post.author.did.clone(), post.author.handle.clone());
                let rkey = post.uri.rsplit('/').next().unwrap_or("");
                ApiPost {
                    id: post.uri.clone(),
                    text: post.record.text,
                    author_id: Some(post.author.did.clone()),
                    created_at: post.record.created_at,
                    url: Some(format!(
                        "https://bsky.app/profile/{}/post/{rkey}",
                        post.author.did
                    )),
                    public_metrics: Metrics {
                        like_count: post.likes,
                        repost_count: post.reposts,
                        reply_count: post.replies,
                        quote_count: post.quotes,
                        bookmark_count: 0,
                    },
                }
            })
            .collect();
        Ok(ApiResponse {
            data,
            includes: Includes {
                users: users
                    .into_iter()
                    .map(|(id, username)| ApiUser { id, username })
                    .collect(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appview_requires_tls() {
        assert!(BlueskyApi::new("http://example.test").is_err());
    }
}
