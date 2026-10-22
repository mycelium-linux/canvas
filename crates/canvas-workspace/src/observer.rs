//! Ephemeral rendering observations. These neither grant nor claim control.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
/// Global presentation slot for the shared assistant voice capsule.
pub const VOICE_INDICATOR_ID: &str = "jarvis-voice";
/// Maximum receiver-owned lifetime of a transient assistant observation.
pub const MAX_TTL_MS: u16 = 5000;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub source: String,
    pub session: String,
    pub revision: u64,
    pub activity: fpl_gfx::reactor::Activity,
    pub ttl_ms: u16,
    pub caption: Option<String>,
    /// Measured playback level in thousandths; None is unavailable, not silence.
    pub playback_level: Option<u16>,
}
#[derive(Clone, Default)]
pub struct Observation(Option<(Frame, Instant)>);
impl Observation {
    pub fn observe(&mut self, frame: Option<Frame>, now: Instant) -> Result<(), String> {
        if let Some(next) = &frame {
            let name =
                |s: &str| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control);
            if !name(&next.source)
                || !name(&next.session)
                || !(1..=MAX_TTL_MS).contains(&next.ttl_ms)
                || next.playback_level.is_some_and(|v| v > 1000)
                || next
                    .caption
                    .as_ref()
                    .is_some_and(|s| s.len() > 512 || s.chars().any(char::is_control))
            {
                return Err("invalid display observation".into());
            }
            if let Some((previous, _)) = &self.0 {
                let mut normalized = next.clone();
                normalized.ttl_ms = previous.ttl_ms;
                if next.source == previous.source
                    && next.session == previous.session
                    && (next.revision < previous.revision
                        || next.revision == previous.revision && &normalized != previous)
                {
                    return Err("superseded or conflicting display observation".into());
                }
            }
        }
        self.0 = frame.map(|f| (f, now));
        Ok(())
    }
    pub fn current(&self, now: Instant) -> Option<&Frame> {
        let (frame, received) = self.0.as_ref()?;
        (now.saturating_duration_since(*received) < Duration::from_millis(u64::from(frame.ttl_ms)))
            .then_some(frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Frame {
        Frame {
            source: "state-producer".into(),
            session: "session".into(),
            revision: 2,
            activity: fpl_gfx::reactor::Activity::Thinking,
            ttl_ms: 1000,
            caption: None,
            playback_level: None,
        }
    }
    #[test]
    fn observation_expires_rejects_conflicts_and_clears_on_disconnect() {
        let now = Instant::now();
        let mut state = Observation::default();
        state.observe(Some(frame()), now).unwrap();
        assert!(state.current(now + Duration::from_millis(1000)).is_none());
        let mut old = frame();
        old.revision = 1;
        assert!(state.observe(Some(old), now).is_err());
        let mut conflict = frame();
        conflict.caption = Some("changed".into());
        assert!(state.observe(Some(conflict), now).is_err());
        state
            .observe(Some(frame()), now + Duration::from_millis(900))
            .unwrap();
        assert!(state.current(now + Duration::from_millis(1500)).is_some());
        let mut heartbeat = frame();
        heartbeat.ttl_ms = 100;
        state.observe(Some(heartbeat), now).unwrap();
        assert!(state.current(now + Duration::from_millis(99)).is_some());
        assert!(state.current(now + Duration::from_millis(100)).is_none());
        state.observe(None, now).unwrap();
        assert!(state.current(now).is_none());
    }
    #[test]
    fn input_bounds_are_atomic() {
        let now = Instant::now();
        let mut state = Observation::default();
        state.observe(Some(frame()), now).unwrap();
        for value in [0, 5001] {
            let mut bad = frame();
            bad.ttl_ms = value;
            assert!(state.observe(Some(bad), now).is_err());
        }
        let mut bad = frame();
        bad.playback_level = Some(1001);
        assert!(state.observe(Some(bad), now).is_err());
        assert_eq!(state.current(now), Some(&frame()));
    }
}
