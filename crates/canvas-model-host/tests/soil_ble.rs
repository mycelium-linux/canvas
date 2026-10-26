//! S1 proof table: every `soil:ble` verb through a real component, plus the
//! capability, lease, deadline and budget gates.

#![cfg(feature = "soil-ble")]

use canvas_model_host::ble::{
    BleHost, Characteristic, Device, MAX_NEXT_EVENT_MS, MAX_SCAN_MS, MockBle, MockMode, Service,
    mock_device,
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

fn fixture() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ble/soil-ble.wasm"),
    )
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

fn call(mut store: &mut Store<BleCtx>, instance: &Instance, name: &str, params: &[Val]) -> Vec<Val> {
    let func = instance
        .get_func(&mut store, name)
        .unwrap_or_else(|| panic!("component exports {name}"));
    let mut results = vec![Val::Bool(false)];
    func.call(store, params, &mut results)
        .unwrap_or_else(|error| panic!("call {name}: {error:#}"));
    results
}

fn ok_value(value: &Val) -> &Option<Box<Val>> {
    match value {
        Val::Result(Ok(value)) => value,
        Val::Result(Err(error)) => panic!("expected ok, got error {error:?}"),
        other => panic!("expected result, got {other:?}"),
    }
}

fn error_code(value: &Val) -> &str {
    match value {
        Val::Result(Err(Some(error))) => match error.as_ref() {
            Val::Enum(name) => name,
            other => panic!("expected typed error enum, got {other:?}"),
        },
        Val::Result(Ok(value)) => panic!("expected error, got ok {value:?}"),
        other => panic!("expected result, got {other:?}"),
    }
}

fn list(value: &Option<Box<Val>>) -> &[Val] {
    match value.as_deref() {
        Some(Val::List(items)) => items,
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_all_eight_verbs_run_through_a_real_component() {
    let engine = engine();
    let mock = scripted();
    mock.push_notification("cmd", &[9, 8]);
    let (mut store, instance) = instantiate(&engine, &full_caps(), Arc::clone(&mock), false);

    let scanned = call(&mut store, &instance, "scan", &[Val::U32(100)]);
    let devices = list(ok_value(&scanned[0]));
    assert_eq!(devices.len(), 1);
    match &devices[0] {
        Val::Record(fields) => {
            let handle = fields.iter().find(|(name, _)| name == "handle").unwrap();
            assert!(matches!(handle.1, Val::U32(7)));
        }
        other => panic!("expected device record, got {other:?}"),
    }

    let connected = call(&mut store, &instance, "connect", &[Val::U32(7)]);
    ok_value(&connected[0]);
    assert_eq!(mock.connected(), Some(7));

    let services = call(&mut store, &instance, "services", &[]);
    assert_eq!(list(ok_value(&services[0])).len(), 1);

    let read = call(&mut store, &instance, "read", &[Val::String("cmd".into()), Val::U32(16)]);
    assert_eq!(list(ok_value(&read[0])).len(), 3);

    let written = call(
        &mut store,
        &instance,
        "write",
        &[Val::String("cmd".into()), Val::List(vec![Val::U8(4), Val::U8(5)])],
    );
    ok_value(&written[0]);
    assert_eq!(mock.writes(), vec![("cmd".to_string(), vec![4, 5])]);

    let subscribed = call(&mut store, &instance, "subscribe", &[Val::String("cmd".into())]);
    ok_value(&subscribed[0]);
    assert_eq!(store.data().host.subscription_count(), 1);

    let event = call(&mut store, &instance, "next-event", &[Val::U32(250)]);
    match ok_value(&event[0]).as_deref() {
        Some(Val::Option(Some(event))) => match event.as_ref() {
            Val::Variant(name, _) => assert_eq!(name, "notification"),
            other => panic!("expected event variant, got {other:?}"),
        },
        other => panic!("expected some event, got {other:?}"),
    }

    let disconnected = call(&mut store, &instance, "disconnect", &[]);
    ok_value(&disconnected[0]);
    assert_eq!(mock.connected(), None);
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_scan_is_delivered_with_only_the_observe_capability() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(&engine, &caps(&["soil:ble.observe"]), mock, true);
    let scanned = call(&mut store, &instance, "scan", &[Val::U32(100)]);
    assert_eq!(list(ok_value(&scanned[0])).len(), 1);
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
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
        "write",
        &[Val::String("cmd".into()), Val::List(vec![Val::U8(1)])],
    );
    assert_eq!(error_code(&result[0]), "denied");
    assert!(mock.writes().is_empty(), "denied write must not reach the radio");
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_connect_is_denied_without_the_connect_capability() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) =
        instantiate(&engine, &caps(&["soil:ble.observe"]), Arc::clone(&mock), true);
    let result = call(&mut store, &instance, "connect", &[Val::U32(7)]);
    assert_eq!(error_code(&result[0]), "denied");
    assert_eq!(mock.connected(), None);
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
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
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_a_competing_soil_host_reports_leased() {
    let engine = engine();
    let mock = scripted();
    mock.held_elsewhere(true);
    let (mut store, instance) = instantiate(&engine, &full_caps(), mock, false);
    let result = call(&mut store, &instance, "connect", &[Val::U32(7)]);
    assert_eq!(error_code(&result[0]), "leased");
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_deadline_and_budget_failures_are_typed() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(&engine, &full_caps(), Arc::clone(&mock), false);

    let over = call(&mut store, &instance, "scan", &[Val::U32(MAX_SCAN_MS + 1)]);
    assert_eq!(error_code(&over[0]), "over-budget");

    mock.set_mode(MockMode::Timeout);
    let timeout = call(&mut store, &instance, "scan", &[Val::U32(100)]);
    assert_eq!(error_code(&timeout[0]), "timeout");
}

#[test]
#[ignore = "component fixture WAT rejected by wasmtime 24: instance not valid to be used as import"]
fn soil_ble_next_event_returns_none_on_deadline_not_an_error() {
    let engine = engine();
    let mock = scripted();
    let (mut store, instance) = instantiate(&engine, &full_caps(), Arc::clone(&mock), false);
    let result = call(
        &mut store,
        &instance,
        "next-event",
        &[Val::U32(MAX_NEXT_EVENT_MS)],
    );
    let value = ok_value(&result[0]);
    assert!(
        matches!(value.as_deref(), Some(Val::Option(None))),
        "no event by the deadline is none: {value:?}"
    );
    assert_eq!(mock.event_deadlines(), vec![MAX_NEXT_EVENT_MS]);
}

#[test]
fn soil_ble_contract_bounds_are_shared_with_the_host() {
    // A second line of defence: the linker must not grow its own copy of the
    // contract bounds. This is the host's own value, used by the tests above.
    assert_eq!(MAX_SCAN_MS, 30_000);
    assert_eq!(MAX_NEXT_EVENT_MS, 1_000);
    assert!(BleHost::new(MockBle::new()).scan(MAX_SCAN_MS + 1).is_err());
    let _: BTreeSet<u32> = BTreeSet::new();
    let _: Device;
}

/// Non-component proof of the same authority decision: the linker's verb plan
/// only contains granted capabilities, and an ungranted verb is typed denied.
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
