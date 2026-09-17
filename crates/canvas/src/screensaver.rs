//! Retained GPU screensaver for Canvas.
//!
//! The shader renders directly inside iced's render pass. No frame is copied
//! back to the CPU and no per-frame image handle is allocated.

use crate::{Background, BackgroundShader};
use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CALM_WORKSPACE_SHADERS: [BackgroundShader; 4] = [
    BackgroundShader::Tidepool,
    BackgroundShader::AuroraDrift,
    BackgroundShader::DuskEmber,
    BackgroundShader::SlowRipple,
];

const UNIFORM_WGSL: &str = r#"
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

const TIDEPOOL: &str = r#"
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

const AURORA_DRIFT: &str = r#"
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

const DUSK_EMBER: &str = r#"
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

const SLOW_RIPPLE: &str = r#"
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

// "Spirales" by Danilo Guanabara (https://www.shadertoy.com/view/4dS3Wd)
// License: MIT
// Converted from GLSL (ShaderToy) to WGSL for the Canvas screensaver.
const SPIRAL: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var c: vec3<f32>;
    var z: f32 = u.time;
    var l: f32 = 0.0;
    for (var i: i32 = 0; i < 3; i = i + 1) {
        let base: vec2<f32> = local(pos) / u.res;
        var uv: vec2<f32> = base;
        var p: vec2<f32> = base - vec2<f32>(0.5);
        p.x = p.x * u.res.x / u.res.y;
        z = z + 0.07;
        l = length(p);
        uv = uv + p / l * (sin(z) + 1.0) * abs(sin(l * 9.0 - z - z));
        c[i] = 0.01 / length((uv - floor(uv)) - vec2<f32>(0.5));
    }
    return vec4<f32>(c / l, u.time);
}
"#;

// "Floating Rectangles" — ShaderToy background
// License: MIT
// Converted from GLSL to WGSL for the Canvas screensaver.
const FLOATING_RECT: &str = r#"
fn random(co: vec2<f32>) -> f32 {
    return fract(sin(dot(co, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let pp = p * 2.8;
    let i = floor(pp);
    let f = fract(pp);
    let u = f * f * (3.0 - 2.0 * f);
    let r1 = mix(random(i), random(i + vec2<f32>(1.0, 0.0)), u.x);
    let r2 = mix(random(i + vec2<f32>(0.0, 1.0)), random(i + vec2<f32>(1.0, 1.0)), u.x);
    return mix(r1, r2, u.y);
}

fn fbm(uv: vec2<f32>) -> f32 {
    var uvv = uv * 5.0;
    let m = mat2x2<f32>(1.6, 1.2, -1.2, 1.6);
    var f = 0.5 * noise(uvv);
    uvv = m * uvv;
    f = f + 0.25 * noise(uvv);
    uvv = m * uvv;
    f = f + 0.125 * noise(uvv);
    uvv = m * uvv;
    f = f + 0.0625 * noise(uvv);
    return 0.5 + 0.5 * f;
}

fn bg(uv: vec2<f32>, t: f32) -> vec3<f32> {
    let velocity = t / 1.6;
    let intensity = sin(uv.x * 3.0 + velocity * 2.0) * 1.1 + 1.5;
    var uvv = uv;
    uvv.y = uvv.y - 2.0;
    let bp = uvv + vec2<f32>(-2.0, 0.0);
    uvv = uvv * 0.6;
    let rb = fbm(vec2<f32>(uvv.x * 0.5 - velocity * 0.03, uvv.y)) * 0.1;
    uvv = uvv + vec2<f32>(rb, 0.0);
    let rz_noise = fbm(uvv * 0.9 + vec2<f32>(velocity * 0.35, 0.0));
    let rz = rz_noise * (dot(bp * intensity, bp) + 1.2);
    let bgColor = vec3<f32>(0.01, 0.16, 0.42);
    let col = bgColor / (0.1 - rz);
    return sqrt(abs(col));
}

fn rectangle(uv: vec2<f32>, pos: vec2<f32>, w: f32, h: f32, blur: f32) -> f32 {
    var p = (vec2<f32>(w, h) + vec2<f32>(0.01)) / 2.0 - abs(uv - pos);
    p = clamp(p / blur, vec2<f32>(0.0), vec2<f32>(1.0));
    p = p * p * (3.0 - 2.0 * p);
    return p.x * p.y;
}

fn rotate2d(angle: f32) -> mat2x2<f32> {
    return mat2x2<f32>(cos(angle), -sin(angle), sin(angle), cos(angle));
}

@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var uv = local(pos) / u.res * 2.0 - vec2<f32>(1.0);
    uv.x = uv.x * u.res.x / u.res.y;
    let t = u.time;
    let bgColor = vec3<f32>(0.01, 0.16, 0.42);
    var color = bg(uv, t) * (2.0 - abs(uv.y * 2.0));
    let velX = -t / 8.0;
    let velY = t / 10.0;
    let rectColor = vec3<f32>(0.01, 0.26, 0.57);
    let total = 60.0;
    let minSize = 0.03;
    let maxSize = 0.08 - minSize;
    let yDist = 0.5;
    for (var i: f32 = 0.0; i < total; i = i + 1.0) {
        let index = i / total;
        let rnd = random(vec2<f32>(index, 0.0));
        var posX = fract(velX * rnd + index) * 4.0 - 2.0;
        var posY = sin(index * rnd * 1000.0 + velY) * yDist;
        let sz = maxSize * rnd + minSize;
        var uvRot = uv - vec2<f32>(posX, posY) + vec2<f32>(sz / 2.0);
        let r = rotate2d(i + t / 2.0);
        uvRot = r * uvRot;
        uvRot = uvRot + vec2<f32>(posX, posY) + vec2<f32>(sz / 2.0);
        let blur = (maxSize + minSize - sz) / 2.0;
        let rect = rectangle(uvRot, vec2<f32>(posX, posY), sz, sz, blur);
        color = color + rectColor * rect * (sz / maxSize);
    }
    return vec4<f32>(color, 1.0);
}
"#;

// "Smoke Rings" — IQ 2D simplex noise, ShaderToy
// License: MIT
// Source: https://www.shadertoy.com/view/M3XBz4
const SMOKE_RINGS: &str = r#"
fn hash(p: vec2<f32>) -> vec2<f32> {
    let pp = vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3)));
    return -1.0 + 2.0 * fract(sin(pp) * 43758.5453123);
}

