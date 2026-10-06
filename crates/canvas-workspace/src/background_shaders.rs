//! Canonical calm background sources, shared by platform rendering adapters.
use crate::BackgroundShader;

pub fn source(shader: BackgroundShader) -> Option<&'static str> {
    match shader {
        BackgroundShader::Tidepool => Some(TIDEPOOL),
        BackgroundShader::AuroraDrift => Some(AURORA_DRIFT),
        BackgroundShader::DuskEmber => Some(DUSK_EMBER),
        BackgroundShader::SlowRipple => Some(SLOW_RIPPLE),
        _ => None,
    }
}

pub const UNIFORM_WGSL: &str = r#"
struct Uniforms {
    res: vec2<f32>,
    time: f32,
    _pad: f32,
    // The framebuffer coordinate that field position zero sits at.
    //
    // `@builtin(position)` is a framebuffer coordinate, not a field one, and
    // the two coincide only for a full-screen background on the first display.
    // A picker tile at x=1100 that divides its framebuffer position by its own
    // width puts `uv` somewhere around 3.0 instead of 0..1 — flat colour for the
    // gradient shaders, black for the rest. A window on the second display has
    // the mirror problem: it starts at framebuffer 0 but half-way across the
    // desk, so subtracting nothing there draws a second copy of the field
    // rather than its right half. Both are the same subtraction.
    origin: vec2<f32>,
    _pad2: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

/// Pixel coordinate inside the field, which is whatever `u.res` measures: the
/// whole desk for a background spanning several displays, the widget itself for
/// a preview tile. Every shader starts here rather than from `pos.xy`, so one
/// of them cannot quietly go back to assuming it owns the whole framebuffer.
fn local(pos: vec4<f32>) -> vec2<f32> {
    return pos.xy - u.origin;
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(p[index], 0.0, 1.0);
}
"#;

pub const TIDEPOOL: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = local(pos) / u.res;
    let t = u.time;
    var c = mix(vec3<f32>(0.02, 0.05, 0.11), vec3<f32>(0.05, 0.14, 0.21), uv.y);
    var w = sin(uv.x * 4.2 + uv.y * 1.3 + t * 0.10) * 0.5 + 0.5;
    c += vec3<f32>(0.05, 0.28, 0.34) * w * 0.22;
    w = sin(uv.x * 6.8 - uv.y * 0.9 + t * 0.07 + 1.7) * 0.5 + 0.5;
    c += vec3<f32>(0.04, 0.22, 0.28) * w * 0.16;
    w = sin((uv.x + uv.y) * 3.1 + t * 0.05 + 4.1) * 0.5 + 0.5;
    c += vec3<f32>(0.03, 0.16, 0.22) * w * 0.12;
    let br = sin(t * 0.05) * 0.5 + 0.5;
    c += vec3<f32>(0.01, 0.02, 0.03) * br;
    return vec4<f32>(c, 1.0);
}
"#;

pub const AURORA_DRIFT: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = local(pos) / u.res;
    let t = u.time;
    var c = mix(vec3<f32>(0.02, 0.03, 0.07), vec3<f32>(0.05, 0.08, 0.14), uv.y);
    let cx = uv.x + 0.10 * sin(uv.y * 2.0 - t * 0.06);
    let c1 = 0.32 + 0.18 * sin(t * 0.045);
    let g1 = exp(-pow((cx - c1) * 3.2, 2.0));
    c += vec3<f32>(0.10, 0.38, 0.26) * g1 * 0.55;
    let c2 = 0.68 + 0.14 * sin(t * 0.038 + 2.1);
    let g2 = exp(-pow((cx - c2) * 3.8, 2.0));
    c += vec3<f32>(0.26, 0.16, 0.40) * g2 * 0.50;
    let shimmer = sin(uv.y * 5.0 + t * 0.09) * 0.5 + 0.5;
    c += vec3<f32>(0.02, 0.06, 0.05) * g1 * shimmer;
    return vec4<f32>(c, 1.0);
}
"#;

pub const DUSK_EMBER: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = local(pos) / u.res;
    let t = u.time;
    var c = mix(vec3<f32>(0.03, 0.03, 0.08), vec3<f32>(0.10, 0.07, 0.13), uv.y);
    let center = vec2<f32>(0.5 + 0.08 * sin(t * 0.03), 0.22 + 0.05 * sin(t * 0.021 + 1.3));
    let d = length((uv - center) * vec2<f32>(1.0, 1.7));
    let breathe = 0.75 + 0.25 * sin(t * 0.06);
    c += vec3<f32>(0.45, 0.22, 0.10) * exp(-d * d * 7.0) * 0.8 * breathe;
    c += vec3<f32>(0.20, 0.10, 0.12) * exp(-d * d * 2.2) * 0.5 * breathe;
    return vec4<f32>(c, 1.0);
}
"#;

pub const SLOW_RIPPLE: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = local(pos) / u.res;
    let t = u.time;
    var c = mix(vec3<f32>(0.04, 0.05, 0.08), vec3<f32>(0.07, 0.08, 0.12), uv.y);
    let center = vec2<f32>(0.5 + 0.06 * sin(t * 0.02), 0.5 + 0.06 * cos(t * 0.017));
    let d = distance(uv, center) * 1.35;
    let ring = sin(d * 14.0 - t * 0.12) * 0.5 + 0.5;
    let fade = exp(-d * 2.6);
    c += vec3<f32>(0.10, 0.16, 0.22) * ring * fade * 0.5;
    return vec4<f32>(c, 1.0);
}
"#;
