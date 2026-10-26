//! The `soil:ble@1.0.0` seam: a transport backend, its typed errors, the
//! Soil-enforced limits, and a deterministic mock.
//!
//! Two pieces, deliberately separate:
//!
//! - [`BleBackend`] is the platform seam. A desktop backend is btleplug /
//!   BlueZ / CoreBluetooth; an Android backend is a JNI GATT callback. Each
//!   returns raw observations and typed [`BleError`]s.
//! - [`BleHost`] is the one place limits live. Every verb a Spore can reach goes
//!   through [`BleHost`], so a backend cannot forget to bound a read, allow a
//!   second link, or exceed the subscription cap. If a limit is only enforced in
//!   a backend, the next backend gets it wrong.
//!
//! The contract itself is `wit/soil-ble.wit`; the linker that exposes these
//! verbs — and only the granted ones — is `ble_linker.rs`. This module has no
//! wasmtime, no clock, and no I/O: it is testable as plain Rust, and
//! [`MockBle`] is deterministic (no sleeps, no wall clock, no randomness).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Mutex;

/// At most this many devices per scan (contract, §B.3).
pub const MAX_SCAN_DEVICES: usize = 64;
/// At most this many subscribed characteristics per link.
pub const MAX_SUBSCRIPTIONS: usize = 8;
/// A scan may ask for at most 30 s.
pub const MAX_SCAN_MS: u32 = 30_000;
/// One `next-event` call may wait at most 1 s.
pub const MAX_NEXT_EVENT_MS: u32 = 1_000;
/// Read and write payload bound, per call. A Spore chunks longer frames.
pub const MAX_FRAME_BYTES: usize = 512;

/// The nine typed errors of `soil:ble@1.0.0`.
///
/// Distinct variants rather than a string: a Spore has to be able to tell
/// "someone else owns the radio" from "you are over budget" without parsing
/// host prose, and the host must not echo backend text (which can contain a
/// device address) back into wasm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BleError {
    /// Capability or lease not granted to this Spore.
    Denied,
    /// Unknown or stale device handle / characteristic UUID.
    NotFound,
    /// No live link.
    NotConnected,
    /// Another authenticated Soil host holds a live radio lease.
    Leased,
    /// A bounded Soil call hit its deadline.
    Timeout,
    /// Frame / queue / scan / subscription limit exceeded.
    OverBudget,
    /// Adapter or link is still coming up.
    NotReady,
    /// Malformed argument (e.g. an empty UUID).
    Invalid,
    /// Backend failure; text is logged Soil-side, never echoed raw.
    Io,
}

impl BleError {
    /// Stable, machine-readable code, matching the WIT case name.
    pub fn code(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::NotFound => "not-found",
            Self::NotConnected => "not-connected",
            Self::Leased => "leased",
            Self::Timeout => "timeout",
            Self::OverBudget => "over-budget",
            Self::NotReady => "not-ready",
            Self::Invalid => "invalid",
            Self::Io => "io",
        }
    }
}

impl fmt::Display for BleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "soil:ble error: {}", self.code())
    }
}

impl std::error::Error for BleError {}

/// One scan result. `handle` is Soil-issued: a Spore never sees a MAC/UUID
/// address, so it cannot target a device the host did not offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub handle: u32,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    /// Stable cross-Soil-host identity, once the host has read DIS.
    pub ring_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Characteristic {
    pub uuid: String,
    pub readable: bool,
    pub writable: bool,
    pub notifiable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub uuid: String,
    pub characteristics: Vec<Characteristic>,
}

/// A polled link event. Notifications carry their characteristic UUID: the old
/// `ring:ble` channel dropped it, so a Spore could not tell the command service
/// from DFU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Notification { char_uuid: String, data: Vec<u8> },
    Disconnected { reason: String },
}