fn noise(p: vec2<f32>) -> f32 {
    let K1: f32 = 0.366025404;
    let K2: f32 = 0.211324865;
    let i = floor(p + (p.x + p.y) * K1);
    let a = p - i + (i.x + i.y) * K2;
    let m = step(a.y, a.x);
    let o = vec2<f32>(m, 1.0 - m);
    let b = a - o + K2;
    let c = a - 1.0 + 2.0 * K2;
    let h = max(
        vec3<f32>(0.5) - vec3<f32>(dot(a, a), dot(b, b), dot(c, c)),
        vec3<f32>(0.0),
    );
    let n = h * h * h * h * vec3<f32>(dot(a, hash(i)), dot(b, hash(i + o)), dot(c, hash(i + vec2<f32>(1.0, 1.0))));
    return dot(n, vec3<f32>(70.0));
}

@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = (local(pos) - 0.5 * u.res) / u.res.y;
    var col: vec3<f32> = vec3<f32>(0.0);
    let tt = 0.05 * u.time;
    let pxAng = atan2(uv.y, uv.x);
    let pxRad = length(uv);
    let ringRad = 0.35;
    let smokeFactor = 1.7;
    let phase = pxAng - (6.2831853 * tt) + 3.14159265;
    let dA = abs((phase - 6.2831853 * floor(phase / 6.2831853)) - 3.14159265);
    let totalRingEnv = 1.0 - smoothstep(0.0, smokeFactor, dA);
    let num_rings = 14.0;
    for (var ii: i32 = 0; ii < 14; ii = ii + 1) {
        let i = f32(ii);
        let ringSpread = 0.01 + 0.06 * (i / 10.0);
        let freq = 2.0 + (i - 5.0 * floor(i / 5.0));
        let mainWave = ringSpread * sin((6.2831853 * tt + pxAng) * freq + i * 1.7);
        let secWave = sin(pxRad * 70.0 + (6.2831853 * tt + pxAng) * 2.0 + i);
        let cFlow = vec2<f32>(cos(6.2831853 * tt + pxAng), sin(6.2831853 * tt + pxAng));
        let cTime = vec2<f32>(cos(6.2831853 * tt), sin(6.2831853 * tt));
        let noiseInp = (2.0 + i * 0.2) * cFlow + 0.5 * cTime + vec2<f32>(i * 4.3, i * 9.1);
        let turb = noise(noiseInp);
        let mw = totalRingEnv * 1.0 * mainWave;
        let sw = totalRingEnv * 0.015 * secWave;
        let tb = totalRingEnv * 0.03 * turb;
        let dist = abs(pxRad - (ringRad + mw + sw + tb));
        let radialD = abs(pxRad - ringRad);
        let radialFallOff = exp(-radialD * 9.0);
        col = col + exp(-dist * dist * 1e4) * 0.14 * radialFallOff;
    }
    return vec4<f32>(col, 1.0);
}
"#;

#[derive(Debug)]
pub struct ShaderPiece {
    pub id: BackgroundShader,
    pub name: &'static str,
    pub wgsl: &'static str,
    pub credit: &'static str,
    pub license: &'static str,
}

