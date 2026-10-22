//! One settled view of the display topology, shared by every desktop adapter.
//!
//! Canvas used to enumerate displays once at startup. Two things followed from
//! that: enabling or disabling a monitor changed nothing, and a Canvas that
//! started before its display backend answered rendered on no display at all
//! until someone restarted it. This module owns the detection half of the fix.
//! A platform observer reports raw reconfiguration edges, a debounce coalesces
//! the burst macOS emits per change into one settled event, and the event
//! carries the freshly enumerated set so a consumer never has to re-enumerate
//! and race the next change.
//!
//! macOS registers a CoreGraphics reconfiguration callback, which needs no
//! Accessibility grant and so lives here rather than in a permission-bearing
//! helper. Other compositors satisfy [`ReconfigurationSignal`] the same way; a
//! desktop with no observer yet still gets [`DisplayWatch`], degraded to the
//! recovery probe below, because Canvas starting is worth more than Canvas
//! reacting.

use std::{
    cell::Cell,
    fmt,
    rc::Rc,
    time::{Duration, Instant},
};

/// How long a burst must stay quiet before it counts as one settled change.
///
/// macOS emits a callback per display per phase, so a single monitor being
/// enabled is several edges milliseconds apart; re-enumerating on each would
/// thrash the backend and hand the layout a half-applied topology.
pub const SETTLE: Duration = Duration::from_millis(400);

/// Ceiling on how long a burst may defer the event it is coalescing into.
///
/// A display waking up can emit edges for as long as it takes to train the
/// link. Without a ceiling the settle timer keeps restarting and the layout
/// never reacts at all, which is the bug this module exists to fix.
pub const MAX_DEFER: Duration = Duration::from_secs(2);

/// How often to re-enumerate while no display is known.
///
/// Nothing reconfigures when a display backend starts *after* Canvas, so no
/// observer can report it. An empty inventory is therefore treated as a fault
/// to recover from rather than a topology to trust.
pub const BLIND_RETRY: Duration = Duration::from_secs(1);

/// Monotonic time, injected so debounce behaviour never depends on wall time.
pub trait Clock {
    /// Time since the watch started. Never goes backwards.
    fn elapsed(&self) -> Duration;
}

/// The real clock: monotonic, and unaffected by the system clock being set.
#[derive(Debug)]
pub struct MonotonicClock(Instant);

impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock tests drive by hand. Clones share one time, so a test can advance
/// the clock a watch already owns.
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Rc<Cell<Duration>>);

impl ManualClock {
    pub fn advance(&self, step: Duration) {
        self.0.set(self.0.get().saturating_add(step));
    }
}

impl Clock for ManualClock {
    fn elapsed(&self) -> Duration {
        self.0.get()
    }
}

/// Raw, unfiltered edges from one compositor's reconfiguration notification.
///
/// Implementations count and nothing else: coalescing and enumeration belong to
/// [`DisplayWatch`], so every desktop implements the same one method and none
/// of them re-derives when a change has settled.
pub trait ReconfigurationSignal {
    /// Edges observed since the previous call. Reading clears the count.
    fn take_edges(&self) -> u32;
}

/// Whether changes will be noticed, and why not when they will not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchStatus {
    /// The platform observer is registered; changes arrive as events.
    Observing,
    /// No observer. An empty topology still recovers on its own, but a change
    /// to a populated one will go unnoticed until Canvas restarts.
    Unobserved(String),
}

impl fmt::Display for WatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observing => f.write_str("Watching displays for reconfiguration."),
            Self::Unobserved(reason) => write!(
                f,
                "Display changes will not be noticed automatically: {reason}"
            ),
        }
    }
}

/// The settle timer. Pure, so its edge cases are provable without a clock.
#[derive(Debug, Default)]
struct Debounce {
    first: Option<Duration>,
    latest: Option<Duration>,
}

impl Debounce {
    fn signal(&mut self, now: Duration) {
        self.first.get_or_insert(now);
        self.latest = Some(now);
    }

    fn pending(&self) -> bool {
        self.first.is_some()
    }

    /// Settled once the burst has been quiet for [`SETTLE`], or has been
    /// deferred for [`MAX_DEFER`], whichever comes first.
    fn settled(&self, now: Duration) -> bool {
        match (self.first, self.latest) {
            (Some(first), Some(latest)) => {
                now.saturating_sub(latest) >= SETTLE || now.saturating_sub(first) >= MAX_DEFER
            }
            _ => false,
        }
    }

    fn clear(&mut self) {
        self.first = None;
        self.latest = None;
    }
}

