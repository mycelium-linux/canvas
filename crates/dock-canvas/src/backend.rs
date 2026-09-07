//! Common lifecycle for Canvas visual backends.
//!
//! A backend owns reconstructible renderer state for one class of visual
//! surface. Workspace documents remain the durable source of presentation
//! intent; GPU handles, animation clocks, decoders, and frame caches stay in
//! backend implementations. This lifecycle is intentionally independent of
//! iced, wgpu, MapLibre, media decoders, and producer APIs.

use std::{error, fmt, time::Duration};

/// Logical and physical sizing supplied by the active Canvas renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMetrics {
    pub logical_width: f64,
    pub logical_height: f64,
    pub scale_factor: f64,
}

impl SurfaceMetrics {
    pub fn validate(self) -> Result<(), BackendError> {
        if self.logical_width.is_finite()
            && self.logical_width > 0.0
            && self.logical_height.is_finite()
            && self.logical_height > 0.0
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
        {
            Ok(())
        } else {
            Err(BackendError::Invalid(
                "surface metrics must be finite and positive".into(),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// The backend can consume direct pointer/keyboard/touch intents.
    pub interactive: bool,
    /// The backend can supply a GPU texture for the Canvas compositor.
    pub offscreen_texture: bool,
    /// The backend may request frames while an animation or stream is active.
    pub continuous_updates: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MountContext {
    pub surface_id: String,
    pub metrics: SurfaceMetrics,
    /// Runtime accessibility preference. This is not persisted in a document.
    pub reduced_motion: bool,
}

/// Timing provided by Canvas for one frame. Backends never read a wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRequest {
    pub sequence: u64,
    pub monotonic_time: Duration,
}

/// A backend can avoid repainting when its retained frame remains valid.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameOutcome<Frame> {
    Present(Frame),
    ReusePrevious,
    Idle,
    RedrawAfter(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendState {
    Mounted,
    Suspended,
    Unmounted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    Invalid(String),
    Conflict(String),
    Lifecycle(String),
    Adapter(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, message) = match self {
            Self::Invalid(message) => ("invalid backend input", message),
            Self::Conflict(message) => ("backend revision conflict", message),
            Self::Lifecycle(message) => ("invalid backend lifecycle", message),
            Self::Adapter(message) => ("visual backend failed", message),
        };
        write!(formatter, "{kind}: {message}")
    }
}

impl error::Error for BackendError {}

/// Typed renderer adapter used by images, documents, maps, video, and future
/// spatial surfaces. Associated model/frame types keep each domain strict while
/// [`BackendSlot`] provides one shared lifecycle and revision policy.
pub trait VisualBackend {
    type Model;
    type Frame;
    type Input;
    type Event;

    fn id(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;
    fn mount(&mut self, context: &MountContext, model: &Self::Model) -> Result<(), BackendError>;
    fn update(&mut self, revision: u64, model: &Self::Model) -> Result<(), BackendError>;
    fn resize(&mut self, metrics: SurfaceMetrics) -> Result<(), BackendError>;
    fn input(&mut self, input: Self::Input) -> Result<(), BackendError>;
    fn drain_events(&mut self) -> Result<Vec<Self::Event>, BackendError>;
    fn render(&mut self, request: FrameRequest) -> Result<FrameOutcome<Self::Frame>, BackendError>;
    fn suspend(&mut self) -> Result<(), BackendError>;
    fn resume(&mut self) -> Result<(), BackendError>;
    fn unmount(&mut self) -> Result<(), BackendError>;
}

/// Enforces the backend lifecycle so individual adapters cannot quietly invent
/// different revision, suspension, or resize semantics.
pub struct BackendSlot<B: VisualBackend> {
    backend: B,
    surface_id: String,
    model_revision: u64,
    metrics: SurfaceMetrics,
    state: BackendState,
}

impl<B: VisualBackend> BackendSlot<B> {
    pub fn mount(
        mut backend: B,
        context: MountContext,
        model_revision: u64,
        model: &B::Model,
    ) -> Result<Self, BackendError> {
        context.metrics.validate()?;
        if context.surface_id.is_empty() || model_revision == 0 {
            return Err(BackendError::Invalid(
                "surface id and model revision must be present".into(),
            ));
        }
        backend.mount(&context, model)?;
        Ok(Self {
            backend,
            surface_id: context.surface_id,
            model_revision,
            metrics: context.metrics,
            state: BackendState::Mounted,
        })
    }

    pub fn backend_id(&self) -> &'static str {
        self.backend.id()
    }

    pub fn capabilities(&self) -> BackendCapabilities {
        self.backend.capabilities()
    }

    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    pub fn model_revision(&self) -> u64 {
        self.model_revision
    }

    pub fn state(&self) -> BackendState {
        self.state
    }

    pub fn update(&mut self, revision: u64, model: &B::Model) -> Result<(), BackendError> {
        self.require_live()?;
        if revision != self.model_revision + 1 {
            return Err(BackendError::Conflict(format!(
                "expected revision {}, received {revision}",
                self.model_revision + 1
            )));
        }
        self.backend.update(revision, model)?;
        self.model_revision = revision;
        Ok(())
    }

    pub fn resize(&mut self, metrics: SurfaceMetrics) -> Result<(), BackendError> {
        self.require_live()?;
        metrics.validate()?;
        if metrics != self.metrics {
            self.backend.resize(metrics)?;
            self.metrics = metrics;
        }
        Ok(())
    }

    pub fn render(
        &mut self,
        request: FrameRequest,
    ) -> Result<FrameOutcome<B::Frame>, BackendError> {
        match self.state {
            BackendState::Mounted => self.backend.render(request),
            BackendState::Suspended => Ok(FrameOutcome::Idle),
            BackendState::Unmounted => Err(BackendError::Lifecycle(
                "cannot render an unmounted surface".into(),
            )),
        }
    }

    pub fn input(&mut self, input: B::Input) -> Result<(), BackendError> {
        self.require_mounted()?;
        if !self.backend.capabilities().interactive {
            return Err(BackendError::Lifecycle(format!(
                "backend '{}' does not accept interaction",
                self.backend.id()
            )));
        }
        self.backend.input(input)
    }

    pub fn drain_events(&mut self) -> Result<Vec<B::Event>, BackendError> {
        self.require_live()?;
        self.backend.drain_events()
    }

    pub fn suspend(&mut self) -> Result<(), BackendError> {
        match self.state {
            BackendState::Mounted => {
                self.backend.suspend()?;
                self.state = BackendState::Suspended;
                Ok(())
            }
            BackendState::Suspended => Ok(()),
            BackendState::Unmounted => Err(BackendError::Lifecycle(
                "cannot suspend an unmounted surface".into(),
            )),
        }
    }

    pub fn resume(&mut self) -> Result<(), BackendError> {
        match self.state {
            BackendState::Suspended => {
                self.backend.resume()?;
                self.state = BackendState::Mounted;
                Ok(())
            }
            BackendState::Mounted => Ok(()),
            BackendState::Unmounted => Err(BackendError::Lifecycle(
                "cannot resume an unmounted surface".into(),
            )),
        }
    }

    pub fn unmount(&mut self) -> Result<(), BackendError> {
        if self.state != BackendState::Unmounted {
            self.backend.unmount()?;
            self.state = BackendState::Unmounted;
        }
        Ok(())
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn require_live(&self) -> Result<(), BackendError> {
        if self.state == BackendState::Unmounted {
            Err(BackendError::Lifecycle(
                "surface backend is unmounted".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn require_mounted(&self) -> Result<(), BackendError> {
        if self.state == BackendState::Mounted {
            Ok(())
        } else {
            Err(BackendError::Lifecycle(
                "surface backend must be mounted to accept interaction".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeBackend {
        calls: Vec<&'static str>,
        events: Vec<u64>,
    }

    impl VisualBackend for FakeBackend {
        type Model = u64;
        type Frame = u64;
        type Input = u64;
        type Event = u64;

        fn id(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                interactive: true,
                offscreen_texture: true,
                continuous_updates: false,
            }
        }

        fn mount(
            &mut self,
            _context: &MountContext,
            _model: &Self::Model,
        ) -> Result<(), BackendError> {
            self.calls.push("mount");
            Ok(())
        }

        fn update(&mut self, _revision: u64, _model: &Self::Model) -> Result<(), BackendError> {
            self.calls.push("update");
            Ok(())
        }

        fn resize(&mut self, _metrics: SurfaceMetrics) -> Result<(), BackendError> {
            self.calls.push("resize");
            Ok(())
        }

        fn input(&mut self, input: Self::Input) -> Result<(), BackendError> {
            self.calls.push("input");
            self.events.push(input);
            Ok(())
        }

        fn drain_events(&mut self) -> Result<Vec<Self::Event>, BackendError> {
            self.calls.push("events");
            Ok(std::mem::take(&mut self.events))
        }

        fn render(
            &mut self,
            request: FrameRequest,
        ) -> Result<FrameOutcome<Self::Frame>, BackendError> {
            self.calls.push("render");
            Ok(FrameOutcome::Present(request.sequence))
        }

        fn suspend(&mut self) -> Result<(), BackendError> {
            self.calls.push("suspend");
            Ok(())
        }

        fn resume(&mut self) -> Result<(), BackendError> {
            self.calls.push("resume");
            Ok(())
        }

        fn unmount(&mut self) -> Result<(), BackendError> {
            self.calls.push("unmount");
            Ok(())
        }
    }

    fn metrics(width: f64) -> SurfaceMetrics {
        SurfaceMetrics {
            logical_width: width,
            logical_height: 800.0,
            scale_factor: 2.0,
        }
    }

    #[test]
    fn slot_enforces_revision_and_lifecycle_once_for_every_backend() {
        let context = MountContext {
            surface_id: "map".into(),
            metrics: metrics(1200.0),
            reduced_motion: false,
        };
        let mut slot = BackendSlot::mount(FakeBackend::default(), context, 3, &7).unwrap();
        assert_eq!(slot.backend_id(), "fake");
        assert_eq!(slot.model_revision(), 3);
        assert!(matches!(slot.update(5, &8), Err(BackendError::Conflict(_))));
        slot.update(4, &8).unwrap();
        slot.resize(metrics(1400.0)).unwrap();
        slot.input(42).unwrap();
        assert_eq!(slot.drain_events().unwrap(), [42]);
        slot.suspend().unwrap();
        assert!(slot.input(43).is_err());
        assert_eq!(
            slot.render(FrameRequest {
                sequence: 1,
                monotonic_time: Duration::ZERO,
            })
            .unwrap(),
            FrameOutcome::Idle
        );
        slot.resume().unwrap();
        assert_eq!(
            slot.render(FrameRequest {
                sequence: 9,
                monotonic_time: Duration::from_millis(16),
            })
            .unwrap(),
            FrameOutcome::Present(9)
        );
        slot.unmount().unwrap();
        slot.unmount().unwrap();
        assert!(
            slot.render(FrameRequest {
                sequence: 10,
                monotonic_time: Duration::from_millis(32),
            })
            .is_err()
        );

        let backend = slot.into_backend();
        assert_eq!(
            backend.calls,
            [
                "mount", "update", "resize", "input", "events", "suspend", "resume", "render",
                "unmount"
            ]
        );
    }

    #[test]
    fn invalid_metrics_fail_before_an_adapter_is_mounted() {
        let context = MountContext {
            surface_id: "map".into(),
            metrics: metrics(f64::NAN),
            reduced_motion: false,
        };
        let error = BackendSlot::mount(FakeBackend::default(), context, 1, &1)
            .err()
            .expect("invalid metrics must fail");
        assert!(matches!(error, BackendError::Invalid(_)));
    }
}
