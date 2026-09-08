//! First-class palette import: the Dock theme file format plus adapters for
//! common rice palette formats. Every adapter maps source colors onto the
//! canonical [ThemePalette] roles explicitly; roles a source lacks are
//! derived by transformation, never configured, and anything unparseable
//! fails loudly.

use crate::{ThemePalette, theme_hex};
use serde::Deserialize;
use std::path::Path;
use std::str::FromStr;

const MAX_THEME_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeFormat {
    /// Canonical Dock theme file: all 13 roles, hex strings.
    Dock,
    Catppuccin,
    TokyoNight,
    Nord,
    Gruvbox,
}

impl FromStr for ThemeFormat {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "dock" => Ok(Self::Dock),
            "catppuccin" => Ok(Self::Catppuccin),
            "tokyo-night" => Ok(Self::TokyoNight),
            "nord" => Ok(Self::Nord),
            "gruvbox" => Ok(Self::Gruvbox),
            _ => Err(format!(
                "unknown theme format {value}; expected dock, catppuccin, tokyo-night, nord, or gruvbox"
            )),
        }
    }
}

pub fn import_file(format: ThemeFormat, path: &Path) -> Result<ThemePalette, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| format!("cannot read theme file {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_THEME_FILE_BYTES {
        return Err("theme file must be a regular file of at most 64 KiB".into());
    }
    let bytes =
        std::fs::read(path).map_err(|_| format!("cannot read theme file {}", path.display()))?;
    parse(format, &bytes)
}

pub fn parse(format: ThemeFormat, bytes: &[u8]) -> Result<ThemePalette, String> {
    match format {
        ThemeFormat::Dock => parse_dock(bytes),
        ThemeFormat::Catppuccin => parse_catppuccin(bytes),
        ThemeFormat::TokyoNight => parse_tokyo_night(bytes),
        ThemeFormat::Nord => parse_nord(bytes),
        ThemeFormat::Gruvbox => parse_gruvbox(bytes),
    }
}

pub fn to_dock_json(palette: &ThemePalette) -> String {
    serde_json::to_string(palette).expect("canonical palette always serializes")
}

fn parse_dock(bytes: &[u8]) -> Result<ThemePalette, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid Dock theme file: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatppuccinFile {
    name: String,
    #[serde(rename = "type")]
    scheme: String,
    colors: CatppuccinColors,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "unused source roles are still validated strictly")]
