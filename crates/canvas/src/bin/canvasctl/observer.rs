//! Bounded display-only adapter; the existing owner socket remains the transport.
use canvas::control::{self, Command, ControlRequest, IndicatorState, Outcome, StatusIndicator};
use canvas_workspace::{
    endpoint,
    observer::{Frame, Observation},
};
use fpl_gfx::{reactor::Activity, voice_popup};
use std::{
    io::{self, BufRead, Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

use canvas_workspace::observer::VOICE_INDICATOR_ID as INDICATOR_ID;

#[derive(Default)]
struct Projection {
    observation: Observation,
    caption: Option<String>,
    active: bool,
    idle_until: Option<Instant>,
}
impl Projection {
    fn observe(&mut self, frame: Option<Frame>, now: Instant) -> Result<(), String> {
        let same_live_session = self.observation.current(now).is_some_and(|previous| {
            frame.as_ref().is_some_and(|next| {
                previous.source == next.source && previous.session == next.session
            })
        });
        self.observation.observe(frame.clone(), now)?;
        if !same_live_session {
            self.caption = None;
            self.active = false;
            self.idle_until = None;
        }
        let Some(frame) = frame else {
            self.active = false;
            self.idle_until = None;
            return Ok(());
        };
        let active = frame.activity != Activity::Hidden;
        if active
            && !self.active
            && matches!(frame.activity, Activity::Listening | Activity::Resting)
        {
            self.caption = None;
        }
        if let Some(caption) = frame.caption.filter(|s| !s.is_empty()) {
            self.caption = Some(caption);
        }
        if self.active && !active {
            self.idle_until = Some(now + Duration::from_secs_f32(voice_popup::LINGER_SECONDS));
        } else if active {
            self.idle_until = None;
        }
        self.active = active;
        Ok(())
    }

    fn current(&self, now: Instant) -> Option<StatusIndicator> {
        let frame = self.observation.current(now)?;
        if frame.activity == Activity::Hidden && self.idle_until.is_none_or(|until| now >= until) {
            return None;
        }
        let state = match frame.activity {
            Activity::Hidden | Activity::Resting => IndicatorState::Ready,
            Activity::Listening => IndicatorState::Listening,
            Activity::Thinking => IndicatorState::Thinking,
            Activity::Speaking => IndicatorState::Speaking,
        };
        Some(StatusIndicator {
            id: INDICATOR_ID.into(),
            title: "Jarvis".into(),
            state,
            caption: self.caption.clone(),
            detail: None,
            diagnostic: None,
        })
    }
}

fn parse(line: &str) -> Result<Option<Frame>, String> {
    let request: endpoint::Request = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if request.schema_version != 1 || request.request_id.is_empty() || request.request_id.len() > 64
    {
        return Err("Invalid observer request".into());
    }
    match request.command {
        endpoint::Command::ObserverFrame { frame } => Ok(frame),
        _ => Err("Observer stream accepts display observations only".into()),
    }
}

fn publish(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    let reply = control::send(&ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: "voice-observation".into(),
        expected_revision: None,
        actor: None,
        command,
    })?;
    serde_json::to_writer(io::stdout().lock(), &reply)?;
    println!();
    io::stdout().flush()?;
    if !matches!(reply.outcome, Outcome::Ok { .. }) {
        return Err("Canvas rejected voice presentation".into());
    }
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // A replaced adapter must clear the previous process's transient overlay
    // before accepting fresh state, including a first idle/disconnected frame.
    publish(Command::DismissIndicator {
        indicator_id: INDICATOR_ID.into(),
    })?;
    let (send, receive) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let mut line = String::new();
            let event = match Read::by_ref(&mut input).take(4097).read_line(&mut line) {
                Ok(0) => break,
                Ok(n) if n <= 4096 && line.ends_with('\n') => parse(&line),
                Ok(_) => Err("Invalid observer stream frame".into()),
                Err(error) => Err(error.to_string()),
            };
            let invalid = event.is_err();
            if send.send(event).is_err() || invalid {
                break;
            }
        }
    });
    let mut projection = Projection::default();
    let mut displayed = None;
    let mut last_publication = Instant::now();
    let mut failure = None;
    loop {
        let mut finished = false;
        match receive.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(frame)) => {
                if let Err(error) = projection.observe(frame, Instant::now()) {
                    failure = Some(error);
                    finished = true;
                }
            }
            Ok(Err(error)) => {
                failure = Some(error);
                finished = true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => finished = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let next = if finished {
            None
        } else {
            projection.current(Instant::now())
        };
        if next != displayed
            || next.is_some() && last_publication.elapsed() >= Duration::from_secs(1)
        {
            let command = match &next {
                Some(indicator) => Command::UpsertIndicator {
                    indicator: indicator.clone(),
                },
                None => Command::DismissIndicator {
                    indicator_id: INDICATOR_ID.into(),
                },
            };
            publish(command)?;
            last_publication = Instant::now();
            displayed = next;
        }
        if finished {
            break;
        }
    }
    if let Some(error) = failure {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(revision: u64, activity: Activity, caption: Option<&str>) -> Frame {
        Frame {
            source: "neo".into(),
            session: "session".into(),
            revision,
            activity,
            ttl_ms: 5000,
            caption: caption.map(str::to_owned),
            playback_level: None,
        }
    }
    #[test]
    fn idle_lingers_once_and_fresh_turn_drops_previous_heard_text() {
        let now = Instant::now();
        let mut state = Projection::default();
        state
            .observe(
                Some(frame(1, Activity::Listening, Some("Jarvis, open Safari"))),
                now,
            )
            .unwrap();
        state
            .observe(Some(frame(2, Activity::Hidden, None)), now)
            .unwrap();
        assert_eq!(
            state.current(now).unwrap().caption.as_deref(),
            Some("Jarvis, open Safari")
        );
        state
            .observe(
                Some(frame(2, Activity::Hidden, None)),
                now + Duration::from_secs(1),
            )
            .unwrap();
        assert!(state.current(now + Duration::from_millis(1401)).is_none());
        state
            .observe(Some(frame(3, Activity::Listening, None)), now)
            .unwrap();
        assert!(state.current(now).unwrap().caption.is_none());
    }
    #[test]
    fn expiry_disconnect_and_invalid_frames_never_leave_live_presentation() {
        let now = Instant::now();
        let mut state = Projection::default();
        state
            .observe(Some(frame(1, Activity::Thinking, Some("opened"))), now)
            .unwrap();
        assert!(state.current(now + Duration::from_secs(5)).is_none());
        assert!(
            state
                .observe(Some(frame(0, Activity::Speaking, None)), now)
                .is_err()
        );
        assert_eq!(
            state.current(now).unwrap().caption.as_deref(),
            Some("opened")
        );
        state.observe(None, now).unwrap();
        assert!(state.current(now).is_none());
        state
            .observe(
                Some(frame(2, Activity::Listening, None)),
                now + Duration::from_secs(6),
            )
            .unwrap();
        assert!(
            state
                .current(now + Duration::from_secs(6))
                .unwrap()
                .caption
                .is_none()
        );
        assert!(parse(r#"{"schema_version":1,"request_id":"x","command":{"command":"workspace_switch","name":"other"}}"#).is_err());
    }
}