// "3D Fire" — fragcoord.xyz/s/cx081tgx
// License: as published at the source; confirm before redistribution.
const FLAME: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let frag = local(pos);
    // ShaderToy's fragCoord starts bottom-left and WGSL's position starts
    // top-left, so without the flip the flame burns downward.
    let I = vec2<f32>(frag.x, u.res.y - frag.y);
    // Slowed from the original: at 1x this reads as a strobe rather than fire.
    // The workspace speed still scales on top of this.
    let T = u.time * 0.35;

    var O = vec4<f32>(0.0);
    var z = 0.0;
    var i = 0.0;
    let ray = normalize(vec3<f32>(I + I, 0.0) - vec3<f32>(u.res.x, u.res.y, u.res.y));

    loop {
        if i >= 90.0 { break; }
        i += 1.0;

        var p = z * ray;
        p.z += 9.0;
        let t = p;
        let a = (p.y - length(p.xz)) * 0.5;

        // mat2(cos(a + vec4(0,5,8,0))) — columns, as GLSL builds them.
        let c = cos(a + vec4<f32>(0.0, 5.0, 8.0, 0.0));
        let rotated = p.xz * mat2x2<f32>(c.x, c.y, c.z, c.w);
        p = vec3<f32>(rotated.x, p.y, rotated.y);

        var d = 2.0;
        loop {
            if d >= 4.0 { break; }
            d /= 0.9;
            p += sin(p.yzx * d - vec3<f32>(T, 0.0, 0.0) * 6.0) / d;
        }

        d = min(length(p.xz), 8.0 - abs(p.y)) / 15.0 / (2.0 + cos(a - T));
        z += d;
        // The denominator reaches zero where the ray crosses the axis; the
        // original relies on float infinity there, which `tanh` would fold back
        // to 1.0 but NaN would not.
        let denom = length(t.xz + (p.xz * 0.5 + vec2<f32>(3.0)) * sin(T - a));
        O += vec4<f32>(7.0, 5.0, z, 0.0) * d / max(denom, 1e-4);
    }

    O = tanh(O * O / 1e3);
    return vec4<f32>(O.rgb, 1.0);
}
"#;

// "Meme" — raymarched window lattice, after glslsandbox.com/e#26840.0 for the
// value noise. License: as published at the source; confirm before use.
const MEME: &str = r#"
fn m_hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 458.325421) * 2.0 - 1.0;
}

fn m_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(m_hash(i + vec2<f32>(0.0, 0.0)), m_hash(i + vec2<f32>(1.0, 0.0)), f.x),
        mix(m_hash(i + vec2<f32>(0.0, 1.0)), m_hash(i + vec2<f32>(1.0, 1.0)), f.x),
        f.y);
}

fn m_rot(p: vec2<f32>, a: f32) -> vec2<f32> {
    return vec2<f32>(p.x * cos(a) - p.y * sin(a), p.x * sin(a) + p.y * cos(a));
}

fn m_nac(p: vec3<f32>, f: vec2<f32>, o: vec3<f32>) -> f32 {
    let q = p + o;
    return length(max(abs(q.xy) - f, vec2<f32>(0.0))) - 0.0001;
}

fn m_recta(p: vec3<f32>, f: vec3<f32>, o: vec3<f32>) -> f32 {
    let q = p + o;
    return length(max(abs(q) - f, vec3<f32>(0.0))) - 0.0001;
}

fn m_map1(p: vec3<f32>, scale: f32) -> f32 {
    let g = 0.5;
    let f = 0.5 * scale;
    var t = m_nac(p, vec2<f32>(f, f), vec3<f32>(g, g, 0.0));
    t = min(t, m_nac(p, vec2<f32>(f, f), vec3<f32>(g, -g, 0.0)));
    t = min(t, m_nac(p, vec2<f32>(f, f), vec3<f32>(-g, g, 0.0)));
    t = min(t, m_nac(p, vec2<f32>(f, f), vec3<f32>(-g, -g, 0.0)));
    return t;
}

fn m_map2(p: vec3<f32>) -> f32 {
    return max(m_map1(p, 0.9), m_recta(p, vec3<f32>(1.0, 1.0, 0.02), vec3<f32>(0.0)));
}

fn m_gennoise(p0: vec2<f32>, time: f32) -> f32 {
    var p = p0;
    var d = 0.5;
    let h = mat2x2<f32>(1.6, 1.2, -1.2, 1.6);
    var color = 0.0;
    for (var i = 0; i < 2; i += 1) {
        color += d * m_noise(p * 5.0 + time);
        p = p * h;
        d /= 2.0;
    }
    return color;
}

@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let frag = local(pos);
    let fragcoord = vec2<f32>(frag.x, u.res.y - frag.y);
    // Slowed from the original; the workspace speed scales on top.
    let time = u.time * 0.4;

    let wbcol = vec3<f32>(0.5, 0.7, 1.7);
    let wbcol2 = vec3<f32>(0.15, 0.8, 1.7);
    var acc = vec4<f32>(0.0);

    for (var count = 0; count < 2; count += 1) {
        var uv = (-1.0 + 2.0 * (fragcoord / u.res)) * 1.4;
        uv.x += m_hash(uv.xy + time + f32(count)) / 512.0;
        uv.y += m_hash(uv.yx + time + f32(count)) / 512.0;

        var dir = normalize(vec3<f32>(uv * vec2<f32>(u.res.x / u.res.y, 1.0), 1.0 + sin(time) * 0.01));
        let xz = m_rot(dir.xz, radians(70.0));
        dir = vec3<f32>(xz.x, dir.y, xz.y);
        let xy = m_rot(dir.xy, radians(90.0));
        dir = vec3<f32>(xy.x, xy.y, dir.z);

        let origin = vec3<f32>(-0.1 + sin(time * 0.3) * 0.1, 2.0 + cos(time * 0.4) * 0.1, -3.5);
        var col = vec3<f32>(0.0);
        var t = 0.0;
        let m = 1.002;
        var bsh = 0.01;
        var dens = 0.0;

        for (var i = 0; i < 600; i += 1) {
            if m_map1(origin + dir * t, 0.6) < 0.2 {
                col += wbcol * 0.005 * dens;
            }
            t += bsh * m;
            bsh *= m;
            dens += 0.025;
        }

        t = 0.0;
        var y = 0.0;
        for (var i = 0; i < 25; i += 1) {
            let temp = m_map2(origin + dir * t);
            if temp < 0.025 {
                col += wbcol2 * 0.5;
            }
            t += temp;
            y += 1.0;
        }

        col += ((2.0 + uv.x) * wbcol2) + (y / (25.0 * 50.0));
        col += m_gennoise(dir.xz, time) * 0.5;
        col *= 1.0 - uv.y * 0.5;
        col *= vec3<f32>(0.05);
        col = pow(max(col, vec3<f32>(0.0)), vec3<f32>(0.717));
        acc += vec4<f32>(col, 1.0 / max(t, 1e-4));
    }

    acc /= 2.0;
    return vec4<f32>(acc.rgb, 1.0);
}
"#;