struct CatppuccinColors {
    #[serde(with = "theme_hex")]
    pub rosewater: [u8; 3],
    #[serde(with = "theme_hex")]
    pub flamingo: [u8; 3],
    #[serde(with = "theme_hex")]
    pub pink: [u8; 3],
    #[serde(with = "theme_hex")]
    pub mauve: [u8; 3],
    #[serde(with = "theme_hex")]
    pub red: [u8; 3],
    #[serde(with = "theme_hex")]
    pub maroon: [u8; 3],
    #[serde(with = "theme_hex")]
    pub peach: [u8; 3],
    #[serde(with = "theme_hex")]
    pub yellow: [u8; 3],
    #[serde(with = "theme_hex")]
    pub green: [u8; 3],
    #[serde(with = "theme_hex")]
    pub teal: [u8; 3],
    #[serde(with = "theme_hex")]
    pub sky: [u8; 3],
    #[serde(with = "theme_hex")]
    pub sapphire: [u8; 3],
    #[serde(with = "theme_hex")]
    pub blue: [u8; 3],
    #[serde(with = "theme_hex")]
    pub lavender: [u8; 3],
    #[serde(with = "theme_hex")]
    pub text: [u8; 3],
    #[serde(with = "theme_hex")]
    pub subtext1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub subtext0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub overlay2: [u8; 3],
    #[serde(with = "theme_hex")]
    pub overlay1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub overlay0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub surface2: [u8; 3],
    #[serde(with = "theme_hex")]
    pub surface1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub surface0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub base: [u8; 3],
    #[serde(with = "theme_hex")]
    pub mantle: [u8; 3],
    #[serde(with = "theme_hex")]
    pub crust: [u8; 3],
}
fn parse_catppuccin(bytes: &[u8]) -> Result<ThemePalette, String> {
    let file: CatppuccinFile =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid catppuccin palette: {e}"))?;
    if file.name.trim().is_empty() {
        return Err("catppuccin palette must name its flavor".into());
    }
    let light = file.scheme.eq_ignore_ascii_case("light");
    let c = file.colors;
    Ok(ThemePalette {
        background: c.base,
        // The darker layer: crust in dark schemes, mantle in light schemes.
        background_strong: if light { c.mantle } else { c.crust },
        surface: c.surface0,
        surface_muted: c.surface1,
        text: c.text,
        text_muted: c.subtext0,
        primary: c.mauve,
        success: c.green,
        warning: c.yellow,
        danger: c.red,
        info: c.blue,
        border: c.surface1,
        border_strong: c.surface2,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokyoNightFile {
    name: String,
    colors: TokyoNightColors,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "unused source roles are still validated strictly")]
struct TokyoNightColors {
    #[serde(with = "theme_hex")]
    pub bg: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg_dark: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg_light: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg: [u8; 3],
    #[serde(with = "theme_hex")]
    pub comment: [u8; 3],
    #[serde(with = "theme_hex")]
    pub red: [u8; 3],
    #[serde(with = "theme_hex")]
    pub orange: [u8; 3],
    #[serde(with = "theme_hex")]
    pub yellow: [u8; 3],
    #[serde(with = "theme_hex")]
    pub green: [u8; 3],
    #[serde(with = "theme_hex")]
    pub cyan: [u8; 3],
    #[serde(with = "theme_hex")]
    pub blue: [u8; 3],
    #[serde(with = "theme_hex")]
    pub magenta: [u8; 3],
    #[serde(with = "theme_hex")]
    pub purple: [u8; 3],
}

fn parse_tokyo_night(bytes: &[u8]) -> Result<ThemePalette, String> {
    let file: TokyoNightFile =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid tokyo-night palette: {e}"))?;
    if file.name.trim().is_empty() {
        return Err("tokyo-night palette must name its variant".into());
    }
    let c = file.colors;
    Ok(ThemePalette {
        background: c.bg,
        background_strong: c.bg_dark,
        surface: c.bg_light,
        surface_muted: mix(c.bg_light, c.bg, 0.5),
        text: c.fg,
        text_muted: c.comment,
        primary: c.blue,
        success: c.green,
        warning: c.yellow,
        danger: c.red,
        info: c.cyan,
        border: mix(c.bg, c.fg, 0.18),
        border_strong: mix(c.bg, c.fg, 0.34),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "unused source roles are still validated strictly")]
struct NordColors {
    #[serde(with = "theme_hex")]
    pub nord0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord2: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord3: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord4: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord5: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord6: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord7: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord8: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord9: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord10: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord11: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord12: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord13: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord14: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord15: [u8; 3],
    #[serde(with = "theme_hex")]
    pub nord16: [u8; 3],
}

fn parse_nord(bytes: &[u8]) -> Result<ThemePalette, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "nord palette must be UTF-8".to_string())?;
    let c: NordColors =
        toml::de::from_str(text).map_err(|e| format!("invalid nord palette: {e}"))?;
    Ok(ThemePalette {
        background: c.nord0,
        background_strong: shade(c.nord0, 0.16),
        surface: c.nord1,
        surface_muted: c.nord2,
        text: c.nord4,
        text_muted: c.nord3,
        primary: c.nord14,
        success: c.nord11,
        warning: c.nord10,
        danger: c.nord8,
        info: c.nord15,
        border: c.nord2,
        border_strong: c.nord3,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "unused source roles are still validated strictly")]
struct GruvboxColors {
    #[serde(with = "theme_hex")]
    pub bg0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg0_h: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg2: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg3: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bg4: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg0: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg1: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg2: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg3: [u8; 3],
    #[serde(with = "theme_hex")]
    pub fg4: [u8; 3],
    #[serde(with = "theme_hex")]
    pub red: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_red: [u8; 3],
    #[serde(with = "theme_hex")]
    pub green: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_green: [u8; 3],
    #[serde(with = "theme_hex")]
    pub yellow: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_yellow: [u8; 3],
    #[serde(with = "theme_hex")]
    pub blue: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_blue: [u8; 3],
    #[serde(with = "theme_hex")]
    pub purple: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_purple: [u8; 3],
    #[serde(with = "theme_hex")]
    pub aqua: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_aqua: [u8; 3],
    #[serde(with = "theme_hex")]
    pub orange: [u8; 3],
    #[serde(with = "theme_hex")]
    pub bright_orange: [u8; 3],
}

fn parse_gruvbox(bytes: &[u8]) -> Result<ThemePalette, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "gruvbox palette must be UTF-8".to_string())?;
    let c: GruvboxColors =
        toml::de::from_str(text).map_err(|e| format!("invalid gruvbox palette: {e}"))?;
    Ok(ThemePalette {
        background: c.bg0,
        background_strong: c.bg0_h,
        surface: c.bg1,
        surface_muted: c.bg2,
        text: c.fg0,
        text_muted: c.fg3,
        primary: c.orange,
        success: c.green,
        warning: c.yellow,
        danger: c.red,
        info: c.blue,
        border: c.bg2,
        border_strong: c.bg3,
    })
}