/// The platform seam. Eight verbs, polling only, no callbacks.
///
/// A backend is responsible for talking to the OS radio. It is *not*
/// responsible for limits, capability checks, or leases — [`BleHost`] owns
/// those, so they cannot diverge between backends.
pub trait BleBackend: Send + Sync {
    fn scan(&self, timeout_ms: u32) -> Result<Vec<Device>, BleError>;
    fn connect(&self, device: u32) -> Result<(), BleError>;
    fn disconnect(&self) -> Result<(), BleError>;
    fn services(&self) -> Result<Vec<Service>, BleError>;
    fn read(&self, char_uuid: &str, max_bytes: u32) -> Result<Vec<u8>, BleError>;
    fn write(&self, char_uuid: &str, data: &[u8]) -> Result<(), BleError>;
    fn subscribe(&self, char_uuid: &str) -> Result<(), BleError>;
    fn next_event(&self, timeout_ms: u32) -> Result<Option<Event>, BleError>;
}

/// A shared backend is itself a backend.
///
/// This is what lets a test hold a [`MockBle`] and hand the same instance to a
/// [`BleHost`] (and to the linker) without a second, parallel implementation
/// that could drift. It is also how a real host shares one radio between
/// several Spores while the limits stay in one place.
impl<T: BleBackend + ?Sized> BleBackend for std::sync::Arc<T> {
    fn scan(&self, timeout_ms: u32) -> Result<Vec<Device>, BleError> {
        (**self).scan(timeout_ms)
    }
    fn connect(&self, device: u32) -> Result<(), BleError> {
        (**self).connect(device)
    }
    fn disconnect(&self) -> Result<(), BleError> {
        (**self).disconnect()
    }
    fn services(&self) -> Result<Vec<Service>, BleError> {
        (**self).services()
    }
    fn read(&self, char_uuid: &str, max_bytes: u32) -> Result<Vec<u8>, BleError> {
        (**self).read(char_uuid, max_bytes)
    }
    fn write(&self, char_uuid: &str, data: &[u8]) -> Result<(), BleError> {
        (**self).write(char_uuid, data)
    }
    fn subscribe(&self, char_uuid: &str) -> Result<(), BleError> {
        (**self).subscribe(char_uuid)
    }
    fn next_event(&self, timeout_ms: u32) -> Result<Option<Event>, BleError> {
        (**self).next_event(timeout_ms)
    }
}

/// Live link state that the limits are computed from.
#[derive(Debug, Default)]
struct LinkState {
    connected: bool,
    device: Option<u32>,
    subscriptions: BTreeSet<String>,
}

/// The single enforcement point for the `soil:ble` limits.
///
/// Every bound in the WIT header comment is applied here, before the write
/// path (the only mutating verb) or a frame can reach a backend:
///
/// - scan ≤ 30 s and ≤ 64 devices
/// - `next-event` ≤ 1 s, and a notification ≤ 512 B
/// - read ≤ 512 B returned, write ≤ 512 B submitted
/// - ≤ 8 subscriptions per link
/// - at most one link: a second `connect` is `over-budget`, not a silent
///   replacement, and a second Soil host sees `leased` from its backend
///
/// A missing link is reported as [`BleError::NotConnected`] before the backend
/// is consulted, so a stale handle cannot reach a radio.
pub struct BleHost {
    backend: Box<dyn BleBackend>,
    state: Mutex<LinkState>,
}

