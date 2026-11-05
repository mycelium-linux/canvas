//! Bounded Lua presentation for desktop chrome. Native hosts own all actions.
//!
//! A candidate is validated before replacing the retained output. Cache keys
//! include source, manifest, theme and host inputs (including the supplied clock).
//! Hosts choose their clock cadence and perform file reads outside `view`.
use crate::{
    lua_module::{
        LuaModule, ModuleInputSnapshot, ModuleManifest, ModuleThemeTokens, RetainedModuleScene,
    },
    lua_scene::{LuaSceneCapabilities, LuaSceneLimits},
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub struct Artifact {
    pub manifest_path: &'static str,
    pub manifest_fallback: &'static str,
    pub source_path: &'static str,
    pub source_fallback: &'static str,
    /// The shell region this artifact draws, if a Spore may claim it.
    pub region: Option<&'static str>,
}

pub const DESKTOP_BAR: Artifact = Artifact {
    manifest_path: "modules/desktop-bar-v1.json",
    manifest_fallback: include_str!("../../../config/modules/desktop-bar-v1.json"),
    source_path: "modules/desktop-bar-v1.lua",
    source_fallback: include_str!("../../../config/modules/desktop-bar-v1.lua"),
    region: Some("top-bar"),
};
pub const DESKTOP_LAUNCHER: Artifact = Artifact {
    manifest_path: "modules/desktop-launcher-v1.json",
    manifest_fallback: include_str!("../../../config/modules/desktop-launcher-v1.json"),
    source_path: "modules/desktop-launcher-v1.lua",
    source_fallback: include_str!("../../../config/modules/desktop-launcher-v1.lua"),
    region: Some("launcher"),
};
pub const DESKTOP_DOCK: Artifact = Artifact {
    manifest_path: "modules/desktop-dock-v1.json",
    manifest_fallback: include_str!("../../../config/modules/desktop-dock-v1.json"),
    source_path: "modules/desktop-dock-v1.lua",
    source_fallback: include_str!("../../../config/modules/desktop-dock-v1.lua"),
    region: Some("dock"),
};

pub const CONTROL_CENTER: Artifact = Artifact {
    manifest_path: "modules/control-center-v1.json",
    manifest_fallback: include_str!("../../../config/modules/control-center-v1.json"),
    source_path: "modules/control-center-v1.lua",
    source_fallback: include_str!("../../../config/modules/control-center-v1.lua"),
    region: Some("control-center"),
};

/// Frameless instruments are desktop content, not app-switching destinations.
/// Use this same projection for presentation slots and their native actions.
pub use crate::shell_launcher::dock_surfaces;

#[derive(Debug, Clone, Default)]
pub struct Presentation {
    /// Which Spore last drew this presentation, so a change of owner is logged
    /// once rather than every frame.
    chrome_source: Option<String>,
    key: Option<[u8; 32]>,
    retained: Option<RetainedModuleScene>,
    current: Option<(std::sync::Arc<str>, std::sync::Arc<str>)>,
}

impl Presentation {
    pub fn retained(&self) -> Option<&RetainedModuleScene> {
        self.retained.as_ref()
    }

    /// Whether this region's current scene asked for a frame clock.
    pub fn animated(&self) -> bool {
        self.retained.as_ref().is_some_and(|retained| retained.animated)
    }

    /// Update native state immediately using the last accepted artifact, with
    /// no filesystem access. The caller polls edited artifacts separately.
    pub fn refresh_current(
        &mut self,
        artifact: Artifact,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
    ) -> crate::Result<bool> {
        let (manifest, source) = self.current.clone().unwrap_or_else(|| {
            (
                artifact.manifest_fallback.into(),
                artifact.source_fallback.into(),
            )
        });
        self.refresh(&manifest, &source, snapshot, theme)
    }

    pub fn refresh_files(
        &mut self,
        artifact: Artifact,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
    ) -> crate::Result<bool> {
        // A chrome Spore draws the region if one is installed: the shell keeps
        // the extent, the input routing and the fallback, and the Spore owns the
        // composition. An uninstalled or broken region falls back per region, so
        // the desktop never loses its bar because a package did not load.
        let spore = artifact.region.and_then(|region| {
            let chrome = crate::spore_catalog::chrome_source(region)?;
            if !self.chrome_source.as_deref().is_some_and(|current| current == chrome.spore_id) {
                eprintln!(
                    "canvas: chrome {region} from Spore {}",
                    chrome.spore_id
                );
            }
            Some(chrome)
        });
        if let Some(chrome) = &spore {
            self.chrome_source = Some(chrome.spore_id.clone());
        }
        let read = |path: &str, fallback: &str| -> crate::Result<String> {
            crate::runtime_config::read(std::path::Path::new(path), 32 * 1024)
                .map_err(|e| crate::Error(e.to_string()))?
                .map(|bytes| String::from_utf8(bytes).map_err(|e| crate::Error(e.to_string())))
                .transpose()
                .map(|value| value.unwrap_or_else(|| fallback.into()))
        };
        // The widget library is executed inside the module's context, not
        // prepended, so its identity is carried as a marker comment: the cache key
        // hashes the source text, and an edited button must invalidate the scene
        // it drew.
        let source = match spore {
            Some(chrome) => chrome.source,
            None => read(artifact.source_path, artifact.source_fallback)?,
        };
        let source = format!(
            "{}\n-- canvas-widgets {}",
            source,
            crate::lua_module::widget_fingerprint()
        );
        self.refresh(
            &read(artifact.manifest_path, artifact.manifest_fallback)?,
            &source,
            snapshot,
            theme,
        )
    }

    /// Returns false for unchanged input without executing Lua. An error leaves
    /// both the cache key and last-good output untouched, allowing a later retry.
    pub fn refresh(
        &mut self,
        manifest: &str,
        source: &str,
        snapshot: ModuleInputSnapshot,
        theme: ModuleThemeTokens,
    ) -> crate::Result<bool> {
        let encoded = serde_json::to_vec(&(manifest, source, &snapshot, theme))
            .map_err(|e| crate::Error(e.to_string()))?;
        let key: [u8; 32] = Sha256::digest(encoded).into();
        if self.key == Some(key) {
            return Ok(false);
        }
        let candidate = LuaModule::load(
            ModuleManifest::parse(manifest)?,
            source,
            snapshot,
            theme,
            Default::default(),
            LuaSceneCapabilities::default(),
            LuaSceneLimits::default(),
        )?;
        if !candidate.retained().actions.is_empty() {
            return Err(crate::Error(
                "Shell presentation cannot execute actions on refresh".into(),
            ));
        }
        self.retained = Some(candidate.retained().clone());
        self.current = Some((manifest.into(), source.into()));
        self.key = Some(key);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Accent, ProjectTheme, ThemeMode};

    const MANIFEST: &str = include_str!("../../../config/modules/desktop-launcher-v1.json");
    const SOURCE: &str = include_str!("../../../config/modules/desktop-launcher-v1.lua");
    fn theme() -> ModuleThemeTokens {
        ProjectTheme {
            mode: ThemeMode::Dark,
            accent: Accent::Info,
        }
        .into()
    }
    fn snapshot() -> ModuleInputSnapshot {
        ModuleInputSnapshot {
            revision: 1,
            now_ms: 1_768_000_000_000,
            values: Default::default(),
        }
    }

    #[test]
    fn launcher_rows_draw_a_tinted_monogram() {
        use crate::lua_module::ModuleInputValue as V;
        let mut p = Presentation::default();
        let mut s = snapshot();
        s.values = std::collections::BTreeMap::from([
            ("width".into(), V::Number(600.0)),
            ("height".into(), V::Number(440.0)),
            ("live".into(), V::Boolean(true)),
            ("count".into(), V::Number(1.0)),
            ("row0-title".into(), V::Text("Inbox".into())),
            ("row0-detail".into(), V::Text("APP · 0.1.0".into())),
            ("row0-available".into(), V::Boolean(true)),
            ("row0-selected".into(), V::Boolean(true)),
            ("row0-badge".into(), V::Text("I".into())),
            ("row0-tint".into(), V::Text("#6ea8fe".into())),
        ]);
        p.refresh(
            DESKTOP_LAUNCHER.manifest_fallback,
            DESKTOP_LAUNCHER.source_fallback,
            s,
            theme(),
        )
        .expect("the launcher must render a row with a mark");
        let scene = &p.retained().expect("scene").scene;
        let mark = scene
            .nodes
            .iter()
            .find(|node| node.id == "row0-mark")
            .unwrap_or_else(|| panic!("a row with a tint has a mark; got {:?}", scene.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>()));
        let canvas_workspace::NodeContent::Rect { rgb, .. } = &mark.content else {
            panic!("the mark is a rectangle");
        };
        assert_eq!(*rgb, Some([0x6e, 0xa8, 0xfe]));
        assert!(
            scene.nodes.iter().any(|node| node.id == "row0-glyph"),
            "the monogram is drawn"
        );
    }

    #[test]
    fn unchanged_input_reuses_output_and_clone_retains_it() {
        let mut p = Presentation::default();
        assert!(p.refresh(MANIFEST, SOURCE, snapshot(), theme()).unwrap());
        let retained = p.retained().unwrap().clone();
        assert!(!p.refresh(MANIFEST, SOURCE, snapshot(), theme()).unwrap());
        assert_eq!(p.clone().retained(), Some(&retained));
    }

    #[test]
    fn invalid_reload_retains_last_good_and_valid_edit_recovers() {
        let mut p = Presentation::default();
        p.refresh(MANIFEST, SOURCE, snapshot(), theme()).unwrap();
        let retained = p.retained().unwrap().clone();
        assert!(
            p.refresh(MANIFEST, "this is not Lua", snapshot(), theme())
                .is_err()
        );
        assert_eq!(p.retained(), Some(&retained));
        assert!(p.refresh("{}", SOURCE, snapshot(), theme()).is_err());
        assert_eq!(p.retained(), Some(&retained));
        let mut changed = snapshot();
        changed.now_ms += 1000;
        assert!(
            p.refresh_current(DESKTOP_LAUNCHER, changed, theme())
                .unwrap()
        );
        assert!(
            p.refresh(
                MANIFEST,
                &format!("{SOURCE}\n-- edited"),
                snapshot(),
                theme()
            )
            .unwrap()
        );
    }

    #[test]
    fn theme_inputs_and_host_clock_invalidate_cache() {
        let mut p = Presentation::default();
        p.refresh(MANIFEST, SOURCE, snapshot(), theme()).unwrap();
        let mut t = theme();
        t.body += 1.0;
        assert!(p.refresh(MANIFEST, SOURCE, snapshot(), t).unwrap());
        let mut s = snapshot();
        s.values.insert(
            "query".into(),
            crate::lua_module::ModuleInputValue::Text("clock".into()),
        );
        assert!(p.refresh(MANIFEST, SOURCE, s.clone(), t).unwrap());
        s.now_ms += 1000;
        assert!(p.refresh(MANIFEST, SOURCE, s, t).unwrap());
    }

    #[test]
    fn desktop_bar_is_opaque_responsive_and_has_only_native_controls() {
        use crate::lua_module::ModuleInputValue as V;
        for width in [320.0, 600.0, 850.0, 1920.0] {
            let mut s = snapshot();
            s.values = std::collections::BTreeMap::from([
                ("width".into(), V::Number(width)),
                ("height".into(), V::Number(44.0)),
                (
                    "padding".into(),
                    V::Number(f64::from(crate::DESKTOP_BAR_PADDING)),
                ),
                (
                    "labelsize".into(),
                    V::Number(f64::from(crate::DESKTOP_BAR_TEXT_SIZE)),
                ),
                ("workspace".into(), V::Text("HOME".into())),
                ("page".into(), V::Text("DESKTOP".into())),
                ("clock".into(), V::Text("12:34:56".into())),
                ("sources".into(), V::Number(3.0)),
                ("back".into(), V::Boolean(false)),
                ("forward".into(), V::Boolean(true)),
                ("create".into(), V::Boolean(false)),
            ]);
            let mut p = Presentation::default();
            p.refresh(
                include_str!("../../../config/modules/desktop-bar-v1.json"),
                include_str!("../../../config/modules/desktop-bar-v1.lua"),
                s,
                theme(),
            )
            .unwrap();
            let output = p.retained().unwrap();
            assert!(output.actions.is_empty());
            assert!(
                output
                    .buttons
                    .iter()
                    .any(|b| b.id == "launcher" && b.enabled)
            );
            for b in &output.buttons {
                assert!(["back", "forward", "launcher", "create"].contains(&b.id.as_str()));
                assert!(b.x >= -4.0 && b.x + b.width <= 4.001, "{width}: {b:?}");
                assert!(b.y <= 4.0 && b.y - b.height >= -4.001);
                assert_eq!(b.y, 4.0);
                assert_eq!(
                    b.height, 8.0,
                    "shell controls retain the full logical hit row"
                );
                if b.id == "back" || b.id == "create" {
                    assert!(!b.enabled);
                }
            }
            let encoded = serde_json::to_string(&output.scene).unwrap();
            assert!(encoded.contains("sources") == (width >= 750.0));
            assert!(!output.buttons.iter().any(|b| b.id == "create"));
            assert!(encoded.contains("12:34") == (width >= 360.0));
            assert!(!encoded.contains("12:34:56"));
            assert!(!encoded.contains("LIVE"));
            assert!(encoded.contains("\"opacity\":1.0"));
        }
    }

    #[test]
    fn bottom_dock_is_bounded_opaque_and_never_emits_actions_on_refresh() {
        use crate::lua_module::ModuleInputValue as V;
        for width in [180.0, 320.0, 600.0, 1408.0, 1920.0] {
            for count in [0, 1, 6, 25] {
                let mut s = snapshot();
                s.values = std::collections::BTreeMap::from([
                    ("width".into(), V::Number(width)),
                    ("height".into(), V::Number(68.0)),
                    ("count".into(), V::Number(count as f64)),
                ]);
                for n in 0..count.min(6) {
                    s.values
                        .insert(format!("item{n}-id"), V::Text(format!("surface-{n}")));
                    s.values
                        .insert(format!("item{n}-title"), V::Text(format!("Window {n}")));
                    s.values
                        .insert(format!("item{n}-focused"), V::Boolean(n == 0));
                    s.values
                        .insert(format!("item{n}-minimized"), V::Boolean(n == 1));
                }
                let mut p = Presentation::default();
                p.refresh(
                    DESKTOP_DOCK.manifest_fallback,
                    DESKTOP_DOCK.source_fallback,
                    s,
                    theme(),
                )
                .unwrap();
                let output = p.retained().unwrap();
                assert!(output.actions.is_empty());
                assert_eq!(output.buttons[0].id, "launcher");
                assert!(output.buttons.len() <= 7);
                for button in &output.buttons {
                    assert_eq!(
                        button.appearance,
                        crate::lua_module::ModuleButtonAppearance::Quiet
                    );
                    assert!(
                        button.x >= -4.0 && button.x + button.width <= 4.001,
                        "{width}: {button:?}"
                    );
                    assert!(button.y <= 4.0 && button.y - button.height >= -4.001);
                }
                assert!(
                    serde_json::to_string(&output.scene)
                        .unwrap()
                        .contains("\"opacity\":1.0")
                );
            }
        }
    }

    #[test]
    fn frameless_desktop_content_is_not_an_application() {
        let mut document: crate::WorkspaceDocument =
            serde_json::from_str(include_str!("../../../config/workspaces/neo/fungos.json"))
                .unwrap();
        assert_eq!(dock_surfaces(&document).count(), 0);
        document.surfaces.push(crate::Surface {
            id: "simulator".into(),
            title: "Simulator".into(),
            content: crate::SurfaceContent::ExternalSurface {
                source_id: "native/simulator".into(),
            },
            inputs: vec![],
        });
        assert_eq!(
            dock_surfaces(&document)
                .map(|surface| surface.id.as_str())
                .collect::<Vec<_>>(),
            vec!["simulator"]
        );
    }

    #[test]
    fn button_appearance_is_semantic_bounded_and_backward_compatible() {
        let manifest = r#"{"schema_version":1,"id":"appearance-test","title":"Appearance","inputs":[],"capabilities":{"write_scene":true},"actions":[{"id":"select","destination":"canvas.shell","kind":"select"}]}"#;
        for (field, expected) in [
            (
                "",
                Some(crate::lua_module::ModuleButtonAppearance::Standard),
            ),
            (
                ",appearance='quiet'",
                Some(crate::lua_module::ModuleButtonAppearance::Quiet),
            ),
            (",appearance='arbitrary-css'", None),
        ] {
            let source = format!(
                "widget.button({{id='select',label='Select',action='select',body={{}},x=-1,y=1,width=2,height=1{field}}})"
            );
            let mut p = Presentation::default();
            let result = p.refresh(manifest, &source, snapshot(), theme());
            if let Some(expected) = expected {
                result.unwrap();
                assert_eq!(p.retained().unwrap().buttons[0].appearance, expected);
                assert!(p.retained().unwrap().actions.is_empty());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn switcher_has_only_compact_window_tiles_not_launcher_chrome() {
        use crate::lua_module::ModuleInputValue as V;
        for count in [1, 3, 6] {
            let [width, height] = crate::shell_geometry::sheet_size(true, count, [1408.0, 881.0]);
            let mut s = snapshot();
            s.values = std::collections::BTreeMap::from([
                ("width".into(), V::Number(f64::from(width))),
                ("height".into(), V::Number(f64::from(height))),
                ("window-cycle".into(), V::Boolean(true)),
                ("count".into(), V::Number(count as f64)),
                ("live".into(), V::Boolean(true)),
            ]);
            for n in 0..count {
                s.values.insert(
                    format!("row{n}-title"),
                    V::Text("日本語のウィンドウ 🎹".into()),
                );
                s.values
                    .insert(format!("row{n}-available"), V::Boolean(true));
            }
            let mut p = Presentation::default();
            p.refresh(MANIFEST, SOURCE, s, theme()).unwrap();
            let output = p.retained().unwrap();
            assert_eq!(output.buttons.len(), count);
            assert!(output.actions.is_empty());
            assert_eq!(
                output.scene.nodes.len(),
                1,
                "only the shared panel, no search/help/status labels"
            );
            for button in &output.buttons {
                assert_eq!(button.icon, Some(canvas_widgets::ShellIcon::Window));
                assert!(button.icon_caption);
                assert!(button.enabled);
                assert!(button.x >= -4.0 && button.x + button.width <= 4.001);
                assert!(button.y <= 4.0 && button.y - button.height >= -4.001);
            }
        }
    }

    #[test]
    fn icon_primitives_are_bounded_and_do_not_grant_file_access() {
        let manifest = r#"{"schema_version":1,"id":"icon-test","title":"Icons","inputs":[],"capabilities":{"write_scene":true,"read_theme":true},"actions":[{"id":"select","destination":"canvas.shell","kind":"select"}]}"#;
        for (icon, size, valid) in [
            ("apps", 28, true),
            ("terminal", 48, true),
            ("/tmp/app.svg", 28, false),
            ("window", 999, false),
        ] {
            let source = format!(
                "widget.icon_button({{id='app',label='App',icon='{icon}',icon_size={size},action='select',body={{}},x=-1,y=1,width=2,height=1}})"
            );
            let mut p = Presentation::default();
            let result = p.refresh(manifest, &source, snapshot(), theme());
            assert_eq!(
                result.is_ok(),
                valid,
                "icon={icon}, size={size}: {result:?}"
            );
        }
    }

    #[test]
    fn pinned_apps_are_not_misrepresented_as_running_windows() {
        use crate::lua_module::ModuleInputValue as V;
        let mut s = snapshot();
        s.values = std::collections::BTreeMap::from([
            ("width".into(), V::Number(1408.0)),
            ("height".into(), V::Number(72.0)),
            ("count".into(), V::Number(1.0)),
            (
                "item0-id".into(),
                V::Text("app:/Applications/App.app".into()),
            ),
            ("item0-title".into(), V::Text("App".into())),
            ("item0-open".into(), V::Boolean(false)),
            ("item0-available".into(), V::Boolean(false)),
        ]);
        let mut p = Presentation::default();
        p.refresh(
            include_str!("../../../config/modules/desktop-dock-v1.json"),
            include_str!("../../../config/modules/desktop-dock-v1.lua"),
            s,
            theme(),
        )
        .unwrap();
        let output = p.retained().unwrap();
        assert!(
            !output
                .buttons
                .iter()
                .find(|b| b.id == "item0")
                .unwrap()
                .enabled
        );
        assert!(!output.scene.nodes.iter().any(|n| n.id == "item0-state"));
    }
}