// "Star Nest" by Pablo Roman Andrioli. License: MIT.
const STAR_NEST: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let frag = local(pos);
    let fragcoord = vec2<f32>(frag.x, u.res.y - frag.y);

    var uv = fragcoord / u.res - 0.5;
    uv.y *= u.res.y / u.res.x;
    var dir = vec3<f32>(uv * 0.800, 1.0);
    let time = u.time * 0.010 + 0.25;

    // ShaderToy reports iMouse as zero until the pointer is used, so these are
    // the angles the shader shows on load — the view everyone knows it by.
    let a1 = 0.5;
    let a2 = 0.8;
    let rot1 = mat2x2<f32>(cos(a1), sin(a1), -sin(a1), cos(a1));
    let rot2 = mat2x2<f32>(cos(a2), sin(a2), -sin(a2), cos(a2));

    let dxz = dir.xz * rot1;
    dir = vec3<f32>(dxz.x, dir.y, dxz.y);
    let dxy = dir.xy * rot2;
    dir = vec3<f32>(dxy.x, dxy.y, dir.z);

    var ray_origin = vec3<f32>(1.0, 0.5, 0.5) + vec3<f32>(time * 2.0, time, -2.0);
    let fxz = ray_origin.xz * rot1;
    ray_origin = vec3<f32>(fxz.x, ray_origin.y, fxz.y);
    let fxy = ray_origin.xy * rot2;
    ray_origin = vec3<f32>(fxy.x, fxy.y, ray_origin.z);

    var s = 0.1;
    var fade = 1.0;
    var v = vec3<f32>(0.0);

    for (var r = 0; r < 20; r += 1) {
        var p = ray_origin + s * dir * 0.5;
        // GLSL `mod` floors; WGSL `%` truncates, which differs for negative
        // coordinates — and this fold is fed negatives constantly.
        let period = vec3<f32>(0.850 * 2.0);
        p = abs(vec3<f32>(0.850) - (p - floor(p / period) * period));

        var pa = 0.0;
        var a = 0.0;
        for (var i = 0; i < 17; i += 1) {
            p = abs(p) / max(dot(p, p), 1e-6) - 0.53;
            a += abs(length(p) - pa);
            pa = length(p);
        }

        let dm = max(0.0, 0.300 - a * a * 0.001);
        a = a * a * a;
        if r > 6 { fade *= 1.0 - dm; }
        v += vec3<f32>(fade);
        v += vec3<f32>(s, s * s, s * s * s * s) * a * 0.0015 * fade;
        fade *= 0.730;
        s += 0.1;
    }

    v = mix(vec3<f32>(length(v)), v, 0.850);
    return vec4<f32>(v * 0.01, 1.0);
}
"#;

pub const SHADERS: &[ShaderPiece] = &[
    ShaderPiece {
        id: BackgroundShader::Tidepool,
        name: "Tidepool",
        wgsl: TIDEPOOL,
        credit: "FPL",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::AuroraDrift,
        name: "Aurora Drift",
        wgsl: AURORA_DRIFT,
        credit: "FPL",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::DuskEmber,
        name: "Dusk Ember",
        wgsl: DUSK_EMBER,
        credit: "FPL",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::SlowRipple,
        name: "Slow Ripple",
        wgsl: SLOW_RIPPLE,
        credit: "FPL",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::Spirales,
        name: "Spirales",
        wgsl: SPIRAL,
        credit: "Danilo Guanabara (ShaderToy)",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::FloatingRectangles,
        name: "Floating Rectangles",
        wgsl: FLOATING_RECT,
        credit: "ShaderToy",
        license: "MIT",
    },
    ShaderPiece {
        id: BackgroundShader::SmokeRings,
        name: "Smoke Rings",
        wgsl: SMOKE_RINGS,
        credit: "Sebastien Berube & Noztol (ShaderToy)",
        license: "CC-BY-4.0",
    },
    ShaderPiece {
        id: BackgroundShader::Flame,
        name: "Flame",
        wgsl: FLAME,
        credit: "fragcoord.xyz/s/cx081tgx",
        license: "see source",
    },
    ShaderPiece {
        id: BackgroundShader::Meme,
        name: "Meme",
        wgsl: MEME,
        credit: "glslsandbox.com/e#26840.0 (noise)",
        license: "see source",
    },
    ShaderPiece {
        id: BackgroundShader::StarNest,
        name: "Star Nest",
        wgsl: STAR_NEST,
        credit: "Pablo Roman Andrioli",
        license: "MIT",
    },
];

