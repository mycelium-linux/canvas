//! Reloadable, bounded visual languages. Theme packs are data, never CSS.

use canvas_core::ThemePalette;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

const MAX_PACK_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePack {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub palette: ThemePalette,
    #[serde(default)]
    pub effects: ThemeEffects,
}

#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeEffects {
    #[serde(default)]
    pub glow: f32,
    #[serde(default)]
    pub scanlines: f32,
    #[serde(default)]
    pub noise: f32,
    #[serde(default = "default_corner_style")]
    pub corner_style: String,
    #[serde(default = "default_density")]
    pub density: f32,
}

fn default_corner_style() -> String { "rounded".into() }
fn default_density() -> f32 { 1.0 }

impl Default for ThemeEffects {
    fn default() -> Self {
        Self { glow: 0.0, scanlines: 0.0, noise: 0.0, corner_style: default_corner_style(), density: 1.0 }
    }
}

impl ThemePack {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|error| format!("theme pack is not UTF-8: {error}"))?;
        let pack: Self = toml::from_str(text).map_err(|error| format!("invalid theme pack: {error}"))?;
        if pack.schema_version != 1 || !safe_id(&pack.id) {
            return Err("theme pack requires schema_version=1 and a safe id".into());
        }
        if pack.name.trim().is_empty() || pack.subtitle.trim().is_empty() {
            return Err("theme pack name and subtitle must be visible".into());
        }
        if !(0.0..=1.0).contains(&pack.effects.glow)
            || !(0.0..=1.0).contains(&pack.effects.scanlines)
            || !(0.0..=1.0).contains(&pack.effects.noise)
            || !(0.75..=1.5).contains(&pack.effects.density)
            || !matches!(pack.effects.corner_style.as_str(), "rounded" | "square" | "cut")
        {
            return Err("theme pack effects are outside their bounded ranges".into());
        }
        Ok(pack)
    }
}

pub fn load() -> (Vec<ThemePack>, Vec<String>) {
    let mut packs = BTreeMap::new();
    let mut notes = Vec::new();
    for bytes in [
        include_bytes!("../../../config/themes/jarvis.toml").as_slice(),
        include_bytes!("../../../config/themes/pip-boy.toml").as_slice(),
        include_bytes!("../../../config/themes/imperial.toml").as_slice(),
    ] {
        if let Ok(pack) = ThemePack::parse(bytes) { packs.insert(pack.id.clone(), pack); }
    }
    if let Some(root) = crate::runtime_config::root() {
        let directory = root.join("themes/packs");
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("toml") { continue; }
                match read_pack(&path) {
                    Ok(pack) => { packs.insert(pack.id.clone(), pack); }
                    Err(error) => notes.push(format!("{}: {error}", path.display())),
                }
            }
        }
    }
    (packs.into_values().collect(), notes)
}

pub fn fingerprint() -> String {
    format!("{:?}", load().0)
}

fn read_pack(path: &Path) -> Result<ThemePack, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_PACK_BYTES {
        return Err("theme pack must contain 1-65536 bytes".into());
    }
    ThemePack::parse(&std::fs::read(path).map_err(|error| error.to_string())?)
}

fn safe_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 48 && value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_personal_themes_are_valid_and_distinct() {
        let (packs, notes) = load();
        assert!(notes.is_empty());
        assert_eq!(packs.len(), 3);
        assert_ne!(packs[0].palette, packs[1].palette);
    }
}
