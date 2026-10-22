//! Brokered device ownership. No direct-open fallback or permission widening.
use smithay::{
    backend::session::{Event, Session, libseat::LibSeatSession},
    reexports::{calloop::EventLoop, rustix::fs::OFlags},
};
use std::{
    os::fd::OwnedFd,
    path::Path,
    time::{Duration, Instant},
};

pub struct SeatAccess {
    pub session: LibSeatSession,
    events: EventLoop<'static, Vec<Event>>,
}
impl SeatAccess {
    pub fn poll_fd(&self) -> impl std::os::fd::AsFd + '_ {
        &self.events
    }
    pub fn open() -> Result<Self, String> {
        let (session, notifier) = LibSeatSession::new().map_err(|error| {
            format!("seat broker: {error}; start Canvas in a local seat-bound login session")
        })?;
        let events: EventLoop<'static, Vec<Event>> =
            EventLoop::try_new().map_err(|error| error.to_string())?;
        events
            .handle()
            .insert_source(notifier, |event, _, events| events.push(event))
            .map_err(|error| format!("seat notifier: {error}"))?;
        let mut access = Self { session, events };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !access.session.is_active() {
            access
                .events
                .dispatch(Duration::from_millis(10), &mut Vec::new())
                .map_err(|error| error.to_string())?;
            if Instant::now() >= deadline {
                return Err("seat remains inactive; refusing to open devices".into());
            }
        }
        Ok(access)
    }
    pub fn dispatch(&mut self) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        self.events
            .dispatch(Duration::ZERO, &mut events)
            .map_err(|error| format!("seat dispatch: {error}"))?;
        Ok(events)
    }
}

/// Keep the broker's original descriptor until all DRM wrappers have dropped.
/// The renderer receives a duplicate, never ownership of the broker descriptor.
pub struct SeatDevice {
    session: LibSeatSession,
    fd: Option<OwnedFd>,
}
impl SeatDevice {
    pub fn open(session: &mut LibSeatSession, path: &Path) -> Result<Self, String> {
        let fd = session
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK)
            .map_err(|error| format!("seat device {}: {error}", path.display()))?;
        Ok(Self {
            session: session.clone(),
            fd: Some(fd),
        })
    }
    pub fn duplicate(&self) -> Result<OwnedFd, String> {
        self.fd
            .as_ref()
            .expect("live broker device")
            .try_clone()
            .map_err(|error| error.to_string())
    }
}
impl Drop for SeatDevice {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            if let Err(error) = self.session.close(fd) {
                eprintln!("seat device release: {error}");
            }
        }
    }
}