pub fn attribution(index: usize) -> String {
    let shader = &SHADERS[index % SHADERS.len()];
    debug_assert!(BackgroundShader::ALL.contains(&shader.id));
    format!("{} — {} ({})", shader.name, shader.credit, shader.license)
}

pub fn index_for(id: BackgroundShader) -> usize {
    SHADERS
        .iter()
        .position(|shader| shader.id == id)
        .expect("every typed Canvas shader must have a renderer")
}

/// Resolve one stable privacy visual for a workspace. An explicitly configured
/// shader wins; otherwise the workspace identity chooses from the deliberately
/// calm subset without storing a second preference.
pub fn workspace_index(workspace_id: &str, background: &Background) -> usize {
    if let Background::Shader { shader_id, .. } = background {
        return index_for(*shader_id);
    }
    let hash = workspace_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let shader = CALM_WORKSPACE_SHADERS[hash as usize % CALM_WORKSPACE_SHADERS.len()];
    index_for(shader)
}

pub fn advance_elapsed(
    index: usize,
    elapsed: Duration,
    delta: Duration,
    max_delta: Duration,
) -> (usize, Duration) {
    (index, elapsed + delta.min(max_delta))
}

/// Bounded frame scheduler and rolling cadence meter.
#[derive(Debug, Clone)]
pub struct FrameClock {
    max_fps: u16,
    next_frame: Instant,
    sample_started: Instant,
    sample_frames: u32,
    measured_fps: f32,
}

impl FrameClock {
    pub fn new(max_fps: u16, now: Instant) -> Self {
        Self {
            max_fps,
            next_frame: now,
            sample_started: now,
            sample_frames: 0,
            measured_fps: 0.0,
        }
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.max_fps))
    }

    pub fn set_max_fps(&mut self, max_fps: u16, now: Instant) {
        if self.max_fps != max_fps {
            *self = Self::new(max_fps, now);
        }
    }

    pub fn record_if_due(&mut self, now: Instant) -> bool {
        if now < self.next_frame {
            return false;
        }
        let interval = self.interval();
        while self.next_frame <= now {
            self.next_frame += interval;
        }
        self.sample_frames += 1;
        let elapsed = now.duration_since(self.sample_started);
        if elapsed >= Duration::from_secs(1) {
            self.measured_fps = self.sample_frames as f32 / elapsed.as_secs_f32();
            self.sample_started = now;
            self.sample_frames = 0;
        }
        true
    }

    pub fn measured_fps(&self) -> f32 {
        self.measured_fps
    }
}

/// Where a background widget sits inside the field its shader draws.
///
/// `u.res` is the field and `local(pos)` the coordinate inside it, so this is
/// the whole of the difference between one image stretched across the desk and
/// one copy of it per display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    /// One field across the desk. The widget shows the slice that starts at
    /// `origin` — its window's top-left in desk coordinates — out of a field
    /// `extent` logical units across. Both are logical, so each window scales
    /// the shared field by its own factor and a mixed-DPI desk still joins up.
    Desk { origin: [f32; 2], extent: [f32; 2] },
    /// The widget is its own field, drawn from its own (0, 0). Per-display
    /// tiling asks for this deliberately, and a preview tile in the background
    /// picker has no desk to belong to.
    Widget,
}

#[derive(Debug, Clone, Copy)]
struct BackgroundProgram {
    index: usize,
    elapsed_seconds: f32,
    field: Field,
    redraw_interval: Option<Duration>,
    slot: u64,
}

#[derive(Debug, Default)]
struct BackgroundState {
    index: Option<usize>,
    elapsed_seconds: f32,
    last_redraw: Option<Instant>,
}

const MAX_WIDGET_FRAME_DELTA: Duration = Duration::from_millis(250);

impl BackgroundProgram {
    fn new(index: usize, elapsed: Duration, field: Field) -> Self {
        Self {
            index: index % SHADERS.len(),
            elapsed_seconds: elapsed.as_secs_f32(),
            field,
            redraw_interval: None,
            slot: claim_slot(),
        }
    }

    fn animated(index: usize, elapsed: Duration, field: Field, interval: Duration) -> Self {
        Self {
            redraw_interval: Some(interval),
            ..Self::new(index, elapsed, field)
        }
    }
}

/// A background drawn into `field`.
pub fn view_in<'a, Message: 'a>(
    index: usize,
    elapsed: Duration,
    field: Field,
) -> Element<'a, Message> {
    Shader::new(BackgroundProgram::new(index, elapsed, field))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// An animated background that schedules bounded redraws independently of
