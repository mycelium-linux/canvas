//! Generated, renderer-independent Coherence tokens. Dimensions are logical pixels.
//! The product profile is a draft; consumers should pin a reviewed Git revision.

mod generated;
pub use generated::{compact, dark, light};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Comfortable,
    Compact,
}

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
        }
    }
}

impl Density {
    pub const fn control_height(self) -> f32 {
        match self {
            Self::Comfortable => light::CONTROL_HEIGHT,
            Self::Compact => compact::CONTROL_HEIGHT,
        }
    }

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

    #[test]
    fn compact_keeps_controls_smaller_than_rows() {
        assert!(Density::Compact.control_height() < Density::Comfortable.control_height());
        assert!(Density::Compact.row_height() > Density::Compact.control_height());
    }
}
