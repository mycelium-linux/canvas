//! Presentation leases, not discovery or capture. A retained stream cannot
//! outlive its producer observation, even if the observer process disappears.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const LEASE_MS: u64 = 5_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLease {
    pub kind: String,
    pub schema_version: u32,
    pub url: String,
    pub generation: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl StreamLease {
    pub fn active(&self, now: u64) -> bool {
        self.kind == "live_stream"
            && self.schema_version == 1
            && self.url.len() <= 2048
            && (self.url.starts_with("http://") || self.url.starts_with("https://"))
            && !self.generation.is_empty()
            && self.generation.len() <= 128
            && self
                .generation
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && self.issued_at_unix_ms <= now.saturating_add(1_000)
            && self.expires_at_unix_ms > now
            && self.expires_at_unix_ms > self.issued_at_unix_ms
            && self.expires_at_unix_ms - self.issued_at_unix_ms <= LEASE_MS
    }
}

pub fn waiting_content() -> Value {
    failure_content(
        "Waiting for media progress",
        "The replay has not supplied fresh media yet.",
        "Playback resumes automatically after media advances. If this persists, check the replay producer and available disk space.",
    )
}

/// Reuse the scene contract so failures remain visible on older endpoints too.
/// Callers supply operator-facing causes, never raw credentials or commands.
pub fn failure_content(cause: &str, impact: &str, recovery: &str) -> Value {
    json!({"kind":"scene", "scene":{"nodes":[{"id":"stream-waiting", "content":{
        "kind":"text", "text":format!("{cause}\n{impact}\n{recovery}")}}]}})
}

/// Unknown content stays intact. Older endpoints retain `live_stream` as an
/// unsupported surface rather than silently treating an expired lease as live.
pub fn present_content(content: &Value, now: u64) -> Value {
    if content["kind"] != "live_stream" {
        return content.clone();
    }
    match serde_json::from_value::<StreamLease>(content.clone()) {
        Ok(lease) if lease.active(now) => {
            let separator = if lease.url.contains('?') { '&' } else { '?' };
            json!({"kind":"stream", "url":format!("{}{separator}canvas_generation={}", lease.url, lease.generation)})
        }
        _ => failure_content(
            "Replay observation expired",
            "Fresh media is no longer confirmed; the retained video has been hidden.",
            "Playback resumes when the observer confirms progress. Check the observer, replay producer, host sleep and network connection.",
        ),
    }
}

