//! Renderer-owned audio control. Requested gain is not evidence of applied gain.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    pub expected_revision: Option<u64>,
    pub command: Command,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    AudioInspect {},
    AudioSetMuted { source: Source, muted: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub surface_id: String,
    pub stream_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub source: Option<Source>,
    pub ready: bool,
    /// Confirmed by the renderer, not merely requested by a controller.
    pub muted: bool,
    pub pending: Option<Pending>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pending {
    pub request_id: String,
    pub revision: u64,
    pub muted: bool,
}

#[derive(Debug, Clone)]
pub struct AudioControl(Snapshot, Option<(Snapshot, std::time::Instant)>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlNotice {
    pub source: Source,
    pub state: &'static str,
}

impl Default for AudioControl {
    fn default() -> Self {
        Self(
            Snapshot {
                revision: 1,
                source: None,
                ready: false,
                muted: false,
                pending: None,
            },
            None,
        )
    }
}

impl AudioControl {
    pub fn snapshot(&self) -> Snapshot {
        self.0.clone()
    }

    /// A pending operation or a short-lived, exact renderer acknowledgement.
    /// Never derives control ownership from assistant display state.
    pub fn notice(&self, now: std::time::Instant) -> Option<ControlNotice> {
        if !self.0.ready {
            return None;
        }
        let source = self.0.source.clone()?;
        let state = if self.0.pending.is_some() {
            "pending"
        } else {
            let (confirmed, received) = self.1.as_ref()?;
            if confirmed != &self.0
                || now.saturating_duration_since(*received) >= std::time::Duration::from_secs(2)
            {
                return None;
            }
            "confirmed"
        };
        Some(ControlNotice { source, state })
    }

    /// Only the renderer supplies source identity and actual player state.
    /// Source changes and human control revoke any outstanding request.
    pub fn observe(&mut self, source: Option<Source>, ready: bool, muted: bool) {
        let ready = ready && source.is_some();
        if self.0.source != source || self.0.ready != ready || self.0.muted != muted {
            self.1 = None;
            self.0.revision += 1;
            self.0.source = source;
            self.0.ready = ready;
            self.0.muted = muted;
            self.0.pending = None;
        }
    }

    pub fn request(
        &mut self,
        request_id: &str,
        revision: u64,
        source: &Source,
        muted: bool,
    ) -> Result<Snapshot, &'static str> {
        if request_id.trim().is_empty() || request_id.len() > 128 {
            return Err("invalid_request_id");
        }
        if revision != self.0.revision {
            return Err("stale_state");
        }
        if self.0.source.as_ref() != Some(source) {
            return Err("source_changed");
        }
        if !self.0.ready {
            return Err("renderer_unavailable");
        }
        if let Some(pending) = &self.0.pending {
            if pending.request_id == request_id && pending.muted == muted {
                return Ok(self.snapshot());
            }
            return Err("request_in_progress");
        }
        if muted != self.0.muted {
            self.1 = None;
            self.0.pending = Some(Pending {
                request_id: request_id.into(),
                revision,
                muted,
            });
        }
        Ok(self.snapshot())
    }

    /// Called after applying gain to the exact observed player on its UI thread.
    pub fn acknowledge(
        &mut self,
        request_id: &str,
        revision: u64,
        source: &Source,
        muted: bool,
    ) -> Result<Snapshot, &'static str> {
        let Some(pending) = &self.0.pending else {
            return Err("no_pending_request");
        };
        if pending.request_id != request_id
            || pending.revision != revision
            || pending.muted != muted
            || self.0.revision != revision
            || self.0.source.as_ref() != Some(source)
            || !self.0.ready
        {
            return Err("stale_acknowledgement");
        }
        self.0.muted = muted;
        self.0.pending = None;
        self.0.revision += 1;
        self.1 = Some((self.0.clone(), std::time::Instant::now()));
        Ok(self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notices_require_exact_owner_state_and_expire() {
        let mut audio = AudioControl::default();
        let source = source("a");
        audio.observe(Some(source.clone()), true, true);
        assert!(audio.notice(std::time::Instant::now()).is_none());
        let revision = audio.snapshot().revision;
        audio.request("request", revision, &source, false).unwrap();
        assert_eq!(
            audio.notice(std::time::Instant::now()).unwrap().state,
            "pending"
        );
        audio
            .acknowledge("request", revision, &source, false)
            .unwrap();
        let now = std::time::Instant::now();
        assert_eq!(audio.notice(now).unwrap().state, "confirmed");
        assert!(
            audio
                .notice(now + std::time::Duration::from_secs(2))
                .is_none()
        );
        audio.observe(Some(super::tests::source("b")), true, false);
        assert!(audio.notice(now).is_none());
    }
    fn source(stream: &str) -> Source {
        Source {
            surface_id: "video".into(),
            stream_id: stream.into(),
            title: "Live video".into(),
        }
    }
    #[test]
    fn pending_is_not_applied_and_exact_renderer_ack_is_required() {
        let mut audio = AudioControl::default();
        let source = source("stream-a");
        audio.observe(Some(source.clone()), true, true);
        let revision = audio.snapshot().revision;
        let pending = audio.request("r1", revision, &source, false).unwrap();
        assert!(pending.muted);
        assert!(pending.pending.is_some());
        assert!(
            audio
                .acknowledge("other", revision, &source, false)
                .is_err()
        );
        assert!(
            !audio
                .acknowledge("r1", revision, &source, false)
                .unwrap()
                .muted
        );
    }
    #[test]
    fn stream_switch_revokes_pending_and_late_ack() {
        let mut audio = AudioControl::default();
        let source = source("stream-a");
        audio.observe(Some(source.clone()), true, true);
        let revision = audio.snapshot().revision;
        audio.request("r1", revision, &source, false).unwrap();
        audio.observe(Some(super::tests::source("stream-b")), true, true);
        assert!(audio.snapshot().pending.is_none());
        assert!(audio.acknowledge("r1", revision, &source, false).is_err());
        assert!(audio.snapshot().muted);
    }
    #[test]
    fn stale_busy_unavailable_and_idempotent_requests_are_bounded() {
        let mut audio = AudioControl::default();
        let source = source("stream-a");
        audio.observe(Some(source.clone()), false, false);
        assert_eq!(
            audio
                .request("r1", audio.snapshot().revision, &source, true)
                .unwrap_err(),
            "renderer_unavailable"
        );
        audio.observe(Some(source.clone()), true, false);
        let revision = audio.snapshot().revision;
        assert_eq!(
            audio
                .request("r1", revision - 1, &source, true)
                .unwrap_err(),
            "stale_state"
        );
        assert!(
            audio
                .request("noop", revision, &source, false)
                .unwrap()
                .pending
                .is_none()
        );
        let first = audio.request("r1", revision, &source, true).unwrap();
        assert_eq!(audio.request("r1", revision, &source, true).unwrap(), first);
        assert_eq!(
            audio.request("r2", revision, &source, true).unwrap_err(),
            "request_in_progress"
        );
    }

    #[test]
    fn human_control_and_lost_player_revoke_pending_work() {
        for ready in [true, false] {
            let mut audio = AudioControl::default();
            let source = source("stream-a");
            audio.observe(Some(source.clone()), true, false);
            let revision = audio.snapshot().revision;
            audio.request("r1", revision, &source, true).unwrap();
            audio.observe(Some(source.clone()), ready, true);
            assert!(audio.snapshot().pending.is_none());
            assert!(audio.acknowledge("r1", revision, &source, true).is_err());
        }
    }

    #[test]
    fn transport_rejects_unknown_fields_and_non_boolean_gain() {
        for raw in [
            r#"{"schema_version":1,"request_id":"r","command":{"command":"audio_inspect","shell":"never"}}"#,
            r#"{"schema_version":1,"request_id":"r","command":{"command":"audio_set_muted","source":{"surface_id":"v","stream_id":"s","title":"Video"},"muted":"false"}}"#,
        ] {
            assert!(serde_json::from_str::<Request>(raw).is_err());
        }
    }
}
