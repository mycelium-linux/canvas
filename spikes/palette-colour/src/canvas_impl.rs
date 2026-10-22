//! Verbatim copy of Canvas's hand-written colour code.
//!
//! Lifted from `crates/canvas-widgets/src/live.rs` (to_oklch, from_oklch,
//! categorical_arcs, arc_span, arc_hue, arc_offset, series_colors, drain) with
//! only two changes: `iced::Color` is replaced by the local `Color` shim, and
//! `ThemePalette` by the local one. No numeric or structural edits — the point
//! of the spike is to measure THIS code, not a paraphrase of it.

use crate::{Color, ThemePalette};

pub fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
    )
}

pub const SERIES_COLORS: usize = 6;
pub const STATUS_HUE_GUARD: f32 = 25.0;
pub const SERIES_CHROMA_MIN: f32 = 0.08;
pub const SERIES_CHROMA_MAX: f32 = 0.15;
pub const SERIES_LIGHTNESS_STEP: f32 = 0.12;
pub const SERIES_LIGHTNESS_MIN: f32 = 0.30;
pub const SERIES_LIGHTNESS_MAX: f32 = 0.92;
pub const STALE_CHROMA: f32 = 0.35;

pub fn series_colors(palette: &ThemePalette) -> Vec<Color> {
    let base = to_oklch(color(palette.primary));
    let open = categorical_arcs(&[
        to_oklch(color(palette.success)).h,
        to_oklch(color(palette.warning)).h,
        to_oklch(color(palette.danger)).h,
        to_oklch(color(palette.info)).h,
    ]);
    let span = arc_span(&open);
    let start = arc_offset(&open, base.h);
    let chroma = base.c.clamp(SERIES_CHROMA_MIN, SERIES_CHROMA_MAX);
    (0..SERIES_COLORS)
        .map(|index| {
            let lift = if index % 2 == 0 {
                1.0
            } else {
                1.0 + SERIES_LIGHTNESS_STEP
            };
            from_oklch(Oklch {
                l: (base.l * lift).clamp(SERIES_LIGHTNESS_MIN, SERIES_LIGHTNESS_MAX),
                c: chroma,
                h: arc_hue(&open, start + span * index as f32 / SERIES_COLORS as f32),
            })
        })
        .collect()
}

pub fn drain(palette: ThemePalette, keep: f32) -> ThemePalette {
    let role = |rgb: [u8; 3]| {
        let value = to_oklch(color(rgb));
        let drained = from_oklch(Oklch {
            c: value.c * keep,
            ..value
        });
        [
            channel_u8(drained.r),
            channel_u8(drained.g),
            channel_u8(drained.b),
        ]
    };
    ThemePalette {
        background: role(palette.background),
        background_strong: role(palette.background_strong),
        surface: role(palette.surface),
        surface_muted: role(palette.surface_muted),
        text: role(palette.text),
        text_muted: role(palette.text_muted),
        primary: role(palette.primary),
        success: role(palette.success),
        warning: role(palette.warning),
        danger: role(palette.danger),
        info: role(palette.info),
        border: role(palette.border),
        border_strong: role(palette.border_strong),
    }
}

pub fn channel_u8(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn categorical_arcs(status_hues: &[f32]) -> Vec<(f32, f32)> {
    let mut blocked: Vec<(f32, f32)> = Vec::new();
    for hue in status_hues {
        let start = (hue - STATUS_HUE_GUARD).rem_euclid(360.0);
        let end = start + STATUS_HUE_GUARD * 2.0;
        if end > 360.0 {
            blocked.push((start, 360.0));
            blocked.push((0.0, end - 360.0));
        } else {
            blocked.push((start, end));
        }
    }
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (start, end) in blocked {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut open = Vec::new();
    let mut cursor = 0.0f32;
    for (start, end) in merged {
        if start > cursor {
            open.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < 360.0 {
        open.push((cursor, 360.0));
    }
    open
}

pub fn arc_span(arcs: &[(f32, f32)]) -> f32 {
    arcs.iter().map(|(start, end)| end - start).sum()
}

pub fn arc_hue(arcs: &[(f32, f32)], distance: f32) -> f32 {
    let span = arc_span(arcs);
    if span <= f32::EPSILON {
        return distance.rem_euclid(360.0);
    }
    let mut remaining = distance.rem_euclid(span);
    for (start, end) in arcs {
        if remaining < end - start {
            return start + remaining;
        }
        remaining -= end - start;
    }
    arcs[0].0
}

pub fn arc_offset(arcs: &[(f32, f32)], hue: f32) -> f32 {
    let mut travelled = 0.0;
    for (start, end) in arcs {
        if hue < *end {
            return travelled + (hue - start).max(0.0);
        }
        travelled += end - start;
    }
    0.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

pub fn to_oklch(value: Color) -> Oklch {
    let r = srgb_decode(value.r);
    let g = srgb_decode(value.g);
    let b = srgb_decode(value.b);
    let long = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b).cbrt();
    let medium = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let short = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let lightness = 0.210_454_26 * long + 0.793_617_8 * medium - 0.004_072_047 * short;
    let green_red = 1.977_998_5 * long - 2.428_592_2 * medium + 0.450_593_7 * short;
    let blue_yellow = 0.025_904_037 * long + 0.782_771_77 * medium - 0.808_675_77 * short;
    Oklch {
        l: lightness,
        c: green_red.hypot(blue_yellow),
        h: blue_yellow.atan2(green_red).to_degrees().rem_euclid(360.0),
    }
}

pub fn from_oklch(value: Oklch) -> Color {
    let mut chroma = value.c;
    if !in_srgb_gamut(oklch_to_linear(value)) {
        let (mut lo, mut hi) = (0.0, value.c);
        for _ in 0..8 {
            let mid = (lo + hi) / 2.0;
            if in_srgb_gamut(oklch_to_linear(Oklch { c: mid, ..value })) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chroma = lo;
    }
    let [r, g, b] = oklch_to_linear(Oklch { c: chroma, ..value });
    Color::from_rgb(srgb_encode(r), srgb_encode(g), srgb_encode(b))
}

/// NOT Canvas code. The naive alternative Canvas's doc-comment says it rejected:
/// convert and clip each channel where it leaves sRGB. Present so the spike can
/// measure the claim rather than repeat it.
pub fn from_oklch_by_clipping(value: Oklch) -> Color {
    let [r, g, b] = oklch_to_linear(value);
    Color::from_rgb(srgb_encode(r), srgb_encode(g), srgb_encode(b))
}

pub fn oklch_to_linear(value: Oklch) -> [f32; 3] {
    let (sin, cos) = value.h.to_radians().sin_cos();
    let (green_red, blue_yellow) = (value.c * cos, value.c * sin);
    let long = (value.l + 0.396_337_78 * green_red + 0.215_803_76 * blue_yellow).powi(3);
    let medium = (value.l - 0.105_561_346 * green_red - 0.063_854_17 * blue_yellow).powi(3);
    let short = (value.l - 0.089_484_18 * green_red - 1.291_485_5 * blue_yellow).powi(3);
    [
        4.076_741_7 * long - 3.307_711_6 * medium + 0.230_969_94 * short,
        -1.268_438 * long + 2.609_757_4 * medium - 0.341_319_38 * short,
        -0.004_196_086 * long - 0.703_418_6 * medium + 1.707_614_7 * short,
    ]
}

pub fn in_srgb_gamut(linear: [f32; 3]) -> bool {
    linear
        .iter()
        .all(|channel| (-1e-4..=1.0 + 1e-4).contains(channel))
}

pub fn srgb_decode(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_encode(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}