/// Normalize only a disposable renderer projection, never the retained state.
/// Lease renewals leave geometry/content stable; expiry changes to a scene.
pub fn present_projection(snapshot: &mut Value, now: u64) {
    if let Some(surfaces) = snapshot["document"]["surfaces"].as_array_mut() {
        for surface in surfaces {
            surface["content"] = present_content(&surface["content"], now);
        }
    }
    if let Some(tiles) = snapshot["tiles"].as_array_mut() {
        for tile in tiles {
            if !tile["surface"].is_null() {
                tile["surface"]["content"] = present_content(&tile["surface"]["content"], now);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsProgress {
    sequence: u64,
    digest: String,
}

pub fn hls_progress(raw: &str) -> Result<HlsProgress, &'static str> {
    if raw.len() > 65_536 || !raw.starts_with("#EXTM3U") {
        return Err("Invalid or oversized HLS playlist");
    }
    if raw.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
        return Err("Producer ended");
    }
    let sequence: u64 = raw
        .lines()
        .find_map(|line| line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .ok_or("Missing media sequence")?
        .trim()
        .parse()
        .map_err(|_| "Invalid media sequence")?;
    if !raw
        .lines()
        .any(|line| !line.trim().is_empty() && !line.starts_with('#'))
    {
        return Err("No media segments");
    }
    // Metadata/comment rewrites are not media progress. Hash only the media
    // identities and sequence, never mutable playlist commentary.
    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    for line in raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }
    Ok(HlsProgress {
        sequence,
        digest: format!("{:x}", digest.finalize()),
    })
}

#[derive(Default)]
pub struct ProgressTracker {
    previous: Option<HlsProgress>,
    generation: Option<String>,
    last_advance: Option<u64>,
}

impl ProgressTracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// One fresh-looking playlist is insufficient: require observed progress.
    /// Identical playlists never renew a lease. A sequence rollback starts a
    /// new generation so decoders cannot reuse the previous producer session.
    pub fn observe(&mut self, marker: HlsProgress, url: &str, now: u64) -> Option<StreamLease> {
        if let Some(previous) = &self.previous
            && previous != &marker
        {
            if self.generation.is_none()
                || marker.sequence < previous.sequence
                || self
                    .last_advance
                    .is_none_or(|last| now.saturating_sub(last) >= LEASE_MS)
            {
                self.generation = Some(format!("{now}-{}", &marker.digest[..16]));
            }
            self.last_advance = Some(now);
        }
        self.previous = Some(marker);
        let issued = self.last_advance?;
        if now.saturating_sub(issued) >= LEASE_MS {
            return None;
        }
        Some(StreamLease {
            kind: "live_stream".into(),
            schema_version: 1,
            url: url.into(),
            generation: self.generation.clone()?,
            issued_at_unix_ms: issued,
            expires_at_unix_ms: issued.saturating_add(LEASE_MS),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn marker(seq: u64) -> HlsProgress {
        hls_progress(&format!(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:{seq}\n#EXTINF:1,\nsegment-{seq}.ts\n"
        ))
        .unwrap()
    }
    #[test]
    fn requires_progress_expires_without_renewal_and_recovers() {
        let mut tracker = ProgressTracker::default();
        assert!(
            tracker
                .observe(marker(1), "http://127.0.0.1/live", 1000)
                .is_none()
        );
        let lease = tracker
            .observe(marker(2), "http://127.0.0.1/live", 2000)
            .unwrap();
        assert!(lease.active(6999));
        assert!(!lease.active(7000));
        assert!(tracker.observe(marker(2), &lease.url, 7000).is_none());
        let next = tracker.observe(marker(3), &lease.url, 8000).unwrap();
        assert_ne!(lease.generation, next.generation);
        assert!(next.active(8000));
    }
    #[test]
    fn restart_forces_new_generation_and_reset_requires_progress_again() {
        let mut tracker = ProgressTracker::default();
        tracker.observe(marker(10), "http://127.0.0.1/live", 1000);
        let before = tracker
            .observe(marker(11), "http://127.0.0.1/live", 2000)
            .unwrap();
        let after = tracker.observe(marker(0), &before.url, 3000).unwrap();
        assert_ne!(before.generation, after.generation);
        tracker.reset();
        assert!(tracker.observe(marker(0), &before.url, 4000).is_none());
    }
    #[test]
    fn rejects_ended_malformed_and_oversized_playlists() {
        for raw in [
            "garbage",
            "#EXTM3U\n#EXT-X-ENDLIST",
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0",
        ] {
            assert!(hls_progress(raw).is_err());
        }
        assert!(hls_progress(&"a".repeat(65_537)).is_err());
    }
    #[test]
    fn metadata_rewrites_do_not_keep_dead_media_alive() {
        let first = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#comment:first\n#EXTINF:1,\nsegment.ts\n";
        let second = first.replace("comment:first", "comment:updated");
        assert_eq!(hls_progress(first).unwrap(), hls_progress(&second).unwrap());
    }
    #[test]
    fn renewals_preserve_presentation_but_expiry_revokes_it() {
        let mut tracker = ProgressTracker::default();
        tracker.observe(marker(1), "http://127.0.0.1/live", 1000);
        let first = serde_json::to_value(
            tracker
                .observe(marker(2), "http://127.0.0.1/live", 2000)
                .unwrap(),
        )
        .unwrap();
        let second = serde_json::to_value(
            tracker
                .observe(marker(3), "http://127.0.0.1/live", 3000)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            present_content(&first, 3000),
            present_content(&second, 3000)
        );
        assert_eq!(present_content(&second, 8000)["kind"], "scene");
        assert_eq!(first["kind"], "live_stream");
    }
    #[test]
    fn malformed_or_far_future_leases_fail_closed() {
        assert_eq!(
            present_content(&json!({"kind":"live_stream"}), 1000)["kind"],
            "scene"
        );
        let invalid = json!({"kind":"live_stream","schema_version":1,"url":"http://127.0.0.1/", "generation":"x", "issued_at_unix_ms":1000,"expires_at_unix_ms":90000});
        assert_eq!(present_content(&invalid, 1000)["kind"], "scene");
    }
}
