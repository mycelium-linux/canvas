//! A stand-in for `canvas_core::VisualTheme` / `ThemePalette`.
//!
//! The spike is standalone by instruction, so this mirrors the shape of the
//! real thing (13 color tokens + metrics) without depending on canvas-core.
//! The field names match `crates/canvas-core/src/lib.rs:128-168` so the
//! mapping below transfers verbatim.

use iced::{Color, Theme};

#[derive(Clone, Copy, Debug)]
pub struct CoherenceLike {
    pub background: Color,
    pub background_strong: Color,
    pub surface: Color,
    pub surface_muted: Color,
    pub text: Color,
    pub text_muted: Color,
    pub primary: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub border: Color,
    pub border_strong: Color,

    pub radius_sm: f32,
    pub radius_md: f32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

impl CoherenceLike {
    pub const fn dark() -> Self {
        Self {
            background: rgb(0x12, 0x14, 0x18),
            background_strong: rgb(0x0b, 0x0d, 0x10),
            surface: rgb(0x1b, 0x1f, 0x25),
            surface_muted: rgb(0x24, 0x29, 0x31),
            text: rgb(0xe6, 0xea, 0xf0),
            text_muted: rgb(0x9a, 0xa4, 0xb2),
            primary: rgb(0x5b, 0x9d, 0xf5),
            success: rgb(0x4c, 0xc3, 0x8a),
            warning: rgb(0xe0, 0xa3, 0x3f),
            danger: rgb(0xe0, 0x5c, 0x5c),
            info: rgb(0x6f, 0xb8, 0xd8),
            border: rgb(0x30, 0x36, 0x40),
            border_strong: rgb(0x44, 0x4c, 0x58),
            radius_sm: 4.0,
            radius_md: 8.0,
        }
    }

    /// Exactly what Canvas already does at `crates/canvas/src/main.rs:4385`.
    ///
    /// Six of the thirteen tokens fit. `surface`, `surface_muted`,
    /// `background_strong`, `text_muted`, `info`, `border` and `border_strong`
    /// have no slot; iced *derives* substitutes for them. That is why Canvas's
    /// own widgets pass tokens through style closures rather than relying on
    /// `extended_palette()`, and it is the ceiling on how well any third-party
    /// widget can match Coherence.
    pub fn iced_theme(&self) -> Theme {
        Theme::custom(
            "Canvas · spike".to_string(),
            iced::theme::Palette {
                background: self.background,
                text: self.text,
                primary: self.primary,
                success: self.success,
                danger: self.danger,
                warning: self.warning,
            },
        )
    }
}
