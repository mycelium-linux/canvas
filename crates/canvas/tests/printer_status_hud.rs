#![cfg(feature = "lua-scenes")]
use canvas::{
    NodeContent,
    lua_module::{
        LuaModule, ModuleInputSnapshot, ModuleInputValue, ModuleManifest, ModuleThemeTokens,
    },
    lua_scene::{LuaSceneCapabilities, LuaSceneLimits},
};
use canvas_core::{Accent, ProjectTheme, ThemeMode};
use std::collections::BTreeMap;
const NOW: u64 = 1_780_000_000_000;
fn module(now: u64, observed: u64, sample: u64) -> LuaModule {
    let mut values = BTreeMap::new();
    for (k, v) in [("serial", "SERIAL"), ("link", "live")] {
        values.insert(k.into(), ModuleInputValue::Text(v.into()));
    }
    for (k, v) in [
        ("observed", observed as f64),
        ("expires", (observed + 6000) as f64),
        ("nozzle-c", 210.0),
        ("nozzle-ctime", sample as f64),
    ] {
        values.insert(k.into(), ModuleInputValue::Number(v));
    }
    let theme: ModuleThemeTokens = ProjectTheme {
        mode: ThemeMode::Dark,
        accent: Accent::Info,
    }
    .into();
    LuaModule::load(
        ModuleManifest::parse(include_str!(
            "../../../config/modules/printer-status-v1.json"
        ))
        .unwrap(),
        include_str!("../../../config/modules/printer-status-v1.lua"),
        ModuleInputSnapshot {
            revision: 1,
            now_ms: now,
            values,
        },
        theme,
        BTreeMap::new(),
        LuaSceneCapabilities::default(),
        LuaSceneLimits::default(),
    )
    .unwrap()
}
fn has(module: &LuaModule, needle: &str) -> bool {
    module
        .retained()
        .scene
        .nodes
        .iter()
        .any(|n| matches!(&n.content,NodeContent::Text{text,..} if text.contains(needle)))
}
#[test]
fn fresh_samples_render_without_controls() {
    let m = module(NOW, NOW, NOW);
    assert!(has(&m, "210°"));
    assert!(m.retained().actions.is_empty());
    assert!(m.retained().buttons.is_empty());
}
#[test]
fn stale_snapshot_and_individual_fields_never_look_live() {
    assert!(!has(&module(NOW + 6000, NOW, NOW), "210°"));
    assert!(!has(&module(NOW, NOW, NOW - 61000), "210°"));
    assert!(has(&module(NOW, 0, 0), "AWAITING ENROLLMENT"));
}

#[test]
fn three_printer_workspace_and_binder_share_the_module_contract() {
    use canvas::{
        SurfaceContent, WorkspaceDocument,
        stream_binding::{StreamEnvelope, apply_named_adapter},
    };
    let doc: WorkspaceDocument =
        serde_json::from_str(include_str!("../../../config/workspaces/tv/printers.json")).unwrap();
    let catalog=canvas_core::Catalog::parse(r#"{"schema_version":1,"projects":[{"id":"printers","name":"Lab printers","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}]}"#).unwrap();
    doc.validate(&catalog).unwrap();
    assert_eq!(doc.surfaces.len(), 3);
    let SurfaceContent::Widget { spec } = &doc.surfaces[0].content else {
        panic!("printer widget")
    };
    let mut spec = spec.clone();
    let event = StreamEnvelope {
        schema_version: 1,
        owner: "bambu".into(),
        stream: "printer-serial".into(),
        contract: "canvas.widget.snapshot.v1".into(),
        sequence: 1,
        emitted_at_ms: NOW,
        project_id: Some("printers".into()),
        payload: serde_json::json!({"printer":{
            "schema_version":1,"read_only":true,"serial":"SERIAL","observer":"lab-printer-1","link":"live",
            "observed_at_unix_ms":NOW,"valid_until_unix_ms":NOW+6000,"telemetry":{
                "nozzle_c":{"value":210,"observed_at_unix_ms":NOW},"job":{"value":"Current print","observed_at_unix_ms":NOW}
            }
        }}),
    };
    apply_named_adapter("printer-status-v1", &event, &mut spec).unwrap();
    spec.validate().unwrap();
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["module"], "printer-status-v1");
    assert_eq!(json["inputs"]["nozzle-c"]["value"].as_f64(), Some(210.0));
    assert_eq!(json["inputs"]["job"]["value"], "Current print");
}