/// The display topology Canvas is currently drawing on, and the settled changes
/// to it.
///
/// `D` is the caller's own display record: this owns *when* to re-enumerate,
/// not what a display is.
pub struct DisplayWatch<D, C = MonotonicClock> {
    enumerate: Box<dyn Fn() -> Result<Vec<D>, String>>,
    signal: Option<Box<dyn ReconfigurationSignal>>,
    status: WatchStatus,
    clock: C,
    debounce: Debounce,
    known: Vec<D>,
    last_probe: Duration,
    last_error: Option<String>,
    edges: u64,
}

impl<D: Clone + PartialEq> DisplayWatch<D, MonotonicClock> {
    /// Registers the platform observer and takes the first inventory.
    ///
    /// Never fails. A refused registration becomes a [`WatchStatus`] the caller
    /// can show, and a refused enumeration leaves the inventory empty, which is
    /// exactly the state the recovery probe exists to climb out of.
    pub fn start(enumerate: impl Fn() -> Result<Vec<D>, String> + 'static) -> Self {
        Self::with_parts(MonotonicClock::default(), platform::observe(), enumerate)
    }
}

impl<D: Clone + PartialEq, C: Clock> DisplayWatch<D, C> {
    /// The seam tests use: any clock, any signal, including one that refused to
    /// register.
    pub fn with_parts(
        clock: C,
        signal: Result<Box<dyn ReconfigurationSignal>, String>,
        enumerate: impl Fn() -> Result<Vec<D>, String> + 'static,
    ) -> Self {
        let enumerate: Box<dyn Fn() -> Result<Vec<D>, String>> = Box::new(enumerate);
        let (known, last_error) = match enumerate() {
            Ok(displays) => (displays, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let (signal, status) = match signal {
            Ok(signal) => (Some(signal), WatchStatus::Observing),
            Err(reason) => (None, WatchStatus::Unobserved(reason)),
        };
        let started = clock.elapsed();
        Self {
            enumerate,
            signal,
            status,
            clock,
            debounce: Debounce::default(),
            known,
            last_probe: started,
            last_error,
            edges: 0,
        }
    }

    /// The topology as of the last settled event, or of startup.
    #[must_use]
    pub fn displays(&self) -> &[D] {
        &self.known
    }

    #[must_use]
    pub fn status(&self) -> &WatchStatus {
        &self.status
    }

    /// The most recent enumeration failure, cleared by the next success.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Raw edges coalesced so far. Diagnostics only: a consumer reacts to
    /// [`poll`](Self::poll), never to this.
    #[must_use]
    pub fn edges(&self) -> u64 {
        self.edges
    }

    /// Advances the watch. Call it from the frame loop; it is a pair of integer
    /// compares and one atomic read until something actually changes.
    ///
    /// Returns the settled display set the moment a burst goes quiet, or the
    /// enumeration failure that replaced one. `None` is the common case: no
    /// change, a change that enumerated back to the same topology, or a repeat
    /// of a failure already reported.
    pub fn poll(&mut self) -> Option<Result<Vec<D>, String>> {
        let now = self.clock.elapsed();
        let edges = self.signal.as_ref().map_or(0, |signal| signal.take_edges());
        if edges > 0 {
            self.edges = self.edges.saturating_add(u64::from(edges));
            self.debounce.signal(now);
        }
        if !self.due(now) {
            return None;
        }
        self.debounce.clear();
        self.last_probe = now;
        match (self.enumerate)() {
            Ok(displays) => {
                self.last_error = None;
                if displays == self.known {
                    return None;
                }
                self.known = displays.clone();
                Some(Ok(displays))
            }
            Err(error) => {
                // Report a failure once. A backend that stays down would
                // otherwise repaint the same status every second and bury
                // whatever the operator was reading.
                if self.last_error.as_deref() == Some(error.as_str()) {
                    return None;
                }
                self.last_error = Some(error.clone());
                Some(Err(error))
            }
        }
    }

    fn due(&self, now: Duration) -> bool {
        if self.debounce.settled(now) {
            return true;
        }
        !self.debounce.pending()
            && self.known.is_empty()
            && now.saturating_sub(self.last_probe) >= BLIND_RETRY
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ReconfigurationSignal;
    use std::{
        ffi::c_void,
        marker::PhantomData,
        ptr,
        rc::Rc,
        sync::atomic::{AtomicU32, Ordering},
    };

    /// CoreGraphics announces every change twice: once before it happens, with
    /// only this flag set, and once after with what actually changed. Ignoring
    /// the first starts the settle timer when the topology lands rather than
    /// when it is merely about to move.
    const BEGIN_CONFIGURATION: u32 = 1 << 0;
    const SUCCESS: i32 = 0;

    type Callback = unsafe extern "C" fn(u32, u32, *mut c_void);

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayRegisterReconfigurationCallback(
            callback: Callback,
            user_info: *mut c_void,
        ) -> i32;
        fn CGDisplayRemoveReconfigurationCallback(
            callback: Callback,
            user_info: *mut c_void,
        ) -> i32;
    }

    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    fn is_change(flags: u32) -> bool {
        flags & !BEGIN_CONFIGURATION != 0
    }

    unsafe extern "C" fn on_reconfiguration(_display: u32, flags: u32, user_info: *mut c_void) {
        if user_info.is_null() || !is_change(flags) {
            return;
        }
        // SAFETY: CoreGraphics passes back the counter registered below, which
        // outlives the registration: Drop removes this callback before freeing
        // it, and leaks it instead when removal fails.
        unsafe { &*user_info.cast::<AtomicU32>() }.fetch_add(1, Ordering::Release);
    }

    struct MacSignal {
        edges: Option<Box<AtomicU32>>,
        _main_thread: PhantomData<Rc<()>>,
    }

    impl MacSignal {
        /// The counter's stable heap address, which is also the token that
        /// identifies this registration to CoreGraphics.
        fn user_info(&self) -> *mut c_void {
            match self.edges.as_deref() {
                Some(edges) => ptr::from_ref(edges).cast_mut().cast(),
                None => ptr::null_mut(),
            }
        }
    }

    impl ReconfigurationSignal for MacSignal {
        fn take_edges(&self) -> u32 {
            self.edges
                .as_ref()
                .map_or(0, |edges| edges.swap(0, Ordering::AcqRel))
        }
    }

    impl Drop for MacSignal {
        fn drop(&mut self) {
            let user_info = self.user_info();
            // SAFETY: this !Send owner is dropped on the registering thread,
            // whose run loop delivers the callback being removed.
            let removed =
                unsafe { CGDisplayRemoveReconfigurationCallback(on_reconfiguration, user_info) };
            if removed != SUCCESS {
                eprintln!(
                    "display reconfiguration callback cleanup failed (CGError {removed}); \
                     leaking its counter so a late callback cannot write freed memory"
                );
                if let Some(edges) = self.edges.take() {
                    let _ = Box::leak(edges);
                }
            }
        }
    }

    pub fn observe() -> Result<Box<dyn ReconfigurationSignal>, String> {
        // SAFETY: pthread_main_np has no preconditions.
        if unsafe { pthread_main_np() } == 0 {
            // CoreGraphics delivers on the run loop of the registering thread,
            // so registering anywhere else would silently never fire.
            return Err("display reconfiguration must be observed from the GUI main thread".into());
        }
        let signal = MacSignal {
            edges: Some(Box::new(AtomicU32::new(0))),
            _main_thread: PhantomData,
        };
        // SAFETY: the counter's heap address stays valid until Drop, which
        // removes this registration before releasing it.
        let result = unsafe {
            CGDisplayRegisterReconfigurationCallback(on_reconfiguration, signal.user_info())
        };
        if result != SUCCESS {
            return Err(format!(
                "could not observe display reconfiguration (CGError {result})"
            ));
        }
        Ok(Box::new(signal))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_a_landed_change_starts_the_settle_timer() {
            assert!(!is_change(BEGIN_CONFIGURATION));
            assert!(!is_change(0));
            // Added, removed, enabled, disabled, and desktop shape changed.
            for flags in [1 << 4, 1 << 5, 1 << 8, 1 << 9, 1 << 12] {
                assert!(is_change(flags));
                assert!(is_change(flags | BEGIN_CONFIGURATION));
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::ReconfigurationSignal;

    pub fn observe() -> Result<Box<dyn ReconfigurationSignal>, String> {
        Err("no display reconfiguration observer is installed for this desktop".into())
    }
}

pub use platform::observe;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A signal a test drives directly, standing in for CoreGraphics.
    #[derive(Clone, Default)]
    struct FakeSignal(Rc<Cell<u32>>);

    impl FakeSignal {
        fn emit(&self, edges: u32) {
            self.0.set(self.0.get() + edges);
        }
    }

    impl ReconfigurationSignal for FakeSignal {
        fn take_edges(&self) -> u32 {
            self.0.replace(0)
        }
    }

    /// The backend's answer, plus how many times it was asked.
    #[derive(Clone)]
    struct FakeBackend {
        answer: Rc<RefCell<Result<Vec<String>, String>>>,
        calls: Rc<Cell<usize>>,
    }

    impl FakeBackend {
        fn new(displays: &[&str]) -> Self {
            Self {
                answer: Rc::new(RefCell::new(Ok(ids(displays)))),
                calls: Rc::new(Cell::new(0)),
            }
        }

        fn set(&self, displays: &[&str]) {
            *self.answer.borrow_mut() = Ok(ids(displays));
        }

        fn fail(&self, error: &str) {
            *self.answer.borrow_mut() = Err(error.into());
        }

        fn enumerator(&self) -> Box<dyn Fn() -> Result<Vec<String>, String>> {
            let backend = self.clone();
            Box::new(move || {
                backend.calls.set(backend.calls.get() + 1);
                backend.answer.borrow().clone()
            })
        }
    }

    fn ids(displays: &[&str]) -> Vec<String> {
        displays.iter().map(|id| (*id).to_string()).collect()
    }

    fn watch(
        clock: &ManualClock,
        signal: &FakeSignal,
        backend: &FakeBackend,
    ) -> DisplayWatch<String, ManualClock> {
        DisplayWatch::with_parts(
            clock.clone(),
            Ok(Box::new(signal.clone())),
            backend.enumerator(),
        )
    }

    /// One reconfiguration, seen and then allowed to go quiet. Every caller
    /// asserts the edge alone settles nothing, which is the whole point.
    fn burst(
        watch: &mut DisplayWatch<String, ManualClock>,
        clock: &ManualClock,
        signal: &FakeSignal,
    ) -> Option<Result<Vec<String>, String>> {
        signal.emit(1);
        assert!(watch.poll().is_none(), "an edge must not settle instantly");
        clock.advance(SETTLE);
        watch.poll()
    }

    #[test]
    fn a_burst_of_edges_coalesces_into_one_settled_event() {
        let clock = ManualClock::default();
        let signal = FakeSignal::default();
        let backend = FakeBackend::new(&["built-in"]);
        let mut watch = watch(&clock, &signal, &backend);
        let enumerations = backend.calls.get();

        backend.set(&["built-in", "studio"]);
        for _ in 0..6 {
            signal.emit(3);
            assert!(watch.poll().is_none(), "a burst must not emit while noisy");
            clock.advance(Duration::from_millis(50));
        }

        clock.advance(SETTLE);
        assert_eq!(watch.poll(), Some(Ok(ids(&["built-in", "studio"]))));
        assert_eq!(watch.displays(), ids(&["built-in", "studio"]));
        assert_eq!(
            backend.calls.get() - enumerations,
            1,
            "one settled change must cost one enumeration"
        );

        clock.advance(SETTLE * 4);
        assert!(watch.poll().is_none(), "a settled burst must not repeat");
    }

    #[test]
    fn every_edge_restarts_the_quiet_period() {
        let clock = ManualClock::default();
        let signal = FakeSignal::default();
        let backend = FakeBackend::new(&["built-in"]);
        let mut watch = watch(&clock, &signal, &backend);
        backend.set(&["studio"]);

        // Edges spaced just inside the quiet period keep deferring the event,
        // up to the ceiling that stops them deferring it forever.
        let step = SETTLE - Duration::from_millis(1);
        let mut waited = Duration::ZERO;
        while waited + step < MAX_DEFER {
            signal.emit(1);
            assert!(watch.poll().is_none());
            clock.advance(step);
            waited += step;
            assert!(watch.poll().is_none(), "quiet period restarts on each edge");
        }

        clock.advance(SETTLE);
        assert_eq!(watch.poll(), Some(Ok(ids(&["studio"]))));
    }

    #[test]
    fn an_unending_storm_still_lands_within_the_deferral_ceiling() {
        let clock = ManualClock::default();
        let signal = FakeSignal::default();
        let backend = FakeBackend::new(&["built-in"]);
        let mut watch = watch(&clock, &signal, &backend);
        backend.set(&["built-in", "studio"]);

        let mut settled = None;
        let mut waited = Duration::ZERO;
        while settled.is_none() && waited <= MAX_DEFER * 2 {
            signal.emit(1);
            settled = watch.poll();
            clock.advance(Duration::from_millis(100));
            waited += Duration::from_millis(100);
        }
        assert_eq!(settled, Some(Ok(ids(&["built-in", "studio"]))));
        assert!(
            waited <= MAX_DEFER + Duration::from_millis(100),
            "a storm must not defer past the ceiling, waited {waited:?}"
        );
    }

    #[test]
    fn a_change_that_settles_back_to_the_same_topology_says_nothing() {
        let clock = ManualClock::default();
        let signal = FakeSignal::default();
        let backend = FakeBackend::new(&["built-in", "studio"]);
        let mut watch = watch(&clock, &signal, &backend);
        let enumerations = backend.calls.get();

        assert!(burst(&mut watch, &clock, &signal).is_none());
        assert_eq!(
            backend.calls.get() - enumerations,
            1,
            "the burst did settle and was re-enumerated"
        );
        assert_eq!(watch.displays(), ids(&["built-in", "studio"]));
        assert_eq!(watch.edges(), 1, "the edge was still observed");
    }

    #[test]
    fn an_empty_inventory_is_reprobed_until_a_backend_answers() {
        let clock = ManualClock::default();
        let backend = FakeBackend::new(&[]);
        backend.fail("Dock Control is not running");
        // No observer at all: nothing reconfigures when a backend starts late,
        // so recovery cannot depend on one.
        let mut watch: DisplayWatch<String, ManualClock> = DisplayWatch::with_parts(
            clock.clone(),
            Err("observer unavailable".into()),
            backend.enumerator(),
        );
        assert!(watch.displays().is_empty());
        assert_eq!(watch.last_error(), Some("Dock Control is not running"));

        clock.advance(BLIND_RETRY);
        assert!(
            watch.poll().is_none(),
            "the same failure must not repaint every probe"
        );

        backend.set(&["built-in", "studio", "wall"]);
        clock.advance(BLIND_RETRY);
        assert_eq!(
            watch.poll(),
            Some(Ok(ids(&["built-in", "studio", "wall"]))),
            "a backend that starts late must be picked up without a restart"
        );
        assert_eq!(watch.last_error(), None);

        let settled = backend.calls.get();
        clock.advance(BLIND_RETRY * 10);
        assert!(watch.poll().is_none());
        assert_eq!(
            backend.calls.get(),
            settled,
            "a known topology is observed, not polled"
        );
    }

    #[test]
    fn a_refused_registration_is_not_fatal() {
        let clock = ManualClock::default();
        let backend = FakeBackend::new(&["built-in"]);
        let mut watch: DisplayWatch<String, ManualClock> = DisplayWatch::with_parts(
            clock.clone(),
            Err("not the GUI main thread".into()),
            backend.enumerator(),
        );

        assert_eq!(
            watch.status(),
            &WatchStatus::Unobserved("not the GUI main thread".into())
        );
        assert!(
            watch
                .status()
                .to_string()
                .contains("not the GUI main thread")
        );
        assert_eq!(
            watch.displays(),
            ids(&["built-in"]),
            "startup enumeration still stands"
        );

        // Behaves as it did before this module existed: a populated topology
        // that no one is watching simply never changes.
        backend.set(&["built-in", "studio"]);
        clock.advance(MAX_DEFER * 10);
        assert!(watch.poll().is_none());
    }

    #[test]
    fn an_enumeration_failure_is_reported_once_and_recovers() {
        let clock = ManualClock::default();
        let signal = FakeSignal::default();
        let backend = FakeBackend::new(&["built-in"]);
        let mut watch = watch(&clock, &signal, &backend);

        backend.fail("Dock Control did not return display geometry");
        assert_eq!(
            burst(&mut watch, &clock, &signal),
            Some(Err("Dock Control did not return display geometry".into()))
        );

        assert!(
            burst(&mut watch, &clock, &signal).is_none(),
            "the same failure reports once"
        );
        assert_eq!(
            watch.displays(),
            ids(&["built-in"]),
            "a failed probe must not blank the known topology"
        );

        backend.set(&["studio"]);
        assert_eq!(
            burst(&mut watch, &clock, &signal),
            Some(Ok(ids(&["studio"])))
        );
        assert_eq!(watch.last_error(), None);
    }

    #[test]
    fn the_settle_timer_has_no_opinion_without_an_edge() {
        let mut debounce = Debounce::default();
        assert!(!debounce.pending());
        assert!(!debounce.settled(Duration::from_secs(60)));

        debounce.signal(Duration::from_millis(10));
        assert!(debounce.pending());
        assert!(!debounce.settled(Duration::from_millis(10) + SETTLE / 2));
        assert!(debounce.settled(Duration::from_millis(10) + SETTLE));

        debounce.clear();
        assert!(!debounce.pending());
        assert!(!debounce.settled(Duration::from_secs(60)));
    }
}
