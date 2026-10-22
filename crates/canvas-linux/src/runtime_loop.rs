//! Readiness coordination only. Device/protocol adapters retain their ownership.
use smithay::reexports::calloop::{
    EventLoop, Interest, Mode, PostAction,
    generic::Generic,
    ping::{Ping, make_ping},
};
use std::{os::fd::AsFd, time::Instant};

pub struct RuntimeLoop {
    events: EventLoop<'static, ()>,
    wake: Ping,
}
/// A worker exit (including unwind) must wake an otherwise idle compositor.
pub struct WakeOnDrop(pub Ping);
impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.ping();
    }
}
impl RuntimeLoop {
    pub fn new() -> Result<Self, String> {
        let events = EventLoop::try_new().map_err(|e| format!("runtime loop: {e}"))?;
        let (wake, source) = make_ping().map_err(|e| e.to_string())?;
        events
            .handle()
            .insert_source(source, |_, _, _| {})
            .map_err(|e| e.to_string())?;
        Ok(Self { events, wake })
    }
    pub fn wake(&self) -> Ping {
        self.wake.clone()
    }
    /// Duplicate only the poll descriptor; never consume data in this callback.
    pub fn watch(&self, fd: impl AsFd) -> Result<(), String> {
        let fd = fd.as_fd().try_clone_to_owned().map_err(|e| e.to_string())?;
        self.events
            .handle()
            .insert_source(Generic::new(fd, Interest::READ, Mode::Level), |_, _, _| {
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn wait(&mut self, deadline: Option<Instant>) -> Result<(), String> {
        let timeout = deadline.map(|time| time.saturating_duration_since(Instant::now()));
        self.events
            .dispatch(timeout, &mut ())
            .map_err(|e| format!("runtime dispatch: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn worker_unwind_signals_the_idle_runtime() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let mut runtime = RuntimeLoop::new().unwrap();
        let (wake, source) = make_ping().unwrap();
        let seen = Arc::new(AtomicBool::new(false));
        let observer = seen.clone();
        runtime
            .events
            .handle()
            .insert_source(source, move |_, _, _| {
                observer.store(true, Ordering::SeqCst);
            })
            .unwrap();
        let worker = std::thread::spawn(move || {
            std::panic::catch_unwind(|| {
                let _exit = WakeOnDrop(wake);
                panic!("owned worker fixture");
            })
            .unwrap_err();
        });
        runtime
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .unwrap();
        worker.join().unwrap();
        assert!(
            seen.load(Ordering::SeqCst),
            "worker unwind did not wake the runtime"
        );
    }
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };
    #[test]
    fn readiness_leaves_bytes_for_owner_and_ping_wakes_waiter() {
        let mut runtime = RuntimeLoop::new().unwrap();
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        runtime.watch(&reader).unwrap();
        writer.write_all(b"x").unwrap();
        runtime
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut byte = [0];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b"x");
        let wake = runtime.wake();
        let worker = std::thread::spawn(move || wake.ping());
        runtime
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .unwrap();
        worker.join().unwrap();
        runtime.wait(Some(Instant::now())).unwrap();
    }
}