/// Linear per-channel mix: `t` is the weight of `b` (0.0 keeps `a`).
pub(crate) fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t).round() as u8)
}

/// Shade toward black.
pub(crate) fn shade(a: [u8; 3], t: f32) -> [u8; 3] {
    mix(a, [0; 3], t)
}

/// Tint toward white.
#[cfg(test)]
pub(crate) fn tint(a: [u8; 3], t: f32) -> [u8; 3] {
    mix(a, [255; 3], t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCHA: &str = r##"{
        "name": "Mocha",
        "type": "Dark",
        "colors": {
            "rosewater": "#f5e0dc", "flamingo": "#f2cdcd", "pink": "#f5c2e7",
            "mauve": "#cba6f7", "red": "#f38ba8", "maroon": "#eba0ac",
            "peach": "#fab387", "yellow": "#f9e2af", "green": "#a6e3a1",
            "teal": "#94e2d5", "sky": "#89dceb", "sapphire": "#74c7ec",
            "blue": "#89b4fa", "lavender": "#b4befe",
            "text": "#cdd6f4", "subtext1": "#bac2de", "subtext0": "#a6adc8",
            "overlay2": "#9399b2", "overlay1": "#7f849c", "overlay0": "#6c7086",
            "surface2": "#585b70", "surface1": "#45475a", "surface0": "#313244",
            "base": "#1e1e2e", "mantle": "#181825", "crust": "#11111b"
        }
    }"##;

    const LATTE: &str = r##"{
        "name": "Latte",
        "type": "Light",
        "colors": {
            "rosewater": "#dc8a78", "flamingo": "#dd7878", "pink": "#ea76cb",
            "mauve": "#8839ef", "red": "#d20f39", "maroon": "#e64553",
            "peach": "#fe640b", "yellow": "#df8e1d", "green": "#40a02b",
            "teal": "#179299", "sky": "#04a5e5", "sapphire": "#209fb5",
            "blue": "#1e66f5", "lavender": "#7287fd",
            "text": "#4c4f69", "subtext1": "#5c5f77", "subtext0": "#6c6f85",
            "overlay2": "#7c7f93", "overlay1": "#8c8fa1", "overlay0": "#9ca0b0",
            "surface2": "#acb0be", "surface1": "#bcc0cc", "surface0": "#ccd0da",
            "base": "#eff1f5", "mantle": "#e6e9ef", "crust": "#dce0e8"
        }
    }"##;

    const TOKYO: &str = r##"{
        "name": "Tokyo Night",
        "colors": {
            "bg": "#1a1b26", "bg_dark": "#16161e", "bg_light": "#292e42",
            "fg": "#c0caf5", "comment": "#565f89",
            "red": "#f7768e", "orange": "#ff9e64", "yellow": "#e0af68",
            "green": "#9ece6a", "cyan": "#7dcfff", "blue": "#7aa2f7",
            "magenta": "#bb9af7", "purple": "#9d7cd8"
        }
    }"##;

    const NORD: &str = r##"
        nord0 = "#2e3440"
        nord1 = "#3b4252"
        nord2 = "#434c5e"
        nord3 = "#4c566a"
        nord4 = "#d8dee9"
        nord5 = "#e5e9f0"
        nord6 = "#eceff4"
        nord7 = "#8fbcbb"
        nord8 = "#bf616a"
        nord9 = "#d08770"
        nord10 = "#ebcb8b"
        nord11 = "#a3be8c"
        nord12 = "#8fa173"
        nord13 = "#b48ead"
        nord14 = "#88c0d0"
        nord15 = "#81a1c1"
        nord16 = "#5e81ac"
    "##;

    const GRUVBOX: &str = r##"
        bg0 = "#282828"
        bg0_h = "#1d2021"
        bg1 = "#3c3836"
        bg2 = "#504945"
        bg3 = "#665c54"
        bg4 = "#7c6f64"
        fg0 = "#fbf1c7"
        fg1 = "#ebdbb2"
        fg2 = "#d5c4a1"
        fg3 = "#bdae93"
        fg4 = "#a89984"
        red = "#cc241d"
        bright_red = "#fb4934"
        green = "#98971a"
        bright_green = "#b8bb26"
        yellow = "#d79921"
        bright_yellow = "#fabd2f"
        blue = "#458588"
        bright_blue = "#83a598"
        purple = "#b16286"
        bright_purple = "#d3869b"
        aqua = "#689d6a"
        bright_aqua = "#8ec07c"
        orange = "#d65d0e"
        bright_orange = "#fe8019"
    "##;

    #[test]
    fn dock_format_round_trips_all_roles() {
        let palette = parse(
            ThemeFormat::Dock,
            br##"{
            "background": "#101014", "background_strong": "#0a0a0e",
            "surface": "#1a1a20", "surface_muted": "#22222a",
            "text": "#e8e8ec", "text_muted": "#8a8a94",
            "primary": "#50a0ff", "success": "#50c878", "warning": "#ffbe54",
            "danger": "#ff5a5a", "info": "#4fc3e8",
            "border": "#2a2a32", "border_strong": "#3a3a44"
        }"##,
        )
        .unwrap();
        assert_eq!(palette.background, [0x10, 0x10, 0x14]);
        assert_eq!(palette.border_strong, [0x3a, 0x3a, 0x44]);
        let json = to_dock_json(&palette);
        assert!(json.contains("\"background\":\"#101014\""));
        assert_eq!(parse(ThemeFormat::Dock, json.as_bytes()).unwrap(), palette);
    }

    #[test]
    fn dock_format_rejects_unknown_roles_and_bad_hex() {
        assert!(
            parse(
                ThemeFormat::Dock,
                br##"{"background": "#101014", "surprise": "#000000"}"##,
            )
            .is_err()
        );
        assert!(parse(
            ThemeFormat::Dock,
            br##"{"background": "101014", "background_strong": "#000000", "surface": "#000000", "surface_muted": "#000000", "text": "#000000", "text_muted": "#000000", "primary": "#000000", "success": "#000000", "warning": "#000000", "danger": "#000000", "info": "#000000", "border": "#000000", "border_strong": "#000000"}"##,
        )
        .is_err());
        let mut partial = String::from(
            r##"{"background": "#101014", "background_strong": "#000000", "surface": "#000000", "surface_muted": "#000000", "text": "#000000", "text_muted": "#000000", "primary": "#000000", "success": "#000000", "warning": "#000000", "danger": "#000000", "info": "#000000", "border": "#000000""##,
        );
        partial.push('}');
        assert!(parse(ThemeFormat::Dock, partial.as_bytes()).is_err());
    }

    #[test]
    fn catppuccin_maps_roles_and_switches_darker_layer_by_scheme() {
        let mocha = parse(ThemeFormat::Catppuccin, MOCHA.as_bytes()).unwrap();
        assert_eq!(mocha.background, [0x1e, 0x1e, 0x2e]);
        assert_eq!(mocha.background_strong, [0x11, 0x11, 0x1b]);
        assert_eq!(mocha.surface, [0x31, 0x32, 0x44]);
        assert_eq!(mocha.text, [0xcd, 0xd6, 0xf4]);
        assert_eq!(mocha.text_muted, [0xa6, 0xad, 0xc8]);
        assert_eq!(mocha.primary, [0xcb, 0xa6, 0xf7]);
        assert_eq!(mocha.success, [0xa6, 0xe3, 0xa1]);
        assert_eq!(mocha.warning, [0xf9, 0xe2, 0xaf]);
        assert_eq!(mocha.danger, [0xf3, 0x8b, 0xa8]);
        assert_eq!(mocha.info, [0x89, 0xb4, 0xfa]);
        assert_eq!(mocha.border, [0x45, 0x47, 0x5a]);
        assert_eq!(mocha.border_strong, [0x58, 0x5b, 0x70]);

        let latte = parse(ThemeFormat::Catppuccin, LATTE.as_bytes()).unwrap();
        assert_eq!(latte.background, [0xef, 0xf1, 0xf5]);
        assert_eq!(latte.background_strong, [0xe6, 0xe9, 0xef]);
        assert_eq!(latte.primary, [0x88, 0x39, 0xef]);
    }

    #[test]
    fn tokyo_night_derives_its_missing_layers() {
        let palette = parse(ThemeFormat::TokyoNight, TOKYO.as_bytes()).unwrap();
        assert_eq!(palette.background, [0x1a, 0x1b, 0x26]);
        assert_eq!(palette.background_strong, [0x16, 0x16, 0x1e]);
        assert_eq!(palette.surface, [0x29, 0x2e, 0x42]);
        assert_eq!(
            palette.surface_muted,
            mix([0x29, 0x2e, 0x42], [0x1a, 0x1b, 0x26], 0.5)
        );
        assert_eq!(palette.text, [0xc0, 0xca, 0xf5]);
        assert_eq!(palette.primary, [0x7a, 0xa2, 0xf7]);
        assert_eq!(palette.info, [0x7d, 0xcf, 0xff]);
        assert_eq!(
            palette.border,
            mix([0x1a, 0x1b, 0x26], [0xc0, 0xca, 0xf5], 0.18)
        );
        assert_eq!(
            palette.border_strong,
            mix([0x1a, 0x1b, 0x26], [0xc0, 0xca, 0xf5], 0.34)
        );
    }

    #[test]
    fn nord_maps_frost_semantics_and_derives_its_dark_layer() {
        let palette = parse(ThemeFormat::Nord, NORD.as_bytes()).unwrap();
        assert_eq!(palette.background, [0x2e, 0x34, 0x40]);
        assert_eq!(palette.background_strong, shade([0x2e, 0x34, 0x40], 0.16));
        assert_eq!(palette.surface, [0x3b, 0x42, 0x52]);
        assert_eq!(palette.text, [0xd8, 0xde, 0xe9]);
        assert_eq!(palette.primary, [0x88, 0xc0, 0xd0]);
        assert_eq!(palette.success, [0xa3, 0xbe, 0x8c]);
        assert_eq!(palette.warning, [0xeb, 0xcb, 0x8b]);
        assert_eq!(palette.danger, [0xbf, 0x61, 0x6a]);
        assert_eq!(palette.info, [0x81, 0xa1, 0xc1]);
    }

    #[test]
    fn gruvbox_maps_its_canonical_roles() {
        let palette = parse(ThemeFormat::Gruvbox, GRUVBOX.as_bytes()).unwrap();
        assert_eq!(palette.background, [0x28, 0x28, 0x28]);
        assert_eq!(palette.background_strong, [0x1d, 0x20, 0x21]);
        assert_eq!(palette.surface, [0x3c, 0x38, 0x36]);
        assert_eq!(palette.surface_muted, [0x50, 0x49, 0x45]);
        assert_eq!(palette.text, [0xfb, 0xf1, 0xc7]);
        assert_eq!(palette.text_muted, [0xbd, 0xae, 0x93]);
        assert_eq!(palette.primary, [0xd6, 0x5d, 0x0e]);
        assert_eq!(palette.success, [0x98, 0x97, 0x1a]);
        assert_eq!(palette.warning, [0xd7, 0x99, 0x21]);
        assert_eq!(palette.danger, [0xcc, 0x24, 0x1d]);
        assert_eq!(palette.info, [0x45, 0x85, 0x88]);
        assert_eq!(palette.border, [0x50, 0x49, 0x45]);
        assert_eq!(palette.border_strong, [0x66, 0x5c, 0x54]);
    }

    #[test]
    fn adapters_fail_loud_on_shape_mismatches() {
        assert!(
            parse(
                ThemeFormat::Catppuccin,
                br##"{"name": "Mocha", "type": "Dark", "colors": {"base": "#1e1e2e"}}"##,
            )
            .is_err()
        );
        assert!(parse(ThemeFormat::Nord, GRUVBOX.as_bytes(),).is_err());
        assert!(parse(ThemeFormat::TokyoNight, NORD.as_bytes()).is_err());
        let mut extra_role = GRUVBOX.as_bytes().to_vec();
        extra_role.extend_from_slice(b"surprise = \"#000000\"\n");
        assert!(parse(ThemeFormat::Gruvbox, &extra_role).is_err());
    }

    #[test]
    fn derivation_transforms_are_deterministic_and_bounded() {
        let a = [0x10, 0x20, 0x30];
        assert_eq!(mix(a, a, 0.5), a);
        assert_eq!(mix([0; 3], [255; 3], 1.0), [255; 3]);
        assert_eq!(shade([255; 3], 1.0), [0; 3]);
        assert_eq!(tint([0; 3], 1.0), [255; 3]);
        assert_eq!(mix(a, [255; 3], 2.0), [255; 3]);
        assert_eq!(mix(a, [255; 3], -1.0), a);
    }

    #[test]
    fn hex_codec_rejects_non_hex_and_short_values() {
        assert_eq!(theme_hex::parse_hex("#1a2b3c").unwrap(), [0x1a, 0x2b, 0x3c]);
        assert!(theme_hex::parse_hex("1a2b3c").is_err());
        assert!(theme_hex::parse_hex("#1a2b3").is_err());
        assert!(theme_hex::parse_hex("#1a2b3g").is_err());
        assert!(theme_hex::parse_hex("").is_err());
    }
}