/// application data changes. This keeps a clock or stream update from becoming
/// the accidental animation driver while still respecting the frame budget.
pub fn animated_view_in<'a, Message: 'a>(
    index: usize,
    elapsed: Duration,
    field: Field,
    redraw_interval: Duration,
) -> Element<'a, Message> {
    Shader::new(BackgroundProgram::animated(
        index,
        elapsed,
        field,
        redraw_interval,
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// A background that is its own field.
///
/// Anything covering a window wants [`view_in`] with [`Field::Desk`] instead: a
/// window that does not know where it sits on the desk draws the field from its
/// own corner, so three displays show three copies of it rather than one image.
pub fn view<'a, Message: 'a>(index: usize, elapsed: Duration) -> Element<'a, Message> {
    view_in(index, elapsed, Field::Widget)
}

impl<Message> shader::Program<Message> for BackgroundProgram {
    type State = BackgroundState;
    type Primitive = BackgroundPrimitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        let interval = self.redraw_interval?;
        match event {
            iced::Event::Window(iced::window::Event::RedrawRequested(now)) => {
                if state.index != Some(self.index) {
                    state.index = Some(self.index);
                    state.elapsed_seconds = self.elapsed_seconds;
                    state.last_redraw = Some(*now);
                } else {
                    state.elapsed_seconds = state.elapsed_seconds.max(self.elapsed_seconds);
                    if let Some(last) = state.last_redraw {
                        state.elapsed_seconds += now
                            .saturating_duration_since(last)
                            .min(MAX_WIDGET_FRAME_DELTA)
                            .as_secs_f32();
                    }
                    state.last_redraw = Some(*now);
                }
                Some(iced::widget::Action::request_redraw_at(*now + interval))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        BackgroundPrimitive {
            index: self.index,
            elapsed_seconds: state.elapsed_seconds.max(self.elapsed_seconds),
            field: self.field,
            slot: self.slot,
        }
    }
}

/// Bytes between uniform slots.
///
/// wgpu requires a dynamic uniform offset to be a multiple of
/// `min_uniform_buffer_offset_alignment`, whose guaranteed ceiling across
/// backends is 256 — so the four floats each shader needs sit in a 256-byte
/// stride rather than packed.
const UNIFORM_SLOT: u64 = 256;

/// Slots in the shared uniform buffer.
///
/// Sized for the widest frame anyone builds: a full picker grid plus a desktop
/// background on each of sixteen displays. Slots are claimed per widget and
/// recycled, so the ring only has to outlast the widgets alive at once, not the
/// session.
const UNIFORM_SLOTS: u64 = (SHADERS.len() as u64 + 1) * 16;

/// Byte offset of the `ticket`-th widget's uniforms.
const fn slot_offset(ticket: u64) -> u64 {
    (ticket % UNIFORM_SLOTS) * UNIFORM_SLOT
}

/// Claims a uniform slot for one widget instance.
///
/// Slots used to be keyed by shader index, on the assumption that a shader
/// appears at most once on screen. `view` runs once per window, so a second
/// display already breaks it: both primitives write the same slot, whichever
/// window prepares last decides `res` for both, and a picker tile previewing
/// the shader already on the desktop does the same within a single frame. A
/// widget is the thing that owns a slot, so a widget is what claims one.
fn claim_slot() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    slot_offset(NEXT.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy)]
struct BackgroundPrimitive {
    index: usize,
    elapsed_seconds: f32,
    field: Field,
    /// Byte offset into the shared uniform buffer, claimed by the widget this
    /// primitive was drawn from.
    slot: u64,
}

impl BackgroundPrimitive {
    /// The uniform block for one widget: the field it draws, the time, and the
    /// framebuffer coordinate `local(pos)` subtracts to reach field coordinates.
    fn uniforms(&self, bounds: &Rectangle, scale: f32) -> [f32; 8] {
        let (extent, origin) = match self.field {
            // The field is desk space, so a pixel's colour follows where it is
            // on the desk rather than which window drew it — and the widget's
            // own offset inside its window plays no part, because `pos` already
            // starts there.
            Field::Desk { origin, extent } => (extent, [-origin[0], -origin[1]]),
            Field::Widget => ([bounds.width, bounds.height], [bounds.x, bounds.y]),
        };
        [
            // A zero-extent field would divide `uv` by zero in every shader.
            (extent[0] * scale).max(1.0),
            (extent[1] * scale).max(1.0),
            self.elapsed_seconds,
            0.0,
            origin[0] * scale,
            origin[1] * scale,
            0.0,
            0.0,
        ]
    }
}

struct BackgroundPipeline {
    pipelines: Vec<wgpu::RenderPipeline>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl shader::Pipeline for BackgroundPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas screensaver uniforms"),
            // One slot per widget, not one buffer total. iced keeps a single
            // pipeline per primitive type — and one `Storage` for the whole
            // process, so this buffer is shared by every background widget on
            // every window. With a single slot each `prepare` overwrote the
            // last, leaving every tile drawing at whichever tile happened to
            // prepare last. That is invisible with one full-screen background
            // and immediately visible in a grid of previews or on a second
            // display.
            size: UNIFORM_SLOTS * UNIFORM_SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Canvas screensaver bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(32),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas screensaver bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                // Bound as one slot; the offset is chosen per draw.
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(32),
                }),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Canvas screensaver pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipelines = SHADERS
            .iter()
            .map(|piece| {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(piece.name),
                    source: wgpu::ShaderSource::Wgsl(
                        format!("{UNIFORM_WGSL}\n{}", piece.wgsl).into(),
                    ),
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(piece.name),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                })
            })
            .collect();
        Self {
            pipelines,
            uniform_buffer,
            bind_group,
        }
    }
}

