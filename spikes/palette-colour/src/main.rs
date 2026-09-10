//! SPIKE: would the `palette` crate noticeably improve Canvas's colour?
//!
//! Four routes from an OKLCH triple to a displayable sRGB colour are compared:
//!
//!   hand      Canvas's own code, verbatim (canvas_impl.rs): Ottosson's
//!             published sRGB<->Oklab matrices, chroma reduced by binary search
//!             until the colour lands inside sRGB.
//!   pal+bsearch  palette's conversions, Canvas's gamut policy. What adoption
//!             would actually look like: the crate supplies the maths, Canvas
//!             still supplies the policy, because palette has no public
//!             gamut-mapping API.
//!   pal+Clamp palette's conversions and palette's own in-range trait, `Clamp`,
//!             which clamps each channel independently. This is the naive
//!             clipping Canvas's from_oklch doc-comment says it rejected.
//!   pal+Okhsl palette's conversions plus Ottosson's analytic cusp
//!             approximation, reached by converting to Okhsl and clamping
//!             saturation to 1. The strongest case for the dependency: a real,
//!             hue-preserving gamut map Canvas would not have to write.
//!             (palette implements neither Clamp nor IsWithinBounds for Okhsl,
//!             so the saturation clamp is still hand-written.)

mod canvas_impl;
mod palettes;

