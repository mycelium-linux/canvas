//! Cheap, deterministic full-screen toys. Pixels stay on the local GPU.
macro_rules! toy {
    ($body:literal) => {
        concat!(r#"
fn reflect_unit(t: f32) -> f32 {
    return 1.0 - abs(1.0 - 2.0 * fract(t));
}
fn moving_center(extent: vec2<f32>, half_size: vec2<f32>, velocity: vec2<f32>) -> vec2<f32> {
    let span = max(extent - half_size * 2.0, vec2<f32>(1.0));
    let phase = (u.time * velocity / span + vec2<f32>(0.37, 0.61)) * 0.5;
    return half_size + span * vec2<f32>(reflect_unit(phase.x), reflect_unit(phase.y));
}
"#, $body)
    };
}

pub const BALL: &str = toy!(r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = min(u.res.x, u.res.y);
    let radius = max(8.0, size * 0.025);
    let center = moving_center(u.res, vec2<f32>(radius + 2.0), size * vec2<f32>(0.44, 0.31));
    let edge = length(local(pos) - center) - radius;
    let ink = 1.0 - smoothstep(-0.8, 0.8, edge);
    return vec4<f32>(vec3<f32>(ink), 1.0);
}
"#);

// Procedural DVD-style lettering and disc emblem; no downloaded texture or
// copyrighted screensaver implementation. Colour changes only on wall hits.
pub const DVD: &str = toy!(r#"
fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    return length(p - a - d * clamp(dot(p - a, d) / dot(d, d), 0.0, 1.0));
}
fn letter_d(p: vec2<f32>) -> f32 {
    let outside = max((length(p / vec2<f32>(0.26, 0.32)) - 1.0) * 0.26, -0.16 - p.x);
    let inside = max((length(p / vec2<f32>(0.15, 0.22)) - 1.0) * 0.15, -0.065 - p.x);
    return max(outside, -inside);
}
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = min(u.res.x, u.res.y);
    let scale = max(24.0, size * 0.085);
    let half_size = scale * vec2<f32>(1.04, 0.70);
    let velocity = size * vec2<f32>(0.15, 0.11);
    let center = moving_center(u.res, half_size, velocity);
    var p = (local(pos) - center) / scale;
    p.x += p.y * 0.24;
    p.y += 0.14;
    var edge = min(letter_d(p - vec2<f32>(-0.62, 0.0)), letter_d(p - vec2<f32>(0.62, 0.0)));
    let v = min(segment_distance(p, vec2<f32>(-0.26,-0.32), vec2<f32>(0.0,0.29)),
                segment_distance(p, vec2<f32>(0.0,0.29), vec2<f32>(0.26,-0.32))) - 0.055;
    edge = min(edge, v);
    let disc = p - vec2<f32>(0.0,0.53);
    let outer = (length(disc / vec2<f32>(0.91,0.115)) - 1.0) * 0.115;
    let hole = (length(disc / vec2<f32>(0.19,0.026)) - 1.0) * 0.026;
    edge = min(edge, max(outer,-hole));
    let span = max(u.res - 2.0 * half_size, vec2<f32>(1.0));
    let travel = u.time * velocity / span + vec2<f32>(0.37,0.61);
    let hits = floor(travel.x) + floor(travel.y);
    let color = 0.55 + 0.45 * cos(vec3<f32>(0.0,2.094,4.188) + hits * 1.618);
    let ink = 1.0 - smoothstep(-0.8 / scale, 0.8 / scale, edge);
    return vec4<f32>(color * ink, 1.0);
}
"#);
