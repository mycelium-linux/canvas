//! S1 proof table: every `soil:ble` verb through a real component, plus the
//! capability, lease, deadline and budget gates.
//!
//! The fixture `.wasm` is generated from WIT by the guest crate in
//! `tests/fixtures/ble/guest`; see `examples/build-wasm-fixtures.rs`.

#![cfg(feature = "soil-ble")]

use canvas_model_host::ble::{
    BleHost, Characteristic, MAX_NEXT_EVENT_MS, MAX_SCAN_MS, MockBle, MockMode, Service, mock_device,
};
use canvas_model_host::ble_linker::{
    BleCtx, add_all_to_linker, add_to_linker, authorize, registered_verbs,
};
use canvas_model_host::caps::{EffectiveCaps, Limits};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use wasmtime::component::{Component, Instance, Linker, Val};
use wasmtime::{Config, Engine, Store};

// Guest-side codes. The fixture returns 0/count on success and these codes for
// typed errors, so the tests exercise the real component ABI without a second
// copy of the record/list lowering.
const DENIED: u32 = 100;
const LEASED: u32 = 103;
const TIMEOUT: u32 = 104;
const OVER_BUDGET: u32 = 105;

fn fixture() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ble/soil-ble.wasm"))
        .expect("read soil-ble fixture")
}

fn engine() -> Engine {
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("engine")
}

fn caps(names: &[&str]) -> EffectiveCaps {
    EffectiveCaps {
        caps: names.iter().map(|name| (*name).to_string()).collect(),
        limits: Limits {
            memory_mb: 64,
            fuel: 1_000_000,
            epoch_ms: 2_000,
        },
        clamped: Vec::new(),
    }
}

fn full_caps() -> EffectiveCaps {
    caps(&["soil:ble.observe", "soil:ble.connect", "soil:ble.write"])
}

fn scripted() -> Arc<MockBle> {
    let mock = Arc::new(MockBle::new());
    mock.script_scan(vec![mock_device(7, "ring")]);
    mock.script_services(vec![Service {
        uuid: "svc".into(),
        characteristics: vec![Characteristic {
            uuid: "cmd".into(),
            readable: true,
            writable: true,
            notifiable: true,
        }],
    }]);
    mock.script_read("cmd", &[1, 2, 3]);
    mock
}

fn instantiate(
    engine: &Engine,
    caps: &EffectiveCaps,
    mock: Arc<MockBle>,
    all: bool,
) -> (Store<BleCtx>, Instance) {
    let component = Component::new(engine, fixture()).expect("compile fixture component");
    let mut linker = Linker::new(engine);
    let context = if all {
        add_all_to_linker(&mut linker, caps, mock).expect("register all verbs")
    } else {
        add_to_linker(&mut linker, caps, mock).expect("register granted verbs")
    };
    let mut store = Store::new(engine, context);
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate fixture component");
    (store, instance)
}

fn call_u64(mut store: &mut Store<BleCtx>, instance: &Instance, name: &str, params: &[Val]) -> u64 {
    let func = instance
        .get_func(&mut store, name)
        .unwrap_or_else(|| panic!("component exports {name}"));
    let mut results = vec![Val::Bool(false)];
    func.call(store, params, &mut results)
        .unwrap_or_else(|error| panic!("call {name}: {error:#}"));
    match results[0] {
        Val::U64(value) => value,
        ref other => panic!("{name} returned {other:?}, expected u64"),
    }
}

fn call(mut store: &mut Store<BleCtx>, instance: &Instance, name: &str, params: &[Val]) -> u32 {
    let func = instance
        .get_func(&mut store, name)
        .unwrap_or_else(|| panic!("component exports {name}"));
    let mut results = vec![Val::Bool(false)];
    func.call(store, params, &mut results)
        .unwrap_or_else(|error| panic!("call {name}: {error:#}"));
    match results[0] {
        Val::U32(value) => value,
        ref other => panic!("{name} returned {other:?}, expected u32"),
    }
}

#[test]
fn soil_ble_all_eight_verbs_run_through_a_real_component() {
    let engine = engine();
    let mock = scripted();
    mock.push_notification("cmd", &[9, 8]);
    let (mut store, instance) = instantiate(&engine, &full_caps(), Arc::clone(&mock), false);

    let packed = call_u64(&mut store, &instance, "run-all", &[Val::U32(100)]);
    let fields: Vec<u64> = (0..8).map(|shift| (packed >> (shift * 8)) & 0xff).collect();
    assert_eq!(
        fields,
        vec![1, 0, 1, 3, 0, 0, 1, 0],
        "scan=1 device, connect=ok, services=1, read=3 bytes, write=ok, subscribe=ok, next-event=notification, disconnect=ok"
    );
    assert_eq!(mock.writes(), vec![("cmd".to_string(), vec![4, 5])]);
    assert_eq!(mock.connected(), None, "run-all disconnects at the end");
}