impl BleHost {
    pub fn new(backend: impl BleBackend + 'static) -> Self {
        Self {
            backend: Box::new(backend),
            state: Mutex::new(LinkState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LinkState> {
        // A poisoned lock would mean a panic mid-call. The link state is a few
        // plain fields with no invariant that a panic can break, so recovering
        // it is honest; refusing every later call is not.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Whether a link is currently live.
    pub fn is_connected(&self) -> bool {
        self.state().connected
    }

    /// Number of live subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.state().subscriptions.len()
    }

    pub fn scan(&self, timeout_ms: u32) -> Result<Vec<Device>, BleError> {
        if timeout_ms > MAX_SCAN_MS {
            return Err(BleError::OverBudget);
        }
        let devices = self.backend.scan(timeout_ms)?;
        if devices.len() > MAX_SCAN_DEVICES {
            return Err(BleError::OverBudget);
        }
        Ok(devices)
    }

    pub fn connect(&self, device: u32) -> Result<(), BleError> {
        if self.state().connected {
            // One link per Soil host. Replacing the link silently would leak
            // the previous device's subscriptions and its lease.
            return Err(BleError::OverBudget);
        }
        self.backend.connect(device)?;
        let mut state = self.state();
        state.connected = true;
        state.device = Some(device);
        state.subscriptions.clear();
        Ok(())
    }

    pub fn disconnect(&self) -> Result<(), BleError> {
        if !self.state().connected {
            return Err(BleError::NotConnected);
        }
        let result = self.backend.disconnect();
        let mut state = self.state();
        // Release the link locally even if the backend failed: the Spore's
        // authoritative state is that no link may be assumed until it
        // reconnects, and a backend that cannot report success is not a link
        // the host can bound.
        state.connected = false;
        state.device = None;
        state.subscriptions.clear();
        result
    }

    pub fn services(&self) -> Result<Vec<Service>, BleError> {
        self.require_link()?;
        self.backend.services()
    }

    pub fn read(&self, char_uuid: &str, max_bytes: u32) -> Result<Vec<u8>, BleError> {
        self.require_link()?;
        if char_uuid.is_empty() || max_bytes == 0 {
            return Err(BleError::Invalid);
        }
        if max_bytes as usize > MAX_FRAME_BYTES {
            return Err(BleError::OverBudget);
        }
        let data = self.backend.read(char_uuid, max_bytes)?;
        if data.len() > MAX_FRAME_BYTES || data.len() > max_bytes as usize {
            // A backend that ignores the requested window is over budget; the
            // data is dropped rather than silently truncated, because a
            // truncated frame is a corrupt one.
            return Err(BleError::OverBudget);
        }
        Ok(data)
    }

    pub fn write(&self, char_uuid: &str, data: &[u8]) -> Result<(), BleError> {
        self.require_link()?;
        if char_uuid.is_empty() {
            return Err(BleError::Invalid);
        }
        if data.len() > MAX_FRAME_BYTES {
            return Err(BleError::OverBudget);
        }
        self.backend.write(char_uuid, data)
    }

    pub fn subscribe(&self, char_uuid: &str) -> Result<(), BleError> {
        self.require_link()?;
        if char_uuid.is_empty() {
            return Err(BleError::Invalid);
        }
        if self.state().subscriptions.contains(char_uuid) {
            // Already subscribed is the requested end state, and re-counting it
            // would let a Spore exhaust the cap by repeating one UUID.
            return Ok(());
        }
        if self.state().subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(BleError::OverBudget);
        }
        self.backend.subscribe(char_uuid)?;
        self.state().subscriptions.insert(char_uuid.to_owned());
        Ok(())
    }

    pub fn next_event(&self, timeout_ms: u32) -> Result<Option<Event>, BleError> {
        if timeout_ms > MAX_NEXT_EVENT_MS {
            return Err(BleError::OverBudget);
        }
        let event = self.backend.next_event(timeout_ms)?;
        if let Some(Event::Notification { data, .. }) = &event {
            if data.len() > MAX_FRAME_BYTES {
                return Err(BleError::OverBudget);
            }
        }
        Ok(event)
    }

    fn require_link(&self) -> Result<(), BleError> {
        if self.state().connected {
            Ok(())
        } else {
            Err(BleError::NotConnected)
        }
    }
}

/// How a [`MockBle`] is behaving at the moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MockMode {
    /// Normal: scripted results are returned.
    #[default]
    Normal,
    /// Every verb fails with [`BleError::Timeout`], the "Soil call hit its
    /// deadline" path.
    Timeout,
    /// Every verb fails with [`BleError::OverBudget`], the backend-side
    /// over-budget path (distinct from the host-side limit checks).
    OverBudget,
    /// Every verb fails with [`BleError::NotReady`], for a link still coming up.
    NotReady,
}

#[derive(Debug, Default)]
struct MockState {
    scan_devices: Vec<Device>,
    services: Vec<Service>,
    reads: BTreeMap<String, Vec<u8>>,
    writes: Vec<(String, Vec<u8>)>,
    notifications: VecDeque<Event>,
    subscribed: BTreeSet<String>,
    connected: Option<u32>,
    /// A *second* Soil host holds the radio, so this one must see `leased`.
    radio_held_elsewhere: bool,
    /// Scan deadlines and `next-event` deadlines the caller actually asked for.
    scan_deadlines: Vec<u32>,
    event_deadlines: Vec<u32>,
    mode: MockMode,
}

/// A deterministic [`BleBackend`] for the S1 proof table.
///
/// Programmable scan results, a scripted notification queue, a simulated
/// competing radio lease, and failure modes. No sleeps and no wall clock: a
/// timeout is a scripted answer, so the tests are reproducible on a loaded
/// machine (this host often is).
#[derive(Debug)]
pub struct MockBle {
    state: Mutex<MockState>,
}

impl Default for MockBle {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBle {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MockState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Replace the scan results a successful scan returns.
    pub fn script_scan(&self, devices: Vec<Device>) -> &Self {
        self.state().scan_devices = devices;
        self
    }

