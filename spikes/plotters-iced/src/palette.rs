//! Field-for-field mirror of `canvas_core::ThemePalette` (crates/canvas-core/src/lib.rs:128).
//!
//! The spike copies the shape deliberately so that the mapping to the real type
//! is mechanical: `From<canvas_core::ThemePalette> for Palette` would be a
//! 13-line struct literal. Nothing here invents a colour.

use plotters::style::RGBColor;

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

/// The complete set of role names a script may reference. A script that names
/// anything else is rejected, loudly, at load time.
pub const ROLES: [&str; 13] = [
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

impl Palette {
    pub fn role(&self, name: &str) -> Option<[u8; 3]> {
        Some(match name {
            "background" => self.background,
            "background_strong" => self.background_strong,
            "surface" => self.surface,
            "surface_muted" => self.surface_muted,
            "text" => self.text,
            "text_muted" => self.text_muted,
            "primary" => self.primary,
            "success" => self.success,
            "warning" => self.warning,
            "danger" => self.danger,
            "info" => self.info,
            "border" => self.border,
            "border_strong" => self.border_strong,
            _ => return None,
        })
    }

    /// Resolve a validated role to a plotters colour. Panics only on a role that
    /// `PlotSpec` validation already proved present.
    pub fn rgb(&self, name: &str) -> RGBColor {
        let [r, g, b] = self
            .role(name)
            .unwrap_or_else(|| panic!("unvalidated palette role reached the renderer: {name}"));
        RGBColor(r, g, b)
    }
}

/// Stand-in for a Coherence dark token set. Values are arbitrary; the point is
/// that *every* mark below resolves through this struct and plotters' own
/// palette (`Palette99`, `RED`, `BLUE`, ...) is never referenced.
pub const DEMO_DARK: Palette = Palette {
    background: [0x0e, 0x11, 0x16],
    background_strong: [0x07, 0x09, 0x0c],
    surface: [0x16, 0x1b, 0x22],
    surface_muted: [0x1f, 0x25, 0x2d],
    text: [0xe6, 0xed, 0xf3],
    text_muted: [0x8b, 0x94, 0x9e],
    primary: [0x58, 0xa6, 0xff],
    success: [0x3f, 0xb9, 0x50],
    warning: [0xd2, 0x99, 0x22],
    danger: [0xf8, 0x51, 0x49],
    info: [0x79, 0xc0, 0xff],
    border: [0x30, 0x36, 0x3d],
    border_strong: [0x48, 0x4f, 0x58],
};