#[test]
fn soil_ble_scan_is_delivered_with_only_the_observe_capability() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(&engine, &caps(&["soil:ble.observe"]), mock, true);
    assert_eq!(
        call(&mut store, &instance, "run-scan", &[Val::U32(100)]),
        1
    );
}

#[test]
fn soil_ble_write_is_denied_without_the_write_capability() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(
        &engine,
        &caps(&["soil:ble.observe", "soil:ble.connect"]),
        Arc::clone(&mock),
        true,
    );
    let result = call(
        &mut store,
        &instance,
        "run-write",
        &[Val::String("cmd".into()), Val::List(vec![Val::U8(1)])],
    );
    assert_eq!(result, DENIED);
    assert!(
        mock.writes().is_empty(),
        "denied write must not reach the radio"
    );
}

#[test]
fn soil_ble_connect_is_denied_without_the_connect_capability() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(
        &engine,
        &caps(&["soil:ble.observe"]),
        Arc::clone(&mock),
        true,
    );
    assert_eq!(
        call(&mut store, &instance, "run-connect", &[Val::U32(7)]),
        DENIED
    );
    assert_eq!(mock.connected(), None);
}

#[test]
fn soil_ble_an_ungranted_verb_is_absent_from_the_production_linker() {
    let engine = engine();
    let component = Component::new(&engine, fixture()).expect("compile fixture component");
    let mut linker = Linker::new(&engine);
    let context = add_to_linker(&mut linker, &caps(&["soil:ble.observe"]), scripted())
        .expect("register observe only");
    let mut store = Store::new(&engine, context);
    let error = match linker.instantiate(&mut store, &component) {
        Ok(_) => panic!("write/connect imports must be absent"),
        Err(error) => error,
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("import") || message.contains("unknown"),
        "failure must name the missing import: {message}"
    );
}

#[test]
fn soil_ble_a_competing_soil_host_reports_leased() {
    let engine = engine();
    let mock = scripted();
    mock.held_elsewhere(true);
    let (mut store, instance) = instantiate(&engine, &full_caps(), mock, false);
    assert_eq!(
        call(&mut store, &instance, "run-connect", &[Val::U32(7)]),
        LEASED
    );
}

#[test]
fn soil_ble_deadline_and_budget_failures_are_typed() {
    let engine = engine();
    let over_mock = scripted();
    let (mut over_store, over_instance) =
        instantiate(&engine, &full_caps(), over_mock, false);
    assert_eq!(
        call(
            &mut over_store,
            &over_instance,
            "run-scan",
            &[Val::U32(MAX_SCAN_MS + 1)]
        ),
        OVER_BUDGET
    );

    let timeout_mock = scripted();
    timeout_mock.set_mode(MockMode::Timeout);
    let (mut timeout_store, timeout_instance) =
        instantiate(&engine, &full_caps(), timeout_mock, false);
    assert_eq!(
        call(
            &mut timeout_store,
            &timeout_instance,
            "run-scan",
            &[Val::U32(100)]
        ),
        TIMEOUT
    );
}

#[test]
fn soil_ble_next_event_returns_none_on_deadline_not_an_error() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(&engine, &full_caps(), Arc::clone(&mock), false);
    assert_eq!(
        call(
            &mut store,
            &instance,
            "run-next-event",
            &[Val::U32(MAX_NEXT_EVENT_MS)]
        ),
        0,
        "no event by the deadline is none"
    );
    assert_eq!(mock.event_deadlines(), vec![MAX_NEXT_EVENT_MS]);
}

#[test]
fn soil_ble_linker_plan_and_denial_without_a_component() {
    let observe_only: BTreeSet<String> = ["soil:ble.observe".to_string()].into_iter().collect();
    assert_eq!(
        registered_verbs(&observe_only, false),
        vec!["scan", "services", "read", "subscribe", "next-event"]
    );
    assert_eq!(
        authorize(&observe_only, "soil:ble.write"),
        Err(canvas_model_host::ble::BleError::Denied)
    );
    assert_eq!(
        authorize(&observe_only, "soil:ble.connect"),
        Err(canvas_model_host::ble::BleError::Denied)
    );
    assert_eq!(authorize(&observe_only, "soil:ble.observe"), Ok(()));

    let nudged: BTreeSet<String> = observe_only
        .iter()
        .cloned()
        .chain(["soil:ble.write".to_string()])
        .collect();
    let mut verbs = registered_verbs(&nudged, false);
    verbs.sort_unstable();
    assert!(verbs.contains(&"write"));
    assert_eq!(authorize(&nudged, "soil:ble.write"), Ok(()));
}

#[test]
fn soil_ble_contract_bounds_are_shared_with_the_host() {
    assert_eq!(MAX_SCAN_MS, 30_000);
    assert_eq!(MAX_NEXT_EVENT_MS, 1_000);
    assert!(BleHost::new(MockBle::new()).scan(MAX_SCAN_MS + 1).is_err());
}