impl shader::Primitive for BackgroundPrimitive {
    type Pipeline = BackgroundPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let uniforms = self.uniforms(bounds, viewport.scale_factor());
        queue.write_buffer(&pipeline.uniform_buffer, self.slot, unsafe {
            std::slice::from_raw_parts(uniforms.as_ptr().cast::<u8>(), 32)
        });
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        render_pass.set_pipeline(&pipeline.pipelines[self.index % pipeline.pipelines.len()]);
        render_pass.set_bind_group(0, &pipeline.bind_group, &[self.slot as u32]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn primitive(field: Field) -> BackgroundPrimitive {
        let program = BackgroundProgram::new(0, Duration::from_secs(1), field);
        BackgroundPrimitive {
            index: program.index,
            elapsed_seconds: program.elapsed_seconds,
            field: program.field,
            slot: program.slot,
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    /// What `local(pos) / u.res` computes in every shader, on the CPU.
    fn uv(uniforms: &[f32; 8], pos: [f32; 2]) -> [f32; 2] {
        [
            (pos[0] - uniforms[4]) / uniforms[0],
            (pos[1] - uniforms[5]) / uniforms[1],
        ]
    }

    #[test]
    fn each_widget_instance_claims_its_own_uniform_slot() {
        let field = Field::Desk {
            origin: [0.0, 0.0],
            extent: [1920.0, 1200.0],
        };
        let first = BackgroundProgram::new(3, Duration::ZERO, field);
        let second = BackgroundProgram::new(3, Duration::ZERO, field);
        assert_eq!(first.index, second.index);
        assert_ne!(
            first.slot, second.slot,
            "two windows showing the same shader must not share a uniform slot"
        );
    }

    #[test]
    fn slots_are_aligned_and_a_full_ring_never_repeats() {
        let claimed = (0..UNIFORM_SLOTS).map(slot_offset).collect::<Vec<_>>();
        for slot in &claimed {
            assert_eq!(slot % UNIFORM_SLOT, 0, "dynamic offsets must stay aligned");
            assert!(slot + UNIFORM_SLOT <= UNIFORM_SLOTS * UNIFORM_SLOT);
        }
        assert_eq!(
            claimed.iter().copied().collect::<BTreeSet<_>>().len(),
            UNIFORM_SLOTS as usize
        );
        assert_eq!(slot_offset(UNIFORM_SLOTS), slot_offset(0));
    }

    #[test]
    fn desk_windows_show_slices_of_one_field() {
        let extent = [3840.0, 1200.0];
        let bounds = rect(0.0, 0.0, 1920.0, 1200.0);
        let left = primitive(Field::Desk {
            origin: [0.0, 0.0],
            extent,
        })
        .uniforms(&bounds, 1.0);
        let right = primitive(Field::Desk {
            origin: [1920.0, 0.0],
            extent,
        })
        .uniforms(&bounds, 1.0);

        // Both measure the desk rather than their own window, so neither can
        // overwrite the other's idea of `res`.
        assert_eq!([left[0], left[1]], extent);
        assert_eq!([right[0], right[1]], extent);
        // The seam is continuous: the right window's left edge carries on from
        // the left window's right edge instead of restarting the field.
        assert_eq!(uv(&left, [1920.0, 0.0]), uv(&right, [0.0, 0.0]));
        assert_eq!(uv(&left, [0.0, 0.0]), [0.0, 0.0]);
        assert_eq!(uv(&right, [1920.0, 1200.0]), [1.0, 1.0]);
    }

    #[test]
    fn a_mixed_size_and_mixed_dpi_desk_still_joins_up() {
        // A 1920-wide retina laptop left of a 2560-wide monitor at 1x.
        let extent = [4480.0, 1440.0];
        let laptop = primitive(Field::Desk {
            origin: [0.0, 0.0],
            extent,
        })
        .uniforms(&rect(0.0, 0.0, 1920.0, 1200.0), 2.0);
        let monitor = primitive(Field::Desk {
            origin: [1920.0, 0.0],
            extent,
        })
        .uniforms(&rect(0.0, 0.0, 2560.0, 1440.0), 1.0);

        // `res` is the desk in each window's own physical pixels, so it follows
        // the scale factor and not the window it happens to be drawn in.
        assert_eq!([laptop[0], laptop[1]], [8960.0, 2880.0]);
        assert_eq!([monitor[0], monitor[1]], [4480.0, 1440.0]);
        // The seam still lands at the same point of the field on both sides.
        let seam = uv(&laptop, [1920.0 * 2.0, 0.0])[0];
        assert!((seam - uv(&monitor, [0.0, 0.0])[0]).abs() < 1e-6);
    }

    #[test]
    fn a_widget_field_stays_widget_local() {
        let tile = primitive(Field::Widget).uniforms(&rect(1100.0, 40.0, 220.0, 124.0), 1.0);
        assert_eq!([tile[0], tile[1]], [220.0, 124.0]);
        // A tile far from the framebuffer origin still runs 0..1 over itself.
        assert_eq!(uv(&tile, [1100.0, 40.0]), [0.0, 0.0]);
        assert_eq!(uv(&tile, [1320.0, 164.0]), [1.0, 1.0]);
    }

    #[test]
    fn a_degenerate_field_never_divides_by_zero() {
        let collapsed = primitive(Field::Desk {
            origin: [0.0, 0.0],
            extent: [0.0, 0.0],
        })
        .uniforms(&rect(0.0, 0.0, 0.0, 0.0), 1.0);
        assert_eq!([collapsed[0], collapsed[1]], [1.0, 1.0]);
    }

    #[test]
    fn manifest_is_complete_and_credited() {
        assert_eq!(SHADERS.len(), BackgroundShader::ALL.len());
        assert!(
            BackgroundShader::ALL
                .iter()
                .all(|id| SHADERS.iter().any(|shader| shader.id == *id))
        );
        for (index, shader) in SHADERS.iter().enumerate() {
            assert_eq!(index_for(shader.id), index);
            assert!(!shader.name.is_empty());
            assert!(shader.wgsl.contains("@fragment"));
            assert!(!shader.credit.is_empty());
            assert!(!shader.license.is_empty());
            assert!(attribution(index).contains(shader.credit));
            assert!(attribution(index).contains(shader.license));
        }
    }

    #[test]
    fn every_registered_shader_parses_as_wgsl() {
        for shader in SHADERS {
            let source = format!("{UNIFORM_WGSL}\n{}", shader.wgsl);
            wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|error| panic!("shader {} failed to parse: {error}", shader.name));
        }
    }

    #[test]
    fn active_workspace_never_rotates_shader_family() {
        let cap = Duration::from_secs(10);
        assert_eq!(
            advance_elapsed(0, Duration::ZERO, Duration::from_secs(90), cap),
            (0, cap)
        );
        assert_eq!(
            advance_elapsed(3, Duration::from_secs(30), Duration::from_secs(90), cap),
            (3, Duration::from_secs(40))
        );
    }

    #[test]
    fn workspace_visual_is_deterministic_and_explicit_shader_wins() {
        let background = Background::default();
        let first = workspace_index("ohm", &background);
        assert_eq!(first, workspace_index("ohm", &background));
        assert!(CALM_WORKSPACE_SHADERS.contains(&SHADERS[first].id));

        let explicit = Background::Shader {
            shader_id: BackgroundShader::Spirales,
            speed: 0.25,
        };
        assert_eq!(
            workspace_index("ohm", &explicit),
            index_for(BackgroundShader::Spirales)
        );
    }

    #[test]
    fn elapsed_time_clamps_resume_gaps() {
        let cap = Duration::from_millis(250);
        assert_eq!(
            advance_elapsed(2, Duration::from_secs(1), Duration::from_secs(30), cap),
            (2, Duration::from_millis(1_250))
        );
    }

    #[test]
    fn animated_program_requests_the_next_bounded_frame() {
        let interval = Duration::from_millis(40);
        let program = BackgroundProgram::animated(0, Duration::ZERO, Field::Widget, interval);
        let now = Instant::now();
        let action = <BackgroundProgram as shader::Program<()>>::update(
            &program,
            &mut BackgroundState::default(),
            &iced::Event::Window(iced::window::Event::RedrawRequested(now)),
            Rectangle::default(),
            iced::mouse::Cursor::Unavailable,
        )
        .expect("animated backgrounds schedule another frame");
        let (_, redraw, _) = action.into_inner();
        assert_eq!(redraw, iced::window::RedrawRequest::At(now + interval));
    }

    #[test]
    fn animated_program_advances_time_on_redraw_without_an_app_tick() {
        let interval = Duration::from_millis(40);
        let program = BackgroundProgram::animated(0, Duration::ZERO, Field::Widget, interval);
        let started = Instant::now();
        let mut state = BackgroundState::default();
        let redraw = |state: &mut BackgroundState, at| {
            <BackgroundProgram as shader::Program<()>>::update(
                &program,
                state,
                &iced::Event::Window(iced::window::Event::RedrawRequested(at)),
                Rectangle::default(),
                iced::mouse::Cursor::Unavailable,
            )
        };
        redraw(&mut state, started);
        redraw(&mut state, started + interval);
        let primitive = <BackgroundProgram as shader::Program<()>>::draw(
            &program,
            &state,
            iced::mouse::Cursor::Unavailable,
            Rectangle::default(),
        );
        assert!((primitive.elapsed_seconds - 0.04).abs() < f32::EPSILON);
    }

    #[test]
    fn frame_clock_enforces_cap_and_measures_cadence() {
        let started = Instant::now();
        let mut clock = FrameClock::new(20, started);
        assert!(clock.record_if_due(started));
        assert!(!clock.record_if_due(started + Duration::from_millis(49)));
        for frame in 1..=20 {
            assert!(clock.record_if_due(started + Duration::from_millis(frame * 50)));
        }
        assert!((clock.measured_fps() - 21.0).abs() < 0.01);
    }

    #[test]
    fn changing_cap_resets_the_measurement_window() {
        let started = Instant::now();
        let mut clock = FrameClock::new(30, started);
        assert!(clock.record_if_due(started));
        clock.set_max_fps(10, started + Duration::from_millis(1));
        assert_eq!(clock.interval(), Duration::from_millis(100));
        assert_eq!(clock.measured_fps(), 0.0);
    }
}
