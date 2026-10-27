//! The gated `soil:ble@1.0.0` linker.
//!
//! The capability package deliberately has no world: a Spore owns its world.
//! This file therefore binds a consumer-defined test world that imports
//! `soil:ble/ble`, then registers host functions against that interface.
//!
//! Two checks stand between a Spore and the radio:
//!
//! 1. [`add_to_linker`] registers only the verbs whose capability is in
//!    [`EffectiveCaps`]. An ungranted import is absent, so instantiation fails
//!    at the normal "unknown import" boundary.
//! 2. Every registered host function re-checks its own capability anyway and
//!    returns the typed `denied` error instead of trapping. That matters when
//!    authority changes after a linker is built, and it is what
//!    [`add_all_to_linker`] exercises in tests.
//!
//! Deadlines are passed through to [`crate::ble::BleHost`], which owns the
//! contract bounds. A backend that reports an expired bounded call returns
//! [`crate::ble::BleError::Timeout`], never a trap.

use crate::ble::{BleBackend, BleError, BleHost, Event};
use crate::caps::EffectiveCaps;
use std::collections::BTreeSet;
use std::sync::Arc;
use wasmtime::component::{Linker, LinkerInstance};
use wasmtime::StoreContextMut;

#[allow(clippy::all)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/soil-ble.wit",
        world: "canvas:soil-ble-test/soil-ble-test",
        inline: "
            package canvas:soil-ble-test@0.1.0;

            world soil-ble-test {
                import soil:ble/ble@1.0.0;
            }
        ",
    });
}

use bindings::soil::ble::ble::Error;
use bindings::soil::ble::types::{
    Characteristic, Device, DisconnectReason, Event as WitEvent, NotificationPayload, Service,
};

/// Store data for a Soil host running a BLE Spore.
pub struct BleCtx {
    pub host: Arc<BleHost>,
    /// A snapshot of the caps used to build the linker. Host functions consult
    /// it per call, so a revoked cap is `denied` even on an existing instance.
    pub caps: BTreeSet<String>,
}

impl BleCtx {
    fn granted(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }
}

/// Build a context and register only the verbs granted by `caps`.
pub fn add_to_linker(
    linker: &mut Linker<BleCtx>,
    caps: &EffectiveCaps,
    backend: impl BleBackend + 'static,
) -> anyhow::Result<BleCtx> {
    register(linker, caps, backend, false)
}

/// Register every verb, leaving each host function's own capability check to
/// deny ungranted calls with [`Error::Denied`]. Production uses
/// [`add_to_linker`]; this exists so the second gate is provable against a real
/// component and is not decoration.
pub fn add_all_to_linker(
    linker: &mut Linker<BleCtx>,
    caps: &EffectiveCaps,
    backend: impl BleBackend + 'static,
) -> anyhow::Result<BleCtx> {
    register(linker, caps, backend, true)
}

fn register(
    linker: &mut Linker<BleCtx>,
    caps: &EffectiveCaps,
    backend: impl BleBackend + 'static,
    register_all: bool,
) -> anyhow::Result<BleCtx> {
    let host = Arc::new(BleHost::new(backend));
    let caps = caps.caps.clone();
    let context = BleCtx {
        host: Arc::clone(&host),
        caps: caps.clone(),
    };
    let observe = caps.contains("soil:ble.observe");
    let connect = caps.contains("soil:ble.connect");
    let write = caps.contains("soil:ble.write");
    let mut ble = linker.instance("soil:ble/ble@1.0.0")?;

    if observe || register_all {
        register_scan(&mut ble, &host)?;
        register_services(&mut ble, &host)?;
        register_read(&mut ble, &host)?;
        register_subscribe(&mut ble, &host)?;
        register_next_event(&mut ble, &host)?;
    }
    if connect || register_all {
        register_connect(&mut ble, &host)?;
        register_disconnect(&mut ble, &host)?;
    }
    if write || register_all {
        register_write(&mut ble, &host)?;
    }

    Ok(context)
}

fn map(error: BleError) -> Error {
    match error {
        BleError::Denied => Error::Denied,
        BleError::NotFound => Error::NotFound,
        BleError::NotConnected => Error::NotConnected,
        BleError::Leased => Error::Leased,
        BleError::Timeout => Error::Timeout,
        BleError::OverBudget => Error::OverBudget,
        BleError::NotReady => Error::NotReady,
        BleError::Invalid => Error::Invalid,
        BleError::Io => Error::Io,
    }
}

fn to_device(device: crate::ble::Device) -> Device {
    Device {
        handle: device.handle,
        name: device.name,
        rssi: device.rssi,
        ring_id: device.ring_id,
    }
}

fn to_services(services: Vec<crate::ble::Service>) -> Vec<Service> {
    services
        .into_iter()
        .map(|service| Service {
            uuid: service.uuid,
            characteristics: service
                .characteristics
                .into_iter()
                .map(|characteristic| Characteristic {
                    uuid: characteristic.uuid,
                    readable: characteristic.readable,
                    writable: characteristic.writable,
                    notifiable: characteristic.notifiable,
                })
                .collect(),
        })
        .collect()
}