    /// Queue a notification, delivered by the next `next-event` call.
    pub fn push_notification(&self, char_uuid: &str, data: &[u8]) -> &Self {
        self.state()
            .notifications
            .push_back(Event::Notification {
                char_uuid: char_uuid.to_owned(),
                data: data.to_vec(),
            });
        self
    }

    /// Queue a disconnect event.
    pub fn push_disconnect(&self, reason: &str) -> &Self {
        self.state()
            .notifications
            .push_back(Event::Disconnected {
                reason: reason.to_owned(),
            });
        self
    }

    /// Serve these services from `services`.
    pub fn script_services(&self, services: Vec<Service>) -> &Self {
        self.state().services = services;
        self
    }

    /// Serve these bytes for `read(char_uuid, ..)`.
    pub fn script_read(&self, char_uuid: &str, data: &[u8]) -> &Self {
        self.state()
            .reads
            .insert(char_uuid.to_owned(), data.to_vec());
        self
    }

    /// Simulate another authenticated Soil host holding the radio lease.
    pub fn held_elsewhere(&self, held: bool) -> &Self {
        self.state().radio_held_elsewhere = held;
        self
    }

    /// Select a scripted failure mode for every verb.
    pub fn set_mode(&self, mode: MockMode) -> &Self {
        self.state().mode = mode;
        self
    }

    /// Everything written so far, in order: `(uuid, bytes)`.
    pub fn writes(&self) -> Vec<(String, Vec<u8>)> {
        self.state().writes.clone()
    }

    /// The scan deadlines a caller actually asked for.
    pub fn scan_deadlines(&self) -> Vec<u32> {
        self.state().scan_deadlines.clone()
    }

    /// The `next-event` deadlines a caller actually asked for.
    pub fn event_deadlines(&self) -> Vec<u32> {
        self.state().event_deadlines.clone()
    }

    /// Whether a Spore-visible link is live in the mock.
    pub fn connected(&self) -> Option<u32> {
        self.state().connected
    }

    fn mode(&self) -> Result<(), BleError> {
        match self.state().mode {
            MockMode::Normal => Ok(()),
            MockMode::Timeout => Err(BleError::Timeout),
            MockMode::OverBudget => Err(BleError::OverBudget),
            MockMode::NotReady => Err(BleError::NotReady),
        }
    }
}

impl BleBackend for MockBle {
    fn scan(&self, timeout_ms: u32) -> Result<Vec<Device>, BleError> {
        let mut state = self.state();
        state.scan_deadlines.push(timeout_ms);
        match state.mode {
            MockMode::Normal => Ok(state.scan_devices.clone()),
            MockMode::Timeout => Err(BleError::Timeout),
            MockMode::OverBudget => Err(BleError::OverBudget),
            MockMode::NotReady => Err(BleError::NotReady),
        }
    }

    fn connect(&self, device: u32) -> Result<(), BleError> {
        self.mode()?;
        let mut state = self.state();
        if state.radio_held_elsewhere {
            return Err(BleError::Leased);
        }
        if !state.scan_devices.iter().any(|entry| entry.handle == device) {
            return Err(BleError::NotFound);
        }
        state.connected = Some(device);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), BleError> {
        self.mode()?;
        let mut state = self.state();
        if state.connected.take().is_none() {
            return Err(BleError::NotConnected);
        }
        state.subscribed.clear();
        Ok(())
    }

    fn services(&self) -> Result<Vec<Service>, BleError> {
        self.mode()?;
        let state = self.state();
        if state.connected.is_none() {
            return Err(BleError::NotConnected);
        }
        Ok(state.services.clone())
    }

