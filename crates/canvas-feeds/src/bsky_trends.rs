//! Bluesky AppView adapter for the shared Canvas social-trends contract.
use crate::x_trends::{ApiPost, ApiResponse, ApiUser, Includes, Metrics, SavedFeed, XApi};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PDS: &str = "https://bsky.social";

pub struct BlueskyApi {
    client: reqwest::blocking::Client,
    pds: String,
    access_jwt: String,
}
impl BlueskyApi {
    pub fn login(pds: &str, identifier: &str, app_password: &str) -> Result<Self, String> {
        let pds = pds.trim_end_matches('/');
        if !pds.starts_with("https://") {
            return Err("Bluesky PDS must use https".into());
        }
        if identifier.trim().is_empty() || app_password.trim().is_empty() {
            return Err("BSKY_IDENTIFIER and BSKY_APP_PASSWORD are required for the home timeline".into());
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent("canvas-bsky-trends/0.1")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .post(format!("{pds}/xrpc/com.atproto.server.createSession"))
            .json(&SessionRequest { identifier, password: app_password })
            .send()
            .map_err(|e| format!("Bluesky login failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("Bluesky login returned HTTP {}", response.status()));
        }
        let session: SessionResponse = response
            .json()
            .map_err(|e| format!("invalid Bluesky login response: {e}"))?;
        Ok(Self {
            client,
            pds: pds.into(),
            access_jwt: session.access_jwt,
        })
    }
}

#[derive(Serialize)]
struct SessionRequest<'a> {
    identifier: &'a str,
    password: &'a str,
}
#[derive(Deserialize)]
struct SessionResponse {
    #[serde(rename = "accessJwt")]
    access_jwt: String,
}
#[derive(Debug, Deserialize)]
struct TimelineResponse {
    #[serde(default)]
    feed: Vec<FeedView>,
}
#[derive(Debug, Deserialize)]
struct FeedView {
    post: PostView,
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
    fn recent(&self, _saved: &SavedFeed) -> Result<ApiResponse, String> {
        let response = self
            .client
            .get(format!("{}/xrpc/app.bsky.feed.getTimeline", self.pds))
            .bearer_auth(&self.access_jwt)
            .query(&[("limit", "100")])
            .send()
            .map_err(|e| format!("Bluesky timeline request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Bluesky timeline returned HTTP {}",
                response.status()
            ));
        }
        let response: TimelineResponse = response
            .json()
            .map_err(|e| format!("invalid Bluesky timeline response: {e}"))?;
        let posts = response.feed.into_iter().map(|item| item.post);
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
    fn pds_requires_tls() {
        assert!(BlueskyApi::login("http://example.test", "me.test", "secret").is_err());
    }
    #[test]
    fn home_timeline_requires_credentials() {
        assert!(BlueskyApi::login("https://example.test", "", "").is_err());
    }
}
