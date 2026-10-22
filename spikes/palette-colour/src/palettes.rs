//! Real palettes, transcribed from the Canvas tree.
//!
//! Coherence values: vendor/coherence-tokens/src/generated.rs (light/dark),
//! mapped to roles the way crates/canvas-core/src/lib.rs `ProjectTheme::palette`
//! does with `Accent::Brand`.
//! Catppuccin / Tokyo Night values: the fixtures in
//! crates/canvas-core/src/theme_file.rs, mapped by `parse_catppuccin` and
//! `parse_tokyo_night` in the same file.

use crate::ThemePalette;

fn hex(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    [
        u8::from_str_radix(&s[0..2], 16).unwrap(),
        u8::from_str_radix(&s[2..4], 16).unwrap(),
        u8::from_str_radix(&s[4..6], 16).unwrap(),
    ]
}

/// canvas-core's `mix`, verbatim: rounded linear blend in 8-bit sRGB.
fn mix(a: [u8; 3], b: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        (f32::from(a[i]) * (1.0 - amount) + f32::from(b[i]) * amount).round() as u8
    })
}

pub fn all() -> Vec<(&'static str, ThemePalette)> {
    let light = coherence_light();
    let mut collided = light;
    collided.primary = collided.danger;
    vec![
        ("coherence-dark", coherence_dark()),
        ("coherence-light", light),
        ("catppuccin-mocha", catppuccin_mocha()),
        ("catppuccin-latte", catppuccin_latte()),
        ("tokyo-night", tokyo_night()),
        // The pathological case the guard exists for, from live.rs's own tests.
        ("primary-collides-with-danger", collided),
    ]
}

pub fn coherence_dark() -> ThemePalette {
    ThemePalette {
        background: [24, 25, 28],
        background_strong: [36, 38, 43],
        surface: [32, 34, 38],
        surface_muted: [41, 44, 49],
        text: [241, 242, 244],
        text_muted: [173, 179, 189],
        primary: [154, 175, 255],
        success: [126, 211, 161],
        warning: [239, 191, 112],
        danger: [255, 157, 170],
        info: [131, 203, 220],
        border: [59, 64, 72],
        border_strong: [127, 135, 148],
    }
}

pub fn coherence_light() -> ThemePalette {
    ThemePalette {
        background: [245, 246, 247],
        background_strong: [233, 236, 239],
        surface: [255, 255, 255],
        surface_muted: [240, 242, 244],
        text: [32, 35, 41],
        text_muted: [96, 104, 116],
        primary: [36, 73, 216],
        success: [33, 117, 75],
        warning: [139, 89, 13],
        danger: [177, 43, 64],
        info: [40, 105, 121],
        border: [217, 221, 227],
        border_strong: [134, 142, 154],
    }
}

fn catppuccin_mocha() -> ThemePalette {
    ThemePalette {
        background: hex("#1e1e2e"),
        background_strong: hex("#11111b"), // dark scheme -> crust
        surface: hex("#313244"),
        surface_muted: hex("#45475a"),
        text: hex("#cdd6f4"),
        text_muted: hex("#a6adc8"),
        primary: hex("#cba6f7"), // mauve
        success: hex("#a6e3a1"),
        warning: hex("#f9e2af"),
        danger: hex("#f38ba8"),
        info: hex("#89b4fa"),
        border: hex("#45475a"),
        border_strong: hex("#585b70"),
    }
}

fn catppuccin_latte() -> ThemePalette {
    ThemePalette {
        background: hex("#eff1f5"),
        background_strong: hex("#e6e9ef"), // light scheme -> mantle
        surface: hex("#ccd0da"),
        surface_muted: hex("#bcc0cc"),
        text: hex("#4c4f69"),
        text_muted: hex("#6c6f85"),
        primary: hex("#8839ef"),
        success: hex("#40a02b"),
        warning: hex("#df8e1d"),
        danger: hex("#d20f39"),
        info: hex("#1e66f5"),
        border: hex("#bcc0cc"),
        border_strong: hex("#acb0be"),
    }
}

fn tokyo_night() -> ThemePalette {
    let bg = hex("#1a1b26");
    let bg_light = hex("#292e42");
    let fg = hex("#c0caf5");
    ThemePalette {
        background: bg,
        background_strong: hex("#16161e"),
        surface: bg_light,
        surface_muted: mix(bg_light, bg, 0.5),
        text: fg,
        text_muted: hex("#565f89"),
        primary: hex("#7aa2f7"),
        success: hex("#9ece6a"),
        warning: hex("#e0af68"),
        danger: hex("#f7768e"),
        info: hex("#7dcfff"),
        border: mix(bg, fg, 0.18),
        border_strong: mix(bg, fg, 0.34),
    }
}
