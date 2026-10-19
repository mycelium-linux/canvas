//! Display-only room occupancy. No identities, sensing or execution authority.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const EMPTY_DELAY: Duration = Duration::from_secs(60);
const HUMAN_GRACE: Duration = Duration::from_secs(120);
pub const MAX_AGE_MS: u64 = 5000;
/// Bound host clock skew without granting a longer receiver-owned lease.
const MAX_FUTURE_SKEW_MS: u64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Occupancy {
    Empty,
    Occupied,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceFrame {
    pub room: String,
    pub state: Occupancy,
    pub observed_at_unix_ms: u64,
    pub valid_for_ms: u64,
}

#[derive(Clone, Default)]
pub struct Ambient {
    room: Option<String>,
    sample: Option<(PresenceFrame, Instant)>,
    empty_since: Option<Instant>,
    automatic: bool,
    manual: bool,
    human_until: Option<Instant>,
}

impl Ambient {
    pub fn room(&self) -> Option<&str> {
        self.room.as_deref()
    }

    pub fn bind(&mut self, room: Option<String>) -> Result<(), String> {
        if room.as_ref().is_some_and(|room| {
            room.is_empty()
                || room.len() > 64
                || !room
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }) {
            return Err("Invalid ambient room".into());
        }
        *self = Self {
            room,
            ..Self::default()
        };
        Ok(())
    }

    pub fn set(&mut self, enabled: bool, now: Instant) {
        if enabled {
            self.manual = true;
        } else {
            self.wake(now);
        }
    }

    /// Human input is consumed only when waking. Grace prevents immediate sleep.
    pub fn wake(&mut self, now: Instant) -> bool {
        self.tick(now);
        let was_active = self.active();
        self.manual = false;
        self.automatic = false;
        self.empty_since = None;
        self.human_until = Some(now + HUMAN_GRACE);
        was_active
    }

    pub fn tick(&mut self, now: Instant) {
        if self.sample.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.sample = None;
            self.empty_since = None;
            // An unavailable sensor must not reveal the previous display.
            // Retain an already-active saver until occupied or human wake.
        }
        if self.human_until.is_some_and(|until| now < until) {
            return;
        }
        if self
            .empty_since
            .is_some_and(|since| now.saturating_duration_since(since) >= EMPTY_DELAY)
            && self
                .sample
                .as_ref()
                .is_some_and(|(frame, _)| frame.state == Occupancy::Empty)
        {
            self.automatic = true;
        }
    }

    pub fn observe(
        &mut self,
        frame: PresenceFrame,
        unix_ms: u64,
        now: Instant,
    ) -> Result<(), String> {
        if self.room.as_deref() != Some(frame.room.as_str()) {
            return Err("Ambient room is not bound to this display".into());
        }
        let age = if frame.observed_at_unix_ms > unix_ms {
            if frame.observed_at_unix_ms - unix_ms > MAX_FUTURE_SKEW_MS {
                return Err("Ambient observation is in the future".into());
            }
            0 // Never add clock skew to the five-second receiver lease.
        } else {
            unix_ms - frame.observed_at_unix_ms
        };
        if frame.valid_for_ms == 0 || frame.valid_for_ms > MAX_AGE_MS || age >= frame.valid_for_ms {
            return Err("Ambient observation is stale or has invalid expiry".into());
        }
        if self.sample.as_ref().is_some_and(|(previous, _)| {
            previous.state != Occupancy::Unknown
                && frame.state != Occupancy::Unknown
                && frame.observed_at_unix_ms < previous.observed_at_unix_ms
        }) {
            return Err("Ambient observation regressed".into());
        }
        if let Some((previous, _)) = &self.sample {
            if previous.state != Occupancy::Unknown
                && frame.state != Occupancy::Unknown
                && previous.observed_at_unix_ms == frame.observed_at_unix_ms
            {
                if previous.state != frame.state || previous.valid_for_ms != frame.valid_for_ms {
                    return Err("Conflicting ambient observation".into());
                }
                self.tick(now);
                return Ok(()); // Replays never extend the receiver's deadline.
            }
        }
        self.tick(now);
        match frame.state {
            Occupancy::Occupied => {
                self.manual = false;
                self.automatic = false;
                self.empty_since = None;
            }
            Occupancy::Unknown => self.empty_since = None,
            Occupancy::Empty => {
                if self.human_until.is_none_or(|until| now >= until) {
                    self.empty_since.get_or_insert(now);
                }
            }
        }
        let remaining = frame.valid_for_ms - age;
        self.sample = Some((frame, now + Duration::from_millis(remaining)));
        self.tick(now);
        Ok(())
    }

    pub fn active(&self) -> bool {
        self.manual || self.automatic
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"active":self.active(),"room":self.room,
            "occupancy":self.sample.as_ref().map(|(frame,_)|frame.state).unwrap_or(Occupancy::Unknown),
            "reason":if self.manual {"manual"} else if self.automatic {"room_empty"} else {"awake"},
            "empty_delay_ms":EMPTY_DELAY.as_millis(),"human_grace_ms":HUMAN_GRACE.as_millis()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(state: Occupancy, time: u64) -> PresenceFrame {
        PresenceFrame {
            room: "main_room".into(),
            state,
            observed_at_unix_ms: time,
            valid_for_ms: 5000,
        }
    }
    fn bound() -> Ambient {
        let mut a = Ambient::default();
        a.bind(Some("main_room".into())).unwrap();
        a
    }
    #[test]
    fn continuous_empty_sleeps_and_presence_wakes() {
        let now = Instant::now();
        let mut a = bound();
        for second in 0..=60 {
            a.observe(
                frame(Occupancy::Empty, 1000 + second * 1000),
                1000 + second * 1000,
                now + Duration::from_secs(second),
            )
            .unwrap();
        }
        assert!(a.active());
        a.observe(
            frame(Occupancy::Occupied, 62000),
            62000,
            now + Duration::from_secs(61),
        )
        .unwrap();
        assert!(!a.active());
    }
    #[test]
    fn absent_unknown_stale_and_gapped_samples_do_not_sleep() {
        let now = Instant::now();
        let mut a = bound();
        a.tick(now + EMPTY_DELAY);
        assert!(!a.active());
        a.observe(frame(Occupancy::Empty, 1000), 1000, now).unwrap();
        a.tick(now + EMPTY_DELAY);
        assert!(!a.active());
        a.observe(
            frame(Occupancy::Empty, 62000),
            62000,
            now + Duration::from_secs(61),
        )
        .unwrap();
        assert!(!a.active());
        a.observe(
            frame(Occupancy::Unknown, 63000),
            63000,
            now + Duration::from_secs(62),
        )
        .unwrap();
        a.tick(now + Duration::from_secs(130));
        assert!(!a.active());
    }
    #[test]
    fn wrong_room_future_stale_and_replay_are_bounded() {
        let now = Instant::now();
        let mut a = bound();
        let mut f = frame(Occupancy::Empty, 1000);
        f.room = "other".into();
        assert!(a.observe(f, 1000, now).is_err());
        assert!(a.observe(frame(Occupancy::Empty, 4000), 1000, now).is_err());
        assert!(a.observe(frame(Occupancy::Empty, 1000), 6000, now).is_err());
        a.observe(frame(Occupancy::Empty, 1000), 1000, now).unwrap();
        a.observe(
            frame(Occupancy::Empty, 1000),
            2000,
            now + Duration::from_secs(4),
        )
        .unwrap();
        a.tick(now + Duration::from_secs(5));
        assert!(a.sample.is_none());
    }
    #[test]
    fn remote_wake_has_grace_and_signal_loss_does_not_reveal() {
        let now = Instant::now();
        let mut a = bound();
        a.set(true, now);
        a.tick(now + EMPTY_DELAY);
        assert!(a.active());
        assert!(a.wake(now));
        assert!(!a.active());
        for second in 1..120 {
            a.observe(
                frame(Occupancy::Empty, 1000 + second * 1000),
                1000 + second * 1000,
                now + Duration::from_secs(second),
            )
            .unwrap();
        }
        assert!(!a.active());
    }
    #[test]
    fn unknown_classification_does_not_block_recovered_sensor_time() {
        let now = Instant::now();
        let mut a = bound();
        a.observe(frame(Occupancy::Unknown, 2000), 2000, now)
            .unwrap();
        a.observe(
            frame(Occupancy::Occupied, 1500),
            2500,
            now + Duration::from_millis(500),
        )
        .unwrap();
        assert_eq!(a.snapshot()["occupancy"], "occupied");
    }
    #[test]
    fn small_host_clock_skew_never_extends_receiver_lease() {
        let now = Instant::now();
        let mut a = bound();
        a.observe(frame(Occupancy::Empty, 2500), 1000, now).unwrap();
        a.tick(now + Duration::from_millis(MAX_AGE_MS));
        assert!(a.sample.is_none());
        assert!(!a.active());
    }
}
