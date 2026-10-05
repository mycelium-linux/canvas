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
}

pub const DESKTOP_BAR: Artifact = Artifact {
    manifest_path: "modules/desktop-bar-v1.json",
    manifest_fallback: include_str!("../../../config/modules/desktop-bar-v1.json"),
    source_path: "modules/desktop-bar-v1.lua",
    source_fallback: include_str!("../../../config/modules/desktop-bar-v1.lua"),
};
pub const DESKTOP_LAUNCHER: Artifact = Artifact {
    manifest_path: "modules/desktop-launcher-v1.json",
    manifest_fallback: include_str!("../../../config/modules/desktop-launcher-v1.json"),
    source_path: "modules/desktop-launcher-v1.lua",
    source_fallback: include_str!("../../../config/modules/desktop-launcher-v1.lua"),
};

#[derive(Debug, Clone, Default)]
pub struct Presentation {
    key: Option<[u8; 32]>,
    retained: Option<RetainedModuleScene>,
    current: Option<(std::sync::Arc<str>, std::sync::Arc<str>)>,
}

impl Presentation {
    pub fn retained(&self) -> Option<&RetainedModuleScene> {
        self.retained.as_ref()
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
        let read = |path: &str, fallback: &str| -> crate::Result<String> {
            crate::runtime_config::read(std::path::Path::new(path), 32 * 1024)
                .map_err(|e| crate::Error(e.to_string()))?
                .map(|bytes| String::from_utf8(bytes).map_err(|e| crate::Error(e.to_string())))
                .transpose()
                .map(|value| value.unwrap_or_else(|| fallback.into()))
        };
        self.refresh(
            &read(artifact.manifest_path, artifact.manifest_fallback)?,
            &read(artifact.source_path, artifact.source_fallback)?,
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
                if b.id == "back" || b.id == "create" {
                    assert!(!b.enabled);
                }
            }
            let encoded = serde_json::to_string(&output.scene).unwrap();
            assert!(encoded.contains("SOURCES") == (width >= 750.0));
            assert!(!encoded.contains("LIVE"));
            assert!(encoded.contains("\"opacity\":1.0"));
        }
    }
}
