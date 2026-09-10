//! An **externally supplied** palette.
//!
//! This mirrors the *shape* of `canvas_core::ThemePalette` (one RGB byte
//! triple per semantic role) without depending on the crate, so the spike
//! stays standalone. On adoption this whole file is deleted and
//! `canvas_core::ThemePalette` / `ThemeMetrics` are used directly — the point
//! of the spike is that plotters never sees a colour it did not receive.

use plotters::style::{RGBAColor, RGBColor};

/// Field-for-field the roles in `canvas_core::ThemePalette`
/// (crates/canvas-core/src/lib.rs:128).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: [u8; 3],
    pub background_strong: [u8; 3],
    pub surface: [u8; 3],
    pub surface_muted: [u8; 3],
    pub text: [u8; 3],
    pub text_muted: [u8; 3],
    pub primary: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub danger: [u8; 3],
    pub info: [u8; 3],
    pub border: [u8; 3],
    pub border_strong: [u8; 3],
}

/// The only names a Lua plot may use for a colour. There is deliberately no
/// escape hatch to a literal hex value: a script that wants a colour Coherence
/// does not define is a script the theme cannot re-skin, which is the
/// regression we are guarding against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Background,
    BackgroundStrong,
    Surface,
    SurfaceMuted,
    Text,
    TextMuted,
    Primary,
    Success,
    Warning,
    Danger,
    Info,
    Border,
    BorderStrong,
}

impl Role {
    pub const NAMES: &'static [&'static str] = &[
        "background",
        "background_strong",
        "surface",
        "surface_muted",
        "text",
        "text_muted",
        "primary",
        "success",
        "warning",
        "danger",
        "info",
        "border",
        "border_strong",
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "background" => Self::Background,
            "background_strong" => Self::BackgroundStrong,
            "surface" => Self::Surface,
            "surface_muted" => Self::SurfaceMuted,
            "text" => Self::Text,
            "text_muted" => Self::TextMuted,
            "primary" => Self::Primary,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "danger" => Self::Danger,
            "info" => Self::Info,
            "border" => Self::Border,
            "border_strong" => Self::BorderStrong,
            _ => return None,
        })
    }
}

impl Palette {
    pub fn rgb(&self, role: Role) -> [u8; 3] {
        match role {
            Role::Background => self.background,
            Role::BackgroundStrong => self.background_strong,
            Role::Surface => self.surface,
            Role::SurfaceMuted => self.surface_muted,
            Role::Text => self.text,
            Role::TextMuted => self.text_muted,
            Role::Primary => self.primary,
            Role::Success => self.success,
            Role::Warning => self.warning,
            Role::Danger => self.danger,
            Role::Info => self.info,
            Role::Border => self.border,
            Role::BorderStrong => self.border_strong,
        }
    }

    pub fn color(&self, role: Role) -> RGBColor {
        let [r, g, b] = self.rgb(role);
        RGBColor(r, g, b)
    }

    /// Palette colour at partial opacity — how gridlines stay subordinate
    /// without introducing an off-palette grey.
    pub fn faded(&self, role: Role, alpha: f64) -> RGBAColor {
        let [r, g, b] = self.rgb(role);
        RGBAColor(r, g, b, alpha)
    }

    pub fn iced(&self, role: Role) -> iced::Color {
        let [r, g, b] = self.rgb(role);
        iced::Color::from_rgb8(r, g, b)
    }
}

/// The subset of `canvas_core::ThemeMetrics` a chart needs. `micro` is the
/// documented floor for labels a renderer draws itself and cannot reflow —
/// exactly what axis ticks are (crates/canvas-core/src/lib.rs:215).
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub space_3: f32,
    pub micro: f32,
    pub small: f32,
    pub body: f32,
    pub title: f32,
}

/// Stand-in for a Coherence dark theme. Values are placeholders; adoption
/// resolves them from the real token crate.
pub const DARK: Palette = Palette {
    background: [0x12, 0x14, 0x18],
    background_strong: [0x0b, 0x0d, 0x10],
    surface: [0x1b, 0x1f, 0x26],
    surface_muted: [0x24, 0x29, 0x32],
    text: [0xe6, 0xe9, 0xef],
    text_muted: [0x8b, 0x93, 0xa3],
    primary: [0x5c, 0x9d, 0xff],
    success: [0x4c, 0xc9, 0x8a],
    warning: [0xe8, 0xb3, 0x39],
    danger: [0xe5, 0x64, 0x5f],
    info: [0x7d, 0xd3, 0xd8],
    border: [0x2e, 0x34, 0x3f],
    border_strong: [0x44, 0x4c, 0x5a],
};

pub const METRICS: Metrics = Metrics {
    space_3: 12.0,
    micro: 11.0,
    small: 13.0,
    body: 15.0,
    title: 20.0,
};
