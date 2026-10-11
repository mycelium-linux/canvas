//! Canvas-owned XR control projection and activation over the existing Unibus SDK.
//! --stdin provides the same deterministic boundary without enrolling a bus client.
use anyhow::{Result, ensure};
use canvas::{
    lua_module::{
        InputDeclaration, InputKind, LuaModule, ModuleActionGrant, ModuleCapabilities, ModuleEvent,
        ModuleInputSnapshot, ModuleInputValue, ModuleManifest, ModuleThemeTokens,
    },
    lua_scene::{LuaSceneCapabilities, LuaSceneLimits},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unibus_sdk::{Publish, Session};

const SOURCE: &str = include_str!("../config/modules/holodeck-bench-v1.lua");
const STATE: &str = "canvas.xr.widget.state.v1";
const ACTIVATE: &str = "canvas.xr.widget.activate.v1";
const MAX_SESSIONS: usize = 16;
const SESSION_TTL_MS: u64 = 10000;
#[derive(Default)]
struct Views {
    sessions: BTreeMap<String, SessionView>,
}
struct SessionView {
    view: View,
    last_activation: u64,
    last_seen: u64,
}
impl Views {
    fn expire(&mut self, at: u64) {
        self.sessions
            .retain(|_, session| at.saturating_sub(session.last_seen) < SESSION_TTL_MS);
    }
    fn request(&mut self, kind: &str, body: Value, at: u64) -> Result<(&'static str, Value)> {
        self.expire(at);
        ensure!(
            serde_json::to_vec(&body)?.len() <= 16384,
            "UI body exceeds budget"
        );
        let session = body["session"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
            .ok_or_else(|| anyhow::anyhow!("invalid UI session"))?
            .to_owned();
        if kind == STATE {
            if let Some(old) = self.sessions.get_mut(&session) {
                let old_revision = old.view.state["state_revision"].as_u64().unwrap();
                let new_revision = body["state_revision"].as_u64().unwrap_or(0);
                ensure!(new_revision >= old_revision, "older UI state");
                if new_revision == old_revision {
                    ensure!(old.view.state == body, "conflicting UI revision");
                } else {
                    old.view.advance(project(body)?)?;
                }
                old.last_seen = at;
                return Ok(("canvas.xr.widget.scene.v1", old.view.scene()));
            }
            ensure!(
                self.sessions.len() < MAX_SESSIONS,
                "UI session budget exceeded"
            );
            let view = project(body)?;
            let scene = view.scene();
            self.sessions.insert(
                session,
                SessionView {
                    view,
                    last_activation: 0,
                    last_seen: at,
                },
            );
            Ok(("canvas.xr.widget.scene.v1", scene))
        } else {
            ensure!(kind == ACTIVATE, "unknown request");
            ensure!(body["schema_version"] == 1, "unsupported activation schema");
            let current = self
                .sessions
                .get_mut(&session)
                .ok_or_else(|| anyhow::anyhow!("no current UI session"))?;
            let id = body["activation_id"].as_u64().unwrap_or(0);
            ensure!(id > current.last_activation, "replayed activation");
            let action = current.view.activate(&body)?;
            current.last_activation = id;
            Ok(("canvas.xr.widget.action.v1", action))
        }
    }
}
struct View {
    state: Value,
    module: LuaModule,
    snapshot: ModuleInputSnapshot,
    theme: ModuleThemeTokens,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn project(state: Value) -> Result<View> {
    ensure!(
        state["schema_version"] == 1 && state["surface_id"] == "transmog-bench",
        "unsupported XR surface"
    );
    ensure!(
        state["session"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 128),
        "invalid UI session"
    );
    ensure!(
        state["state_revision"].as_u64().is_some_and(|r| r > 0),
        "invalid state revision"
    );
    let controls = state["controls"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing controls"))?;
    ensure!(
        !controls.is_empty() && controls.len() <= 48,
        "control budget exceeded"
    );
    let mut values = BTreeMap::new();
    let (mut low, mut high, mut visibility) = (0u32, 0u32, 0usize);
    for (n, c) in controls.iter().enumerate() {
        ensure!(
            c["id"] == format!("c{n}"),
            "controls must have canonical IDs"
        );
        let kind = c["kind"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing kind"))?;
        ensure!(
            kind.len() <= 32 && c["enabled"].is_boolean(),
            "invalid control"
        );
        if kind == "visibility" && visibility == 0 {
            visibility = n;
        }
        if c["enabled"] == true {
            if n < 24 {
                low |= 1 << n;
            } else {
                high |= 1 << (n - 24);
            }
        }
        values.insert(format!("control{n}"), ModuleInputValue::Text(kind.into()));
    }
    for (id, value) in [
        ("count", controls.len() as f64),
        ("enabled-low", low as f64),
        ("enabled-high", high as f64),
        ("visibility-start", visibility as f64),
    ] {
        values.insert(id.into(), ModuleInputValue::Number(value));
    }
    let mut declarations: Vec<InputDeclaration> = values
        .iter()
        .filter(|(id, _)| !id.starts_with("control"))
        .map(|(id, v)| InputDeclaration {
            id: id.clone(),
            kind: match v {
                ModuleInputValue::Text(_) => InputKind::Text,
                _ => InputKind::Number,
            },
            required: true,
        })
        .collect();
    declarations.extend((0..48).map(|n| InputDeclaration {
        id: format!("control{n}"),
        kind: InputKind::Text,
        required: false,
    }));
    let readable = declarations.iter().map(|d| d.id.clone()).collect();
    let manifest = ModuleManifest {
        schema_version: 1,
        id: "holodeck-bench-v1".into(),
        title: "Transmog bench".into(),
        inputs: declarations,
        resources: vec![],
        capabilities: ModuleCapabilities {
            write_scene: true,
            read_inputs: readable,
            read_theme: true,
            present_resources: vec![],
        },
        actions: vec![ModuleActionGrant {
            id: "activate".into(),
            destination: "holodeck.ui".into(),
            kind: "canvas.xr.widget.action.v1".into(),
        }],
    };
    let snapshot = ModuleInputSnapshot {
        revision: 1,
        now_ms: now(),
        values,
    };
    let theme = ModuleThemeTokens::from(canvas_core::ProjectTheme {
        mode: canvas_core::ThemeMode::Dark,
        accent: canvas_core::Accent::Brand,
    });
    let module = LuaModule::load(
        manifest,
        SOURCE,
        snapshot.clone(),
        theme,
        BTreeMap::new(),
        LuaSceneCapabilities::default(),
        LuaSceneLimits::default(),
    )?;
    Ok(View {
        state,
        module,
        snapshot,
        theme,
    })
}
impl View {
    fn advance(&mut self, mut next: View) -> Result<()> {
        ensure!(
            next.state["session"] == self.state["session"]
                && next.state["state_revision"].as_u64() > self.state["state_revision"].as_u64(),
            "state must advance within its session"
        );
        next.snapshot.revision = self.snapshot.revision + 1;
        self.module.update(
            next.snapshot.clone(),
            next.theme,
            ModuleEvent::DataChanged {
                changed_inputs: next.snapshot.values.keys().cloned().collect(),
            },
        )?;
        self.snapshot = next.snapshot;
        self.state = next.state;
        self.theme = next.theme;
        Ok(())
    }
    fn scene(&self) -> Value {
        let mut retained =
            serde_json::to_value(self.module.retained()).expect("serializable retained scene");
        retained["actions"] = json!([]);
        json!({"schema_version":1,"surface_id":"transmog-bench","session":self.state["session"],
   "state_revision":self.state["state_revision"],"module_id":"holodeck-bench-v1","retained":retained})
    }
    fn activate(&mut self, body: &Value) -> Result<Value> {
        ensure!(
            body["session"] == self.state["session"]
                && body["surface_id"] == self.state["surface_id"]
                && body["state_revision"] == self.state["state_revision"],
            "stale activation"
        );
        let id = body["target_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing target"))?;
        ensure!(
            self.module
                .retained()
                .buttons
                .iter()
                .any(|b| b.id == id && b.enabled),
            "unavailable target"
        );
        let request = body["activation_id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing activation ID"))?;
        self.snapshot.revision += 1;
        self.snapshot.now_ms = now();
        let output = self.module.update(
            self.snapshot.clone(),
            self.theme,
            ModuleEvent::Activate {
                target_id: id.into(),
            },
        )?;
        ensure!(
            output.actions.len() == 1 && output.actions[0].body["target_id"] == id,
            "invalid action projection"
        );
        Ok(
            json!({"schema_version":1,"surface_id":"transmog-bench","session":self.state["session"],"state_revision":self.state["state_revision"],"activation_id":request,"target_id":id}),
        )
    }
}
fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) == Some("--export-lua") {
        let output = std::path::Path::new(
            arguments
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("output path required"))?,
        );
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, SOURCE)?;
        return Ok(());
    }
    if std::env::args().any(|a| a == "--stdin") {
        let mut views = Views::default();
        for line in io::stdin().lock().lines() {
            let v: Value = serde_json::from_str(&line?)?;
            let (_, result) =
                views.request(v["kind"].as_str().unwrap_or(""), v["body"].clone(), now())?;
            println!("{}", serde_json::to_string(&result)?);
        }
        return Ok(());
    }
    let path = std::env::var("CANVAS_XR_BUS_CONFIG")?;
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 65536, "config exceeds budget");
    let config: Value = serde_json::from_slice(&bytes)?;
    let owner = config["producer_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing owner identity"))?
        .to_owned();
    let peer = std::env::var("CANVAS_XR_PEER")?;
    let bus = Session::connect(serde_json::from_value(config)?)?;
    let mut views = Views::default();
    loop {
        views.expire(now());
        while let Some(event) = bus.poll() {
            if event.kind == "reply" && event.body["success"] == false {
                eprintln!("Canvas XR publication rejected by Unibus");
                continue;
            }
            if event.producer_id != peer
                || event.destination.as_deref() != Some(owner.as_str())
                || (event.kind != STATE && event.kind != ACTIVATE)
            {
                continue;
            }
            let result = views.request(&event.kind, event.body, now());
            match result {
                Ok((kind, body)) => {
                    bus.publish(Publish {
                        destination: Some(peer.clone()),
                        kind: kind.into(),
                        body,
                    })?;
                }
                Err(error) => {
                    eprintln!("Canvas XR request rejected: {error}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Value {
        json!({"schema_version":1,"surface_id":"transmog-bench","session":"test","state_revision":1,"controls":[{"id":"c0","kind":"project","enabled":true},{"id":"c1","kind":"part","enabled":true},{"id":"c2","kind":"visibility","enabled":true},{"id":"c3","kind":"thermal","enabled":false}]})
    }
    #[test]
    fn real_canvas_projection_and_activation() {
        let mut view = project(state()).unwrap();
        assert_eq!(view.module.retained().buttons.len(), 4);
        assert!(view.module.retained().actions.is_empty());
        let request = json!({"surface_id":"transmog-bench","session":"test","state_revision":1,"activation_id":1,"target_id":"c1"});
        assert_eq!(view.activate(&request).unwrap()["target_id"], "c1");
        let mut denied = request.clone();
        denied["target_id"] = json!("c3");
        assert!(view.activate(&denied).is_err());
        denied = request;
        denied["state_revision"] = json!(2);
        assert!(view.activate(&denied).is_err());
    }
    #[test]
    fn rejects_unknown_semantics_and_oversize() {
        let mut bad = state();
        bad["controls"][0]["kind"] = json!("shell-execute");
        assert!(project(bad).is_err());
        let mut bad = state();
        bad["controls"] = json!(vec![json!({"id":"c0","kind":"part","enabled":true}); 49]);
        assert!(project(bad).is_err());
    }
    #[test]
    fn retained_revisions_advance_with_control_changes() {
        let mut view = project(state()).unwrap();
        let original = view.module.retained().output_revision;
        let mut next = state();
        next["state_revision"] = json!(2);
        next["controls"].as_array_mut().unwrap().pop();
        view.advance(project(next).unwrap()).unwrap();
        assert!(view.module.retained().output_revision > original);
        assert_eq!(view.module.retained().buttons.len(), 3);
        assert_eq!(view.module.retained().module_generation, 1);
    }
    #[test]
    fn independent_sessions_preserve_selection_and_replay_guards() {
        let mut views = Views::default();
        let a = state();
        let mut b = state();
        b["session"] = json!("james");
        b["controls"][1]["enabled"] = json!(false);
        views.request(STATE, a.clone(), 0).unwrap();
        views.request(STATE, b.clone(), 1).unwrap();
        let click = json!({"schema_version":1,"surface_id":"transmog-bench",
            "session":"test","state_revision":1,"activation_id":1,"target_id":"c1"});
        assert_eq!(
            views.request(ACTIVATE, click.clone(), 2).unwrap().1["session"],
            "test"
        );
        let mut other = click.clone();
        other["session"] = json!("james");
        assert!(views.request(ACTIVATE, other.clone(), 3).is_err());
        other["target_id"] = json!("c0");
        assert!(
            views.request(ACTIVATE, other.clone(), 4).is_ok(),
            "activation IDs are per session"
        );
        assert!(
            views.request(ACTIVATE, click, 5).is_err(),
            "another session cannot reset replay guard"
        );
        let before = views.sessions["james"]
            .view
            .module
            .retained()
            .output_revision;
        views.request(STATE, b, 6).unwrap();
        assert_eq!(
            views.sessions["james"]
                .view
                .module
                .retained()
                .output_revision,
            before,
            "heartbeats reuse the retained scene"
        );
        let mut newer = a;
        newer["state_revision"] = json!(2);
        views.request(STATE, newer, 7).unwrap();
        assert_eq!(views.sessions["james"].view.state["state_revision"], 1);
        assert!(
            views.request(ACTIVATE, other, SESSION_TTL_MS + 6).is_err(),
            "expired sessions cannot activate"
        );
        assert_eq!(views.sessions.len(), 1);
    }
    #[test]
    fn session_budget_expiry_and_bad_heartbeats() {
        let mut views = Views::default();
        for n in 0..MAX_SESSIONS {
            let mut body = state();
            body["session"] = json!(format!("room-member-{n}"));
            views.request(STATE, body, 0).unwrap();
        }
        assert!(views.request(STATE, state(), 1).is_err());
        let mut bad = state();
        bad["session"] = json!("room-member-0");
        bad["controls"][0]["enabled"] = json!(false);
        assert!(views.request(STATE, bad, SESSION_TTL_MS - 1).is_err());
        assert_eq!(
            views.sessions["room-member-0"].last_seen, 0,
            "rejections do not renew sessions"
        );
        assert!(views.request(STATE, state(), SESSION_TTL_MS).is_ok());
        assert_eq!(views.sessions.len(), 1);
    }
}
