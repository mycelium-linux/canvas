#![cfg(feature = "lua-scenes")]
use canvas::{
    NodeContent, SurfaceContent, WorkspaceDocument,
    lua_module::{
        LuaModule, ModuleEvent, ModuleInputSnapshot, ModuleInputValue, ModuleManifest,
        ModuleThemeTokens, StatusLevel,
    },
    lua_scene::{LuaSceneCapabilities, LuaSceneLimits},
};
use canvas_core::{Accent, ProjectTheme, ThemeMode};
use std::collections::BTreeMap;
const NOW: u64 = 1_780_000_000_000;
const SOURCE: &str = include_str!("../../../config/modules/wiki-race-v1.lua");
const MANIFEST: &str = include_str!("../../../config/modules/wiki-race-v1.json");

#[test]
fn lua_activation_roundtrips_over_real_unibus_framing() {
    use std::{
        io::Write,
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };
    use unibus::{Envelope, Message, framing, kinds};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut token = tempfile::NamedTempFile::new().unwrap();
    token.write_all(b"private-host-token").unwrap();
    let config = serde_json::from_value(serde_json::json!({
        "installation_id":"test","producer_id":"canvas-wikiracing", "zones":["home"],
        "hub_endpoint":listener.local_addr().unwrap().to_string(),"token_file":token.path(),"schema_version":2
    })).unwrap();
    let (release, held) = std::sync::mpsc::channel();
    let router = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (registration, _) = framing::read_frame(&mut socket).unwrap();
        let ack = Envelope::new(
            "test",
            "router",
            Some("canvas-wikiracing"),
            "private-host-token",
            Message::new(
                kinds::V1_REPLY,
                serde_json::json!({"request_id":registration.request_id,"success":true}),
            ),
        );
        framing::write_frame(&mut socket, &ack, &[]).unwrap();
        let command = loop {
            let (command, _) = framing::read_frame(&mut socket).unwrap();
            if command.message.kind == "wiki.race.control.v1" {
                break command;
            }
        };
        assert_eq!(command.destination.as_deref(), Some("wiki-race-controller"));
        assert_eq!(command.message.body["action"], "start");
        assert_eq!(command.message.body["expected_revision"], 7);
        let reply = Envelope::new(
            "test",
            "wiki-race-controller",
            Some("canvas-wikiracing"),
            "private-host-token",
            Message::new(
                "canvas.module.action.result.v1",
                serde_json::json!({"request_id":command.request_id,"ok":true}),
            ),
        );
        framing::write_frame(&mut socket, &reply, &[]).unwrap();
        held.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let mut bus =
        canvas::module_bus::ModuleBus::from_session(unibus_sdk::Session::connect(config).unwrap());
    let mut module = module("READY");
    module
        .update(
            snapshot(2, "READY", NOW),
            theme(),
            ModuleEvent::Activate {
                target_id: "start-control".into(),
            },
        )
        .unwrap();
    let id = bus.publish(module.retained().actions[0].clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = bus.poll();
        if let Some(event) = events.first() {
            assert!(
                !serde_json::to_string(event)
                    .unwrap()
                    .contains("private-host-token")
            );
            assert_eq!(event.body["request_id"], id);
            assert!(bus.receipt(event).unwrap().contains(&id));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    release.send(()).unwrap();
    router.join().unwrap();
}

fn theme() -> ModuleThemeTokens {
    ProjectTheme {
        mode: ThemeMode::Dark,
        accent: Accent::Info,
    }
    .into()
}
fn snapshot(revision: u64, phase: &str, now_ms: u64) -> ModuleInputSnapshot {
    let mut values = BTreeMap::new();
    for (id, value) in [
        ("start", "Kevin Costner"),
        ("target", "Nebuchadnezzar"),
        ("raceid", "race-1"),
        ("raceclock", "00:00.00"),
        ("wallclock", "00:00.00"),
    ] {
        values.insert(id.into(), ModuleInputValue::Text(value.into()));
    }
    for (id, value) in [
        ("revision", 7.0),
        ("clicks", 4.0),
        ("racems", 12340.0),
        ("wallms", 15340.0),
        ("observed", NOW as f64),
    ] {
        values.insert(id.into(), ModuleInputValue::Number(value));
    }
    values.insert("connected".into(), ModuleInputValue::Boolean(true));
    values.insert(
        "status".into(),
        ModuleInputValue::Status {
            level: StatusLevel::Good,
            label: phase.into(),
        },
    );
    ModuleInputSnapshot {
        revision,
        now_ms,
        values,
    }
}
fn module(phase: &str) -> LuaModule {
    LuaModule::load(
        ModuleManifest::parse(MANIFEST).unwrap(),
        SOURCE,
        snapshot(1, phase, NOW),
        theme(),
        BTreeMap::new(),
        LuaSceneCapabilities::default(),
        LuaSceneLimits::default(),
    )
    .unwrap()
}

#[test]
fn hud_is_lua_native_and_reload_never_emits_commands() {
    let mut module = module("READY");
    assert!(module.retained().buttons[0].enabled);
    assert!(!module.retained().buttons[1].enabled);
    assert!(module.retained().actions.is_empty());
    module
        .hot_reload(ModuleManifest::parse(MANIFEST).unwrap(), SOURCE)
        .unwrap();
    assert!(module.retained().actions.is_empty());
    assert!(
        module
            .retained()
            .scene
            .nodes
            .iter()
            .any(|node| matches!(&node.content,
        NodeContent::Text {text,..} if text == "00:12.34"))
    );
}

#[test]
fn controls_use_current_revision_and_disabled_controls_do_not_emit() {
    let mut module = module("READY");
    module
        .update(
            snapshot(2, "READY", NOW),
            theme(),
            ModuleEvent::Activate {
                target_id: "start-control".into(),
            },
        )
        .unwrap();
    let action = &module.retained().actions[0];
    assert_eq!(action.kind, "wiki.race.control.v1");
    assert_eq!(action.destination, "wiki-race-controller");
    assert_eq!(action.body["expected_revision"], 7);
    assert_eq!(action.body["action"], "start");
    let mut newer = snapshot(3, "RACING", NOW);
    newer
        .values
        .insert("revision".into(), ModuleInputValue::Number(8.0));
    module
        .update(
            newer.clone(),
            theme(),
            ModuleEvent::Activate {
                target_id: "stop-control".into(),
            },
        )
        .unwrap();
    assert_eq!(module.retained().actions[0].body["expected_revision"], 8);
    assert_eq!(module.retained().actions[0].body["action"], "stop");
    newer.revision = 4;
    module
        .update(
            newer,
            theme(),
            ModuleEvent::Activate {
                target_id: "start-control".into(),
            },
        )
        .unwrap();
    assert!(module.retained().actions.is_empty());
}

#[test]
fn stale_and_finished_states_are_honest() {
    let mut module = module("HOLDING");
    module
        .update(
            snapshot(2, "HOLDING", NOW + 6000),
            theme(),
            ModuleEvent::Activate {
                target_id: "stop-control".into(),
            },
        )
        .unwrap();
    assert!(
        module
            .retained()
            .buttons
            .iter()
            .all(|button| !button.enabled)
    );
    assert!(module.retained().actions.is_empty());
    module
        .update(
            snapshot(3, "FINISHED", NOW),
            theme(),
            ModuleEvent::DataChanged {
                changed_inputs: vec!["status".into()],
            },
        )
        .unwrap();
    assert!(
        module
            .retained()
            .scene
            .nodes
            .iter()
            .any(|node| matches!(&node.content,
        NodeContent::Text {text,..} if text == "FINAL TIME"))
    );
    assert!(!module.retained().buttons[1].enabled);
}

#[test]
fn unauthorized_routes_and_failed_candidates_cannot_escape() {
    let mut module = module("READY");
    let before = module.retained().clone();
    let bad = SOURCE.replace("action=\"start\"", "action=\"ungranted\"");
    assert!(
        module
            .hot_reload(ModuleManifest::parse(MANIFEST).unwrap(), bad)
            .is_err()
    );
    assert_eq!(&before, module.retained());
    let bad = format!("{SOURCE}\nerror('candidate failed')");
    assert!(
        module
            .hot_reload(ModuleManifest::parse(MANIFEST).unwrap(), bad)
            .is_err()
    );
    assert_eq!(&before, module.retained());
}

#[test]
fn controller_snapshot_binds_into_module_and_bad_data_preserves_it() {
    let workspace: WorkspaceDocument = serde_json::from_str(include_str!(
        "../../../config/workspaces/tv/wikiracing.json"
    ))
    .unwrap();
    let SurfaceContent::Widget { mut spec } = workspace.surfaces[0].content.clone() else {
        panic!("expected module")
    };
    let mut event = canvas::stream_contract::StreamEnvelope {
        schema_version: 1,
        owner: "surf".into(),
        stream: "wiki-race".into(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence: 1,
        emitted_at_ms: NOW,
        project_id: Some("wikiracing".into()),
        payload: serde_json::json!({"race":{"schema_version":1,"race_id":"race-1","revision":7,"phase":"racing",
            "start_page":"Kevin Costner","target_page":"Nebuchadnezzar","clicks":4,
            "race_elapsed_ms":12340,"wall_elapsed_ms":15340,"observed_at_ms":NOW}}),
    };
    canvas::stream_binding::apply_named_adapter("wiki-race-state-v1", &event, &mut spec).unwrap();
    let before = spec.clone();
    event.payload["race"]["race_elapsed_ms"] = serde_json::json!(999999);
    assert!(
        canvas::stream_binding::apply_named_adapter("wiki-race-state-v1", &event, &mut spec)
            .is_err()
    );
    assert_eq!(before, spec);
}
