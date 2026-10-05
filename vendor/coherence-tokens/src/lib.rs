//! Coherence design tokens for native UI, with no dependencies.
//!
//! These are the same values the CSS uses. They are generated from
//! `config/product-theme.css` during Coherence's build, so a native app and a
//! browser rendering the same product cannot drift apart — which is the whole
//! reason this crate exists rather than each app carrying its own palette.
//!
//! # Themes
//!
//! Three themes and one density, each a module of constants:
//!
//! | module | when to use it |
//! |---|---|
//! | [`light`] | the default |
//! | [`dark`] | the viewer asked for dark, or the OS did |
//! | [`sepia`] | long-form reading; a warm, low-luminance ground |
//! | [`compact`] | a density, not a theme — control and row sizes only |
//!
//! ```
//! use coherence_tokens::{Mode, light, sepia};
//!
//! // Colours are [r, g, b], ready for any renderer.
//! assert_eq!(light::COLOR_BACKGROUND, [245, 246, 247]);
//!
//! // Sepia is a reading ground, so it is warm rather than grey.
//! let [r, _, b] = sepia::COLOR_BACKGROUND;
//! assert!(r > b, "sepia leans warm");
//!
//! // Or take a whole palette by mode.
//! assert_eq!(Mode::Light.palette().background, light::COLOR_BACKGROUND);
//! ```
//!
//! # Reading
//!
//! Long-form surfaces should take their measurements from here rather than
//! inventing numbers. The values follow WCAG 2.2 §1.4.8: line spacing of at
//! least 1.5 within paragraphs, paragraph spacing of at least 1.5× that, and
//! no more than 80 characters per line.
//!
//! ```
//! use coherence_tokens::light;
//! assert_eq!(light::READING_FONT_SIZE, 17.0);
//! assert!(light::READING_LINE_HEIGHT >= 1.5, "WCAG 1.4.8 floor");
//! ```
//!
//! # Contrast
//!
//! Every theme is checked at build time: body text reaches AAA (7:1) against
//! each of its surfaces, and every semantic colour clears AA (4.5:1). A palette
//! that fails is a build failure, not a review comment — so a theme published
//! here has already been measured.
//!
//! # Stability
//!
//! `src/generated.rs` is written by Coherence's build. Do not edit it; change
//! `config/product-theme.css` and rebuild, which regenerates this crate and
//! re-runs the contrast assertions.

mod generated;
pub use generated::{compact, dark, light, sepia};

/// A colour theme.
///
/// `Sepia` is never selected by the operating system — no OS setting expresses
/// "I want a warm reading ground" — so it is only ever an explicit choice by
/// the person reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
    /// Warm low-luminance reading ground. Not a system preference — no OS
    /// setting expresses it, so it is only ever an explicit choice.
    Sepia,
}

/// How tightly controls and rows are packed.
///
/// A density is orthogonal to a theme: `Compact` is right for a console and
/// wrong for prose, whichever colours are in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Comfortable,
    Compact,
}

/// The colours a renderer needs, resolved for one [`Mode`].
///
/// Each is `[r, g, b]`. This is a convenience over the theme modules, which
/// carry every token including the ones most apps never touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: [u8; 3],
    pub surface: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub primary: [u8; 3],
    pub success: [u8; 3],
    pub danger: [u8; 3],
    pub warning: [u8; 3],
}

impl Mode {
    /// The colours for this theme.
    ///
    /// ```
    /// use coherence_tokens::Mode;
    /// // Every theme is distinct — no two resolve to the same palette.
    /// assert_ne!(Mode::Light.palette(), Mode::Sepia.palette());
    /// ```
    pub const fn palette(self) -> Palette {
        match self {
            Self::Light => Palette {
                background: light::COLOR_BACKGROUND,
                surface: light::COLOR_SURFACE,
                text: light::COLOR_TEXT,
                muted: light::COLOR_TEXT_MUTED,
                primary: light::COLOR_BRAND,
                success: light::COLOR_SUCCESS,
                danger: light::COLOR_DANGER,
                warning: light::COLOR_WARNING,
            },
            Self::Dark => Palette {
                background: dark::COLOR_BACKGROUND,
                surface: dark::COLOR_SURFACE,
                text: dark::COLOR_TEXT,
                muted: dark::COLOR_TEXT_MUTED,
                primary: dark::COLOR_BRAND,
                success: dark::COLOR_SUCCESS,
                danger: dark::COLOR_DANGER,
                warning: dark::COLOR_WARNING,
            },
            Self::Sepia => Palette {
                background: sepia::COLOR_BACKGROUND,
                surface: sepia::COLOR_SURFACE,
                text: sepia::COLOR_TEXT,
                muted: sepia::COLOR_TEXT_MUTED,
                primary: sepia::COLOR_BRAND,
                success: sepia::COLOR_SUCCESS,
                danger: sepia::COLOR_DANGER,
                warning: sepia::COLOR_WARNING,
            },
        }
    }
}

impl Density {
    /// Height of a control such as a button or input, in logical pixels.
    pub const fn control_height(self) -> f32 {
        match self {
            Self::Comfortable => light::CONTROL_HEIGHT,
            Self::Compact => compact::CONTROL_HEIGHT,
        }
    }

    /// Height of a table or list row, in logical pixels.
    ///
    /// Always taller than [`Self::control_height`], so a control sits inside a
    /// row rather than defining it.
    pub const fn row_height(self) -> f32 {
        match self {
            Self::Comfortable => light::ROW_HEIGHT,
            Self::Compact => compact::ROW_HEIGHT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_use_distinct_palettes() {
        assert_ne!(Mode::Light.palette(), Mode::Dark.palette());
        assert_eq!(Mode::Light.palette().primary, light::COLOR_BRAND);
        assert_eq!(Mode::Dark.palette().text, dark::COLOR_TEXT);
    }

    /// The generated file is written by Coherence's build, but the module
    /// re-export and `Mode` are hand-written — so a new theme lands in
    /// generated.rs and is silently unreachable. That is exactly how sepia
    /// shipped missing from 0.1.0.
    #[test]
    fn every_generated_theme_is_reachable() {
        let generated = include_str!("generated.rs");
        for module in generated
            .lines()
            .filter_map(|l| l.strip_prefix("pub mod ").and_then(|m| m.split(' ').next()))
        {
            // compact is a density, not a mode, and has no palette.
            if module == "compact" {
                continue;
            }
            let reachable = [Mode::Light, Mode::Dark, Mode::Sepia]
                .iter()
                .any(|m| format!("{m:?}").to_lowercase() == module);
            assert!(reachable, "generated module `{module}` has no Mode variant");
        }
    }

    #[test]
    fn every_mode_has_a_distinct_palette() {
        let modes = [Mode::Light, Mode::Dark, Mode::Sepia];
        for (i, a) in modes.iter().enumerate() {
            for b in &modes[i + 1..] {
                assert_ne!(a.palette(), b.palette(), "{a:?} and {b:?} share a palette");
            }
        }
    }

    #[test]
    fn compact_keeps_controls_smaller_than_rows() {
        assert!(Density::Compact.control_height() < Density::Comfortable.control_height());
        assert!(Density::Compact.row_height() > Density::Compact.control_height());
    }
}