    fn read(&self, char_uuid: &str, _max_bytes: u32) -> Result<Vec<u8>, BleError> {
        self.mode()?;
        let state = self.state();
        if state.connected.is_none() {
            return Err(BleError::NotConnected);
        }
        state
            .reads
            .get(char_uuid)
            .cloned()
            .ok_or(BleError::NotFound)
    }

    fn write(&self, char_uuid: &str, data: &[u8]) -> Result<(), BleError> {
        self.mode()?;
        let mut state = self.state();
        if state.connected.is_none() {
            return Err(BleError::NotConnected);
        }
        state.writes.push((char_uuid.to_owned(), data.to_vec()));
        Ok(())
    }

    fn subscribe(&self, char_uuid: &str) -> Result<(), BleError> {
        self.mode()?;
        let mut state = self.state();
        if state.connected.is_none() {
            return Err(BleError::NotConnected);
        }
        state.subscribed.insert(char_uuid.to_owned());
        Ok(())
    }

    fn next_event(&self, timeout_ms: u32) -> Result<Option<Event>, BleError> {
        let mut state = self.state();
        state.event_deadlines.push(timeout_ms);
        match state.mode {
            MockMode::Normal => Ok(state.notifications.pop_front()),
            MockMode::Timeout => Err(BleError::Timeout),
            MockMode::OverBudget => Err(BleError::OverBudget),
            MockMode::NotReady => Err(BleError::NotReady),
        }
    }
}

/// A scan result used across the S1 tests.
pub fn mock_device(handle: u32, name: &str) -> Device {
    Device {
        handle,
        name: Some(name.to_owned()),
        rssi: Some(-42),
        ring_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn connected_host(mock: &Arc<MockBle>) -> BleHost {
        mock.script_scan(vec![mock_device(1, "ring")]);
        let host = BleHost::new(Arc::clone(mock));
        host.connect(1).expect("link");
        host
    }

    #[test]
    fn scan_is_bounded_by_time_and_device_count() {
        let mock = Arc::new(MockBle::new());
        let host = BleHost::new(Arc::clone(&mock));
        assert_eq!(host.scan(MAX_SCAN_MS + 1), Err(BleError::OverBudget));

        // A backend that ignores the 64-device cap is caught here, not by a
        // second backend that forgets the check.
        let devices: Vec<Device> = (0..=MAX_SCAN_DEVICES as u32)
            .map(|handle| mock_device(handle, "ring"))
            .collect();
        mock.script_scan(devices);
        assert_eq!(host.scan(MAX_SCAN_MS), Err(BleError::OverBudget));
    }

    /// The one-link rule: a second connect is refused rather than silently
    /// stealing the radio from the live link.
    #[test]
    fn a_second_connect_is_refused_rather_than_replacing_the_link() {
        let mock = Arc::new(MockBle::new());
        mock.script_scan(vec![mock_device(1, "ring"), mock_device(2, "other")]);
        let host = BleHost::new(Arc::clone(&mock));
        host.connect(1).expect("first link");
        assert_eq!(host.connect(2), Err(BleError::OverBudget));
        assert_eq!(mock.connected(), Some(1));
    }

    #[test]
    fn reads_and_writes_are_bounded_to_the_frame_limit() {
        let mock = Arc::new(MockBle::new());
        let host = connected_host(&mock);
        assert_eq!(host.read("cmd", 0), Err(BleError::Invalid));
        assert_eq!(host.read("", 16), Err(BleError::Invalid));
        assert_eq!(host.read("cmd", 513), Err(BleError::OverBudget));
        assert_eq!(
            host.write("cmd", &[0u8; MAX_FRAME_BYTES + 1]),
            Err(BleError::OverBudget)
        );
        assert_eq!(host.write("", &[1]), Err(BleError::Invalid));
    }

    /// A backend that returns a frame larger than requested is over budget, and
    /// nothing is recorded as written.
    #[test]
    fn a_backend_that_ignores_the_read_window_is_over_budget() {
        let mock = Arc::new(MockBle::new());
        let host = connected_host(&mock);
        mock.script_read("cmd", &[7u8; 32]);
        assert_eq!(host.read("cmd", 16), Err(BleError::OverBudget));
        assert_eq!(host.read("cmd", 32), Ok(vec![7u8; 32]));
    }

    #[test]
    fn subscriptions_stop_at_the_contract_cap() {
        let mock = Arc::new(MockBle::new());
        let host = connected_host(&mock);
        for index in 0..MAX_SUBSCRIPTIONS {
            host.subscribe(&format!("char-{index}")).expect("within cap");
        }
        assert_eq!(host.subscription_count(), MAX_SUBSCRIPTIONS);
        assert_eq!(host.subscribe("char-over"), Err(BleError::OverBudget));
        // Re-subscribing an existing characteristic is idempotent, not another
        // slot: otherwise one UUID could exhaust the cap.
        host.subscribe("char-0").expect("idempotent");
        assert_eq!(host.subscription_count(), MAX_SUBSCRIPTIONS);
    }

    #[test]
    fn next_event_deadline_is_bounded_and_none_is_not_an_error() {
        let mock = Arc::new(MockBle::new());
        let host = BleHost::new(Arc::clone(&mock));
        assert_eq!(
            host.next_event(MAX_NEXT_EVENT_MS + 1),
            Err(BleError::OverBudget)
        );
        assert_eq!(host.next_event(MAX_NEXT_EVENT_MS), Ok(None));
        assert_eq!(mock.event_deadlines(), vec![MAX_NEXT_EVENT_MS]);
    }

    /// An over-long notification frame is over budget rather than handed to a
    /// Spore that sized its buffer for 512 bytes.
    #[test]
    fn an_oversized_notification_is_over_budget() {
        let mock = Arc::new(MockBle::new());
        let host = BleHost::new(Arc::clone(&mock));
        mock.push_notification("cmd", &[0u8; MAX_FRAME_BYTES + 1]);
        assert_eq!(host.next_event(100), Err(BleError::OverBudget));
    }

    #[test]
    fn verbs_require_a_link_before_reaching_the_backend() {
        let mock = Arc::new(MockBle::new());
        let host = BleHost::new(Arc::clone(&mock));
        assert_eq!(host.services(), Err(BleError::NotConnected));
        assert_eq!(host.read("cmd", 16), Err(BleError::NotConnected));
        assert_eq!(host.write("cmd", &[1]), Err(BleError::NotConnected));
        assert_eq!(host.subscribe("cmd"), Err(BleError::NotConnected));
        assert_eq!(host.disconnect(), Err(BleError::NotConnected));
    }

    #[test]
    fn a_competing_soil_host_reports_leased() {
        let mock = Arc::new(MockBle::new());
        mock.script_scan(vec![mock_device(7, "ring")]);
        mock.held_elsewhere(true);
        let host = BleHost::new(Arc::clone(&mock));
        assert_eq!(host.connect(7), Err(BleError::Leased));
        assert!(!host.is_connected());
    }

    #[test]
    fn scripted_modes_surface_as_typed_errors() {
        let mock = Arc::new(MockBle::new());
        let host = connected_host(&mock);
        mock.set_mode(MockMode::Timeout);
        assert_eq!(host.scan(100), Err(BleError::Timeout));
        assert_eq!(host.next_event(10), Err(BleError::Timeout));
        mock.set_mode(MockMode::OverBudget);
        assert_eq!(host.write("cmd", &[1]), Err(BleError::OverBudget));
        mock.set_mode(MockMode::NotReady);
        assert_eq!(host.services(), Err(BleError::NotReady));
    }

    #[test]
    fn error_codes_match_the_wit_case_names() {
        let all = [
            BleError::Denied,
            BleError::NotFound,
            BleError::NotConnected,
            BleError::Leased,
            BleError::Timeout,
            BleError::OverBudget,
            BleError::NotReady,
            BleError::Invalid,
            BleError::Io,
        ];
        let codes: Vec<&str> = all.iter().map(|error| error.code()).collect();
        assert_eq!(
            codes,
            vec![
                "denied",
                "not-found",
                "not-connected",
                "leased",
                "timeout",
                "over-budget",
                "not-ready",
                "invalid",
                "io",
            ]
        );
    }
}
