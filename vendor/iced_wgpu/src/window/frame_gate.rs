//! One unfinished frame per native window. No waiting on the event-loop thread.
//! Redraw attempts while busy retain no scene/frame: completion asks the shell
//! to draw current application state. Offscreen rendering is deliberately separate.
use crate::graphics::Shell;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

#[derive(Default)]
struct State {
    busy: bool,
    deferred: bool,
    failed: bool,
    submitted: u64,
    completed: u64,
    coalesced: u64,
}

impl State {
    fn admit(&mut self) -> bool {
        assert!(!self.failed, "Canvas GPU completion worker failed");
        if self.busy {
            self.deferred = true;
            self.coalesced += 1;
            false
        } else {
            self.busy = true;
            self.submitted += 1;
            true
        }
    }

    fn complete(&mut self) -> bool {
        self.busy = false;
        std::mem::take(&mut self.deferred)
    }
}

pub(super) struct FrameGate {
    state: Arc<Mutex<State>>,
    sender: mpsc::SyncSender<(wgpu::SubmissionIndex, Instant)>,
}

impl FrameGate {
    pub fn new(device: wgpu::Device, shell: Shell) -> Option<Self> {
        // 0 exists only for controlled A/B measurements; reject ambiguous config.
        match std::env::var("CANVAS_GPU_MAX_IN_FLIGHT").as_deref() {
            Ok("0") => return None,
            Ok("1") | Err(std::env::VarError::NotPresent) => {}
            _ => panic!("CANVAS_GPU_MAX_IN_FLIGHT must be 0 (A/B) or 1"),
        }
        let state = Arc::new(Mutex::new(State::default()));
        let worker_state = Arc::clone(&state);
        let (sender, receiver) = mpsc::sync_channel::<(wgpu::SubmissionIndex, Instant)>(1);
        let metrics = std::env::var_os("CANVAS_GPU_FRAME_METRICS").is_some();
        let _worker = std::thread::Builder::new()
            .name("canvas-gpu-completion".into())
            .spawn(move || {
                while let Ok((submission_index, started)) = receiver.recv() {
                    let result = device.poll(wgpu::PollType::Wait {
                        submission_index: Some(submission_index),
                        timeout: Some(std::time::Duration::from_secs(30)),
                    });
                    let mut state = worker_state.lock().expect("GPU frame gate poisoned");
                    if let Err(error) = result {
                        eprintln!("canvas: GPU completion failed: {error}");
                        state.failed = true;
                        let _ = state.complete();
                        drop(state);
                        shell.request_redraw();
                        break;
                    }
                    state.completed += 1;
                    if metrics {
                        eprintln!("canvas: gpu_frame completed={} admitted={} coalesced={} completion_ms={:.3} max_in_flight=1",
                            state.completed, state.submitted, state.coalesced, started.elapsed().as_secs_f64() * 1000.0);
                    }
                    let redraw = state.complete();
                    drop(state);
                    if redraw {
                        shell.request_redraw();
                    }
                }
            })
            .expect("start Canvas GPU completion worker");
        eprintln!("canvas: GPU frame gate max_in_flight=1 per_window=true");
        Some(Self { state, sender })
    }

    pub fn admit(&self) -> bool {
        self.state.lock().expect("GPU frame gate poisoned").admit()
    }

    pub fn submitted(&self, index: wgpu::SubmissionIndex) {
        self.sender
            .try_send((index, Instant::now()))
            .expect("GPU completion queue must contain at most one frame");
    }

    pub fn cancel(&self) {
        let _ = self
            .state
            .lock()
            .expect("GPU frame gate poisoned")
            .complete();
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    #[ignore = "requires a working GPU backend"]
    fn gpu_completion_wakes_deferred_redraw_without_another_submission() {
        struct Notifier(std::sync::mpsc::Sender<()>);
        impl crate::graphics::shell::Notifier for Notifier {
            fn request_redraw(&self) {
                self.0.send(()).expect("redraw observer");
            }
            fn invalidate_layout(&self) {}
        }
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("GPU adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("GPU device");
        let (sender, receiver) = std::sync::mpsc::channel();
        let gate = super::FrameGate::new(
            device.clone(),
            crate::graphics::Shell::new(Notifier(sender)),
        )
        .expect("test requires enabled gate");
        // A second window has an independent permit on the same GPU device.
        // Its progress must not depend on the first window becoming idle.
        let second = super::FrameGate::new(device.clone(), crate::graphics::Shell::headless())
            .expect("second window gate");
        assert!(gate.admit());
        assert!(second.admit());
        gate.cancel();
        second.cancel();
        for _ in 0..10 {
            assert!(gate.admit());
            // Worker cannot complete this reservation until submitted below.
            assert!(!gate.admit());
            let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            gate.submitted(queue.submit([encoder.finish()]));
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("completion must wake shell even without another submit/poll");
            assert!(!gate.state.lock().expect("gate state").busy);
        }
        assert_eq!(gate.state.lock().expect("gate state").completed, 10);
    }

    #[test]
    fn only_one_unfinished_frame_and_one_coalesced_redraw() {
        let mut state = State::default();
        assert!(state.admit());
        for _ in 0..1000 {
            assert!(!state.admit());
        }
        assert_eq!(state.submitted, 1);
        assert_eq!(state.coalesced, 1000);
        assert!(state.complete());
        assert!(!state.complete());
        assert!(state.admit());
        assert!(!state.complete());
    }

    #[test]
    fn surface_failure_releases_reservation() {
        let mut state = State::default();
        assert!(state.admit());
        assert!(!state.complete());
        assert!(state.admit());
    }

    #[test]
    #[should_panic(expected = "GPU completion worker failed")]
    fn worker_failure_is_not_silently_ignored() {
        let mut state = State {
            failed: true,
            ..State::default()
        };
        let _ = state.admit();
    }
}
