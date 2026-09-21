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
    #[serde(default)]
    pub typography: ThemeTypography,
    #[serde(default)]
    pub charts: ThemeCharts,
    #[serde(default)]
    pub chrome: ThemeChrome,
}

/// Window-manager visual language. These are presentation tokens only: hit
/// targets and window actions remain owned by Canvas, so a theme cannot make a
/// window impossible to operate.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeChrome {
    #[serde(default = "default_titlebar_height")]
    pub titlebar_height: f32,
    #[serde(default = "default_control_style")]
    pub control_style: String,
    #[serde(default = "default_title_alignment")]
    pub title_alignment: String,
    #[serde(default = "default_border_width")]
    pub border_width: f32,
    #[serde(default = "default_border_radius")]
    pub border_radius: f32,
    #[serde(default = "default_shadow_blur")]
    pub shadow_blur: f32,
    #[serde(default = "default_shadow_offset")]
    pub shadow_offset: f32,
    #[serde(default = "default_shadow_opacity")]
    pub shadow_opacity: f32,
    #[serde(default = "default_header_opacity")]
    pub header_opacity: f32,
    #[serde(default)]
    pub uppercase_titles: bool,
    #[serde(default = "default_focus_edge")]
    pub focus_edge: String,
}

fn default_titlebar_height() -> f32 {
    26.0
}
fn default_control_style() -> String {
    "dots".into()
}
fn default_title_alignment() -> String {
    "left".into()
}
fn default_border_width() -> f32 {
    1.0
}
fn default_border_radius() -> f32 {
    8.0
}
fn default_shadow_blur() -> f32 {
    18.0
}
fn default_shadow_offset() -> f32 {
    5.0
}
fn default_shadow_opacity() -> f32 {
    0.28
}
fn default_header_opacity() -> f32 {
    0.82
}
fn default_focus_edge() -> String {
    "border".into()
}

impl Default for ThemeChrome {
    fn default() -> Self {
        Self {
            titlebar_height: default_titlebar_height(),
            control_style: default_control_style(),
            title_alignment: default_title_alignment(),
            border_width: default_border_width(),
            border_radius: default_border_radius(),
            shadow_blur: default_shadow_blur(),
            shadow_offset: default_shadow_offset(),
            shadow_opacity: default_shadow_opacity(),
            header_opacity: default_header_opacity(),
            uppercase_titles: false,
            focus_edge: default_focus_edge(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeCharts {
    /// Six categorical identities used by charts, in display order. Empty
    /// means derive a perceptually-separated ramp from the theme primary.
    #[serde(default, with = "chart_colors")]
    pub categorical: Vec<[u8; 3]>,
}

mod chart_colors {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(colors: &Vec<[u8; 3]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let values = colors
            .iter()
            .map(|rgb| format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]))
            .collect::<Vec<_>>();
        serde::Serialize::serialize(&values, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<[u8; 3]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<String>::deserialize(deserializer)?;
        values
            .into_iter()
            .map(|value| parse(&value).map_err(serde::de::Error::custom))
            .collect()
    }

    fn parse(value: &str) -> Result<[u8; 3], String> {
        let hex = value
            .strip_prefix('#')
            .ok_or_else(|| "chart colors must use #rrggbb".to_string())?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("chart colors must use #rrggbb".into());
        }
        Ok([
            u8::from_str_radix(&hex[0..2], 16).map_err(|error| error.to_string())?,
            u8::from_str_radix(&hex[2..4], 16).map_err(|error| error.to_string())?,
            u8::from_str_radix(&hex[4..6], 16).map_err(|error| error.to_string())?,
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeTypography {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_mono_family")]
    pub mono_family: String,
    #[serde(default)]
    pub letter_spacing: f32,
}

fn default_font_family() -> String {
    "system-ui, sans-serif".into()
}
fn default_mono_family() -> String {
    "ui-monospace, monospace".into()
}

impl Default for ThemeTypography {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            mono_family: default_mono_family(),
            letter_spacing: 0.0,
        }
    }
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

fn default_corner_style() -> String {
    "rounded".into()
}
fn default_density() -> f32 {
    1.0
}

impl Default for ThemeEffects {
    fn default() -> Self {
        Self {
            glow: 0.0,
            scanlines: 0.0,
            noise: 0.0,
            corner_style: default_corner_style(),
            density: 1.0,
        }
    }
}

impl ThemePack {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("theme pack is not UTF-8: {error}"))?;
        let pack: Self =
            toml::from_str(text).map_err(|error| format!("invalid theme pack: {error}"))?;
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
            || !matches!(
                pack.effects.corner_style.as_str(),
                "rounded" | "square" | "cut"
            )
        {
            return Err("theme pack effects are outside their bounded ranges".into());
        }
        if pack.typography.font_family.len() > 160
            || pack.typography.mono_family.len() > 160
            || pack.typography.font_family.chars().any(char::is_control)
            || pack.typography.mono_family.chars().any(char::is_control)
            || !(-1.0..=3.0).contains(&pack.typography.letter_spacing)
        {
            return Err("theme pack typography is outside its bounded ranges".into());
        }
        if !pack.charts.categorical.is_empty() && pack.charts.categorical.len() != 6 {
            return Err("theme pack chart palette must contain exactly six colors".into());
        }
        if !(22.0..=40.0).contains(&pack.chrome.titlebar_height)
            || !matches!(
                pack.chrome.control_style.as_str(),
                "dots" | "glyphs" | "segments"
            )
            || !matches!(pack.chrome.title_alignment.as_str(), "left" | "center")
            || !(0.5..=4.0).contains(&pack.chrome.border_width)
            || !(0.0..=20.0).contains(&pack.chrome.border_radius)
            || !(0.0..=40.0).contains(&pack.chrome.shadow_blur)
            || !(0.0..=16.0).contains(&pack.chrome.shadow_offset)
            || !(0.0..=0.8).contains(&pack.chrome.shadow_opacity)
            || !(0.4..=1.0).contains(&pack.chrome.header_opacity)
            || !matches!(pack.chrome.focus_edge.as_str(), "border" | "top" | "left")
        {
            return Err("theme pack chrome is outside its bounded ranges".into());
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
        if let Ok(pack) = ThemePack::parse(bytes) {
            packs.insert(pack.id.clone(), pack);
        }
    }
    if let Some(root) = crate::runtime_config::root() {
        let directory = root.join("themes/packs");
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("toml") {
                    continue;
                }
                match read_pack(&path) {
                    Ok(pack) => {
                        packs.insert(pack.id.clone(), pack);
                    }
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
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_personal_themes_are_valid_and_distinct() {
        let packs = [
            ThemePack::parse(include_bytes!("../../../config/themes/jarvis.toml")),
            ThemePack::parse(include_bytes!("../../../config/themes/pip-boy.toml")),
            ThemePack::parse(include_bytes!("../../../config/themes/imperial.toml")),
        ]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("bundled theme packs");
        assert_eq!(packs.len(), 3);
        assert_ne!(packs[0].palette, packs[1].palette);
        assert!(packs.iter().all(|pack| pack.charts.categorical.len() == 6));
        assert_ne!(packs[0].charts.categorical, packs[1].charts.categorical);
        assert_ne!(packs[0].chrome, packs[1].chrome);
        assert_ne!(packs[1].chrome, packs[2].chrome);
        assert_eq!(packs[0].chrome.control_style, "glyphs");
        assert_eq!(packs[1].chrome.control_style, "segments");
        assert_eq!(packs[2].chrome.border_radius, 0.0);
    }
}