fn to_event(event: Event) -> WitEvent {
    match event {
        Event::Notification { char_uuid, data } => WitEvent::Notification(NotificationPayload {
            char_uuid,
            data,
        }),
        Event::Disconnected { reason } => {
            WitEvent::Disconnected(DisconnectReason { reason })
        }
    }
}

fn check(context: &BleCtx, cap: &str) -> Result<(), Error> {
    if context.granted(cap) {
        Ok(())
    } else {
        Err(Error::Denied)
    }
}

/// The authority decision a host function makes, shared with the non-component
/// gating tests so there is no second copy of the rule.
pub fn authorize(caps: &BTreeSet<String>, cap: &str) -> Result<(), BleError> {
    if caps.contains(cap) {
        Ok(())
    } else {
        Err(BleError::Denied)
    }
}

const OBSERVE_VERBS: &[&str] = &["scan", "services", "read", "subscribe", "next-event"];
const CONNECT_VERBS: &[&str] = &["connect", "disconnect"];
const WRITE_VERBS: &[&str] = &["write"];

/// The verbs a linker built from `caps` should carry. `register_all` mirrors
/// [`add_all_to_linker`]; production leaves it false.
pub fn registered_verbs(caps: &BTreeSet<String>, register_all: bool) -> Vec<&'static str> {
    let mut verbs = Vec::new();
    if register_all || caps.contains("soil:ble.observe") {
        verbs.extend_from_slice(OBSERVE_VERBS);
    }
    if register_all || caps.contains("soil:ble.connect") {
        verbs.extend_from_slice(CONNECT_VERBS);
    }
    if register_all || caps.contains("soil:ble.write") {
        verbs.extend_from_slice(WRITE_VERBS);
    }
    verbs
}

fn register_scan(ble: &mut LinkerInstance<'_, BleCtx>, host: &Arc<BleHost>) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "scan",
        move |store: StoreContextMut<'_, BleCtx>, (timeout_ms,): (u32,)| {
            if let Err(error) = check(&store.data(), "soil:ble.observe") {
                return Ok((Err(error),));
            }
            Ok((host
                .scan(timeout_ms)
                .map(|devices| devices.into_iter().map(to_device).collect::<Vec<_>>())
                .map_err(map),))
        },
    )
}

fn register_connect(
    ble: &mut LinkerInstance<'_, BleCtx>,
    host: &Arc<BleHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "connect",
        move |store: StoreContextMut<'_, BleCtx>, (device,): (u32,)| {
            if let Err(error) = check(&store.data(), "soil:ble.connect") {
                return Ok((Err(error),));
            }
            Ok((host.connect(device).map_err(map),))
        },
    )
}

fn register_disconnect(
    ble: &mut LinkerInstance<'_, BleCtx>,
    host: &Arc<BleHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "disconnect",
        move |store: StoreContextMut<'_, BleCtx>, (): ()| {
            if let Err(error) = check(&store.data(), "soil:ble.connect") {
                return Ok((Err(error),));
            }
            Ok((host.disconnect().map_err(map),))
        },
    )
}

fn register_services(
    ble: &mut LinkerInstance<'_, BleCtx>,
    host: &Arc<BleHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "services",
        move |store: StoreContextMut<'_, BleCtx>, (): ()| {
            if let Err(error) = check(&store.data(), "soil:ble.observe") {
                return Ok((Err(error),));
            }
            Ok((host.services().map(to_services).map_err(map),))
        },
    )
}

fn register_read(ble: &mut LinkerInstance<'_, BleCtx>, host: &Arc<BleHost>) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "read",
        move |store: StoreContextMut<'_, BleCtx>, (char_uuid, max_bytes): (String, u32)| {
            if let Err(error) = check(&store.data(), "soil:ble.observe") {
                return Ok((Err(error),));
            }
            Ok((host.read(&char_uuid, max_bytes).map_err(map),))
        },
    )
}

fn register_write(ble: &mut LinkerInstance<'_, BleCtx>, host: &Arc<BleHost>) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "write",
        move |store: StoreContextMut<'_, BleCtx>, (char_uuid, data): (String, Vec<u8>)| {
            if let Err(error) = check(&store.data(), "soil:ble.write") {
                return Ok((Err(error),));
            }
            Ok((host.write(&char_uuid, &data).map_err(map),))
        },
    )
}

fn register_subscribe(
    ble: &mut LinkerInstance<'_, BleCtx>,
    host: &Arc<BleHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "subscribe",
        move |store: StoreContextMut<'_, BleCtx>, (char_uuid,): (String,)| {
            if let Err(error) = check(&store.data(), "soil:ble.observe") {
                return Ok((Err(error),));
            }
            Ok((host.subscribe(&char_uuid).map_err(map),))
        },
    )
}

fn register_next_event(
    ble: &mut LinkerInstance<'_, BleCtx>,
    host: &Arc<BleHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    ble.func_wrap(
        "next-event",
        move |store: StoreContextMut<'_, BleCtx>, (timeout_ms,): (u32,)| {
            if let Err(error) = check(&store.data(), "soil:ble.observe") {
                return Ok((Err(error),));
            }
            Ok((host
                .next_event(timeout_ms)
                .map(|event| event.map(to_event))
                .map_err(map),))
        },
    )
}