use canvas_impl as hand;
use palette::color_difference::Ciede2000;
use palette::{Clamp, FromColor, IntoColor, IsWithinBounds, Lab, Okhsl, Oklch as PalOklch, Srgb};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Color {
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
    fn hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            hand::channel_u8(self.r),
            hand::channel_u8(self.g),
            hand::channel_u8(self.b)
        )
    }
    fn srgb(self) -> Srgb<f32> {
        Srgb::new(self.r, self.g, self.b)
    }
    fn of(srgb: Srgb<f32>) -> Self {
        Self::from_rgb(srgb.red, srgb.green, srgb.blue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemePalette {
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

// ---------------------------------------------------------------------------
// The four routes
// ---------------------------------------------------------------------------

fn pal_to_oklch(c: Color) -> PalOklch<f32> {
    PalOklch::from_color(c.srgb())
}

fn hand_oklch(v: PalOklch<f32>) -> hand::Oklch {
    hand::Oklch {
        l: v.l,
        c: v.chroma,
        h: v.hue.into_positive_degrees(),
    }
}

/// palette maths, palette's own way back into range: per-channel clamp.
fn route_pal_clamp(v: PalOklch<f32>) -> Color {
    Color::of(Srgb::<f32>::from_color(v).clamp())
}

/// palette maths, Canvas's gamut policy (binary search on chroma).
fn route_pal_bsearch(v: PalOklch<f32>) -> Color {
    let mut chroma = v.chroma;
    if !Srgb::<f32>::from_color(v).is_within_bounds() {
        let (mut lo, mut hi) = (0.0f32, v.chroma);
        for _ in 0..8 {
            let mid = (lo + hi) / 2.0;
            if Srgb::<f32>::from_color(PalOklch::new(v.l, mid, v.hue)).is_within_bounds() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chroma = lo;
    }
    Color::of(Srgb::<f32>::from_color(PalOklch::new(v.l, chroma, v.hue)).clamp())
}

/// palette maths plus Ottosson's analytic gamut cusp, via Okhsl saturation.
fn route_pal_okhsl(v: PalOklch<f32>) -> Color {
    let mut hsl: Okhsl<f32> = Okhsl::from_color(v);
    if hsl.saturation > 1.0 {
        hsl.saturation = 1.0;
    }
    Color::of(Srgb::<f32>::from_color(hsl).clamp())
}

type Route = fn(PalOklch<f32>) -> Color;

const ROUTES: [(&str, Route); 3] = [
    ("pal+bsearch", route_pal_bsearch),
    ("pal+Clamp", route_pal_clamp),
    ("pal+Okhsl", route_pal_okhsl),
];

/// `series_colors` rebuilt on palette. Structure identical to the hand-written
/// one; only the colour-space calls change.
fn pal_series_colors(palette: &ThemePalette, gamut: Route) -> Vec<Color> {
    let base = pal_to_oklch(hand::color(palette.primary));
    let deg = |rgb: [u8; 3]| pal_to_oklch(hand::color(rgb)).hue.into_positive_degrees();
    let open = hand::categorical_arcs(&[
        deg(palette.success),
        deg(palette.warning),
        deg(palette.danger),
        deg(palette.info),
    ]);
    let span = hand::arc_span(&open);
    let start = hand::arc_offset(&open, base.hue.into_positive_degrees());
    let chroma = base
        .chroma
        .clamp(hand::SERIES_CHROMA_MIN, hand::SERIES_CHROMA_MAX);
    (0..hand::SERIES_COLORS)
        .map(|index| {
            let lift = if index % 2 == 0 {
                1.0
            } else {
                1.0 + hand::SERIES_LIGHTNESS_STEP
            };
            gamut(PalOklch::new(
                (base.l * lift).clamp(hand::SERIES_LIGHTNESS_MIN, hand::SERIES_LIGHTNESS_MAX),
                chroma,
                hand::arc_hue(&open, start + span * index as f32 / hand::SERIES_COLORS as f32),
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// CIEDE2000, via palette's own implementation. 1.0 is the classical
/// just-noticeable difference for adjacent patches; ~2.3 is the threshold
/// usually quoted for "noticeable at a glance".
fn delta_e2000(a: Color, b: Color) -> f32 {
    let a: Lab = a.srgb().into_color();
    let b: Lab = b.srgb().into_color();
    a.difference(b)
}

/// Euclidean distance in Oklab — the metric the ramp's own spacing rule uses
/// (live.rs `separation`, which requires > 0.05 between series).
fn delta_e_ok(a: Color, b: Color) -> f32 {
    let p = |c: Color| {
        let v = hand::to_oklch(c);
        let (sin, cos) = v.h.to_radians().sin_cos();
        (v.l, v.c * cos, v.c * sin)
    };
    let (l1, a1, b1) = p(a);
    let (l2, a2, b2) = p(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
}

fn hue_gap(a: f32, b: f32) -> f32 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

fn roles(p: &ThemePalette) -> Vec<(&'static str, [u8; 3])> {
    vec![
        ("background", p.background),
        ("background_strong", p.background_strong),
        ("surface", p.surface),
        ("surface_muted", p.surface_muted),
        ("text", p.text),
        ("text_muted", p.text_muted),
        ("primary", p.primary),
        ("success", p.success),
        ("warning", p.warning),
        ("danger", p.danger),
        ("info", p.info),
        ("border", p.border),
        ("border_strong", p.border_strong),
    ]
}

/// Resolved version, read from the lockfile rather than written down twice.
fn pal_version() -> String {
    let lock = include_str!("../Cargo.lock");
    let mut want = false;
    for line in lock.lines() {
        if line.trim() == "name = \"palette\"" {
            want = true;
        } else if want && line.starts_with("version = ") {
            return line.trim_start_matches("version = ").trim_matches('"').into();
        }
    }
    "unknown".into()
}

// ---------------------------------------------------------------------------

fn main() {
    println!(
        "SPIKE palette {} vs hand-written OKLCH in crates/canvas-widgets/src/live.rs",
        pal_version()
    );
    println!();
    section_1_conversion_agreement();
    section_2_ramp_agreement();
    section_3_gamut_policy();
    section_4_status_guard();
    section_5_drain();
    section_6_api_surface();
}

/// 1. Do the two implementations agree on what OKLCH a real theme colour has?
///    This is the "does palette catch an error in ours" question.
fn section_1_conversion_agreement() {
    println!("== 1. sRGB -> OKLCH agreement on real theme roles ==");
    println!("   (Canvas uses Ottosson's composed sRGB<->Oklab matrices;");
    println!("    palette routes LinSrgb -> XYZ D65 -> Oklab. Same space, different arithmetic.)");
    let mut worst_l = (0.0f32, String::new());
    let mut worst_c = (0.0f32, String::new());
    let mut worst_h = (0.0f32, String::new());
    let mut worst_rt = (0.0f32, String::new());
    let mut n = 0;
    for (name, p) in palettes::all() {
        for (role, rgb) in roles(&p) {
            n += 1;
            let c = hand::color(rgb);
            let a = hand::to_oklch(c);
            let b = pal_to_oklch(c);
            let tag = format!("{name}/{role} {rgb:?}");
            let dl = (a.l - b.l).abs();
            let dc = (a.c - b.chroma).abs();
            // Hue of a near-grey is meaningless; only compare where chroma is real.
            let dh = if a.c > 0.01 {
                hue_gap(a.h, b.hue.into_positive_degrees())
            } else {
                0.0
            };
            if dl > worst_l.0 {
                worst_l = (dl, tag.clone());
            }
            if dc > worst_c.0 {
                worst_c = (dc, tag.clone());
            }
            if dh > worst_h.0 {
                worst_h = (dh, tag.clone());
            }
            // Full round trip: sRGB -> OKLCH -> sRGB, each implementation alone.
            let de = delta_e2000(hand::from_oklch(a), route_pal_bsearch(b));
            if de > worst_rt.0 {
                worst_rt = (de, tag);
            }
        }
    }
    println!("  {n} role colours across {} palettes", palettes::all().len());
    println!("  worst dL   = {:.7}   ({})", worst_l.0, worst_l.1);
    println!("  worst dC   = {:.7}   ({})", worst_c.0, worst_c.1);
    println!("  worst dHue = {:.5} deg ({})", worst_h.0, worst_h.1);
    println!(
        "  worst round-trip dE2000 = {:.5} ({})",
        worst_rt.0, worst_rt.1
    );
    println!();
}

/// 2. The actual deliverable: the six-colour series ramp, every route.
fn section_2_ramp_agreement() {
    println!("== 2. Series ramp: hand-written vs each palette route ==");
    println!(
        "{:<30} {:>2}  {:<8} | {:<8} {:>7} {:>7} | {:<8} {:>7} | {:<8} {:>7}",
        "palette", "i", "hand", "pal+bsrch", "dE2000", "dEok", "pal+Clmp", "dE2000", "pal+Okhsl",
        "dE2000"
    );
    let mut worst = [(0.0f32, String::new()), (0.0, String::new()), (0.0, String::new())];
    let mut sum = [0.0f32; 3];
    let mut count = 0;
    for (name, p) in palettes::all() {
        let a = hand::series_colors(&p);
        let by_route: Vec<Vec<Color>> = ROUTES
            .iter()
            .map(|(_, f)| pal_series_colors(&p, *f))
            .collect();
        for i in 0..a.len() {
            count += 1;
            let d: Vec<f32> = (0..3).map(|r| delta_e2000(a[i], by_route[r][i])).collect();
            for r in 0..3 {
                sum[r] += d[r];
                if d[r] > worst[r].0 {
                    worst[r] = (d[r], format!("{name} series {i}"));
                }
            }
            println!(
                "{:<30} {:>2}  {:<8} | {:<8} {:>7.3} {:>7.4} | {:<8} {:>7.3} | {:<8} {:>7.3}",
                if i == 0 { name } else { "" },
                i,
                a[i].hex(),
                by_route[0][i].hex(),
                d[0],
                delta_e_ok(a[i], by_route[0][i]),
                by_route[1][i].hex(),
                d[1],
                by_route[2][i].hex(),
                d[2],
            );
        }
    }
    println!();
    for r in 0..3 {
        println!(
            "  {:<12} mean dE2000 = {:.4}   worst = {:.4}  ({})",
            ROUTES[r].0,
            sum[r] / count as f32,
            worst[r].0,
            worst[r].1
        );
    }
    println!("  reference: dE2000 1.0 = JND for adjacent patches; 2.3 = noticeable at a glance");
    println!();
}

/// 3. The gamut question, head on.
fn section_3_gamut_policy() {
    println!("== 3. Gamut policy on out-of-gamut OKLCH ==");
    println!("   palette 0.7.7 exposes NO public gamut-mapping API. `Clamp` clamps");
    println!("   components; the Ottosson cusp approximation exists only as the private");
    println!("   `ok_utils::find_gamut_intersection`, reachable in public API only by");
    println!("   round-tripping through Okhsl/Okhsv and clamping saturation by hand.");
    println!();

    let cases: Vec<(&str, f32, f32, f32)> = vec![
        ("light saturated blue", 0.75, 0.20, 264.0),
        ("light saturated blue (brighter)", 0.85, 0.20, 264.0),
        ("mid saturated blue", 0.50, 0.25, 264.0),
        ("light saturated violet", 0.80, 0.22, 300.0),
        ("light saturated red", 0.75, 0.22, 25.0),
        ("light saturated green", 0.92, 0.25, 145.0),
    ];
    println!(
        "  {:<32} {:>5} {:>5} {:>6} | {:<8} {:>7} | {:<8} {:>7} | {:<8} {:>7} | {:<8} {:>7}",
        "case", "L", "C", "hue", "hand", "d_hue", "pal+bsrch", "d_hue", "pal+Clamp", "d_hue",
        "pal+Okhsl", "d_hue"
    );
    for (label, l, c, h) in &cases {
        let hv = hand::Oklch { l: *l, c: *c, h: *h };
        let pv = PalOklch::new(*l, *c, *h);
        let out = [
            hand::from_oklch(hv),
            route_pal_bsearch(pv),
            route_pal_clamp(pv),
            route_pal_okhsl(pv),
        ];
        let shift = |c: Color| hue_gap(hand::to_oklch(c).h, *h);
        println!(
            "  {:<32} {:>5.2} {:>5.3} {:>6.1} | {:<8} {:>7.2} | {:<8} {:>7.2} | {:<8} {:>7.2} | {:<8} {:>7.2}",
            label, l, c, h,
            out[0].hex(), shift(out[0]),
            out[1].hex(), shift(out[1]),
            out[2].hex(), shift(out[2]),
            out[3].hex(), shift(out[3]),
        );
    }
    println!();

    // The claim in live.rs's doc-comment, tested: clipping shifts a light
    // saturated blue by ~14 degrees. Sweep the blue region for the worst case.
    let mut worst_blue = (0.0f32, String::new());
    for l in (30..=95).step_by(1) {
        let l = l as f32 / 100.0;
        for c in (5..=32).step_by(1) {
            let c = c as f32 / 100.0;
            for h in 240..=290 {
                let h = h as f32;
                let pv = PalOklch::new(l, c, h);
                if Srgb::<f32>::from_color(pv).is_within_bounds() {
                    continue;
                }
                let d = hue_gap(hand::to_oklch(route_pal_clamp(pv)).h, h);
                if d > worst_blue.0 {
                    worst_blue = (d, format!("L={l:.2} C={c:.2} h={h:.0}"));
                }
            }
        }
    }
    println!(
        "  clipping's worst hue shift in the blue region (h 240-290): {:.2} deg  ({})",
        worst_blue.0, worst_blue.1
    );

    // Whole-space sweep: how far apart are the two chroma-reducing routes, and
    // how bad does clipping get anywhere?
    let mut stats = [(0.0f32, String::new()), (0.0, String::new()), (0.0, String::new())];
    let mut de_vs_hand = [(0.0f32, String::new()), (0.0, String::new()), (0.0, String::new())];
    let mut oog = 0u32;
    let mut total = 0u32;
    for l in (30..=95).step_by(1) {
        let l = l as f32 / 100.0;
        for c in (5..=32).step_by(1) {
            let c = c as f32 / 100.0;
            for h in (0..360).step_by(2) {
                let h = h as f32;
                total += 1;
                let pv = PalOklch::new(l, c, h);
                if Srgb::<f32>::from_color(pv).is_within_bounds() {
                    continue;
                }
                oog += 1;
                let hv = hand::Oklch { l, c, h };
                let base = hand::from_oklch(hv);
                let tag = format!("L={l:.2} C={c:.2} h={h:.0}");
                for (r, (_, f)) in ROUTES.iter().enumerate() {
                    let out = f(pv);
                    let dh = hue_gap(hand::to_oklch(out).h, h);
                    if dh > stats[r].0 {
                        stats[r] = (dh, tag.clone());
                    }
                    let de = delta_e2000(base, out);
                    if de > de_vs_hand[r].0 {
                        de_vs_hand[r] = (de, tag.clone());
                    }
                }
            }
        }
    }
    println!("  sweep: {oog} of {total} OKLCH samples fall outside sRGB");
    for r in 0..3 {
        println!(
            "    {:<12} worst hue shift {:>6.2} deg ({:<28}) | worst dE2000 vs hand {:>7.3} ({})",
            ROUTES[r].0, stats[r].0, stats[r].1, de_vs_hand[r].0, de_vs_hand[r].1
        );
    }
    println!();
}

/// 4. The only difference that is categorical rather than perceptual.
fn section_4_status_guard() {
    println!("== 4. Does the status-hue guarantee survive each route? ==");
    println!(
        "   guard = {} deg either side of every status hue; live.rs asserts >= {} deg",
        hand::STATUS_HUE_GUARD,
        hand::STATUS_HUE_GUARD - 1.0
    );
    for (name, p) in palettes::all() {
        let status = [
            ("success", hand::to_oklch(hand::color(p.success)).h),
            ("warning", hand::to_oklch(hand::color(p.warning)).h),
            ("danger", hand::to_oklch(hand::color(p.danger)).h),
            ("info", hand::to_oklch(hand::color(p.info)).h),
        ];
        let check = |colors: &[Color]| -> (f32, String) {
            let mut worst = (f32::INFINITY, String::new());
            for (i, c) in colors.iter().enumerate() {
                let h = hand::to_oklch(*c).h;
                for (role, sh) in status {
                    let gap = hue_gap(h, sh);
                    if gap < worst.0 {
                        worst = (gap, format!("s{i}/{role}"));
                    }
                }
            }
            worst
        };
        let verdict = |g: f32| {
            if g >= hand::STATUS_HUE_GUARD - 1.0 {
                "holds"
            } else {
                "VIOLATED"
            }
        };
        let h = check(&hand::series_colors(&p));
        print!("  {name:<30} hand {:>5.2} {:<8}", h.0, verdict(h.0));
        for (label, f) in ROUTES {
            let r = check(&pal_series_colors(&p, f));
            print!(" | {label} {:>5.2} {:<8} {}", r.0, verdict(r.0), r.1);
        }
        println!();
    }
    println!();
}

/// 5. The other consumer of the conversions: staleness draining.
fn section_5_drain() {
    println!("== 5. Stale-value chroma drain (drain(palette, {})) ==", hand::STALE_CHROMA);
    let mut worst = (0.0f32, String::new());
    for (name, p) in palettes::all() {
        let hand_d = hand::drain(p, hand::STALE_CHROMA);
        let hand_roles = roles(&hand_d);
        for (i, (role, rgb)) in roles(&p).into_iter().enumerate() {
            let v = pal_to_oklch(hand::color(rgb));
            let pal_d = route_pal_bsearch(PalOklch::new(
                v.l,
                v.chroma * hand::STALE_CHROMA,
                v.hue,
            ));
            let de = delta_e2000(hand::color(hand_roles[i].1), pal_d);
            debug_assert_eq!(role, hand_roles[i].0);
            if de > worst.0 {
                worst = (de, format!("{name}/{role}"));
            }
        }
    }
    println!(
        "  worst dE2000 across every role of every palette: {:.5}  ({})",
        worst.0, worst.1
    );
    println!();
}

/// 6. What the dependency actually is.
fn section_6_api_surface() {
    println!("== 6. What Canvas would and would not stop maintaining ==");
    let replaced = [
        ("to_oklch", true),
        ("oklch_to_linear", true),
        ("srgb_decode / srgb_encode", true),
        ("in_srgb_gamut", true),
        ("from_oklch (gamut policy)", false),
        ("categorical_arcs", false),
        ("arc_span / arc_hue / arc_offset", false),
        ("series_colors", false),
        ("drain", false),
        ("STATUS_HUE_GUARD and the chroma/lightness rules", false),
    ];
    for (what, gone) in replaced {
        println!(
            "  {:<48} {}",
            what,
            if gone {
                "replaced by palette"
            } else {
                "STAYS hand-written"
            }
        );
    }
    println!();
}
