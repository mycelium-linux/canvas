//! Existing ShaderToy ports shared by all Canvas rendering adapters.
//!
//! Shader bodies retain the desktop implementation. Do not add an unverified
//! source to a new shipped ambient rotation.
//
// Star Nest copyright (c) Pablo Roman Andrioli.
// Original: https://www.shadertoy.com/view/XlfGRj
// Original source header: "Star Nest by Pablo Roman Andrioli / License: MIT".
// Corroborating official engine sample:
// https://github.com/defold/sample-shadertoy/blob/22fd1e27889bd481c5ce5fbd3e0673edfc67a050/main/star-nest.fp
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// The MIT notice above applies to STAR_NEST, not to SPIRAL. Spirales has an
// unverified upstream license and is excluded from new ambient rotation.

pub const SPIRAL_CREDIT: &str = "Danilo Guanabara (Creation by Silexars / ShaderToy)";
pub const SPIRAL_LICENSE: &str = "unverified; excluded from ambient rotation";
pub const STAR_NEST_CREDIT: &str = "Pablo Roman Andrioli (Kali / ShaderToy)";
pub const STAR_NEST_LICENSE: &str = "MIT";

// Canvas display name: "Spirales". Upstream: "Creation by Silexars".
// Author: Danilo Guanabara. https://www.shadertoy.com/view/XsXXDn
// The previous 4dS3Wd link referred to an unrelated noise shader. No MIT grant
// was found. Retain the existing desktop source; do not add it to new rotation.
pub const SPIRAL: &str = r#"
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

// "Star Nest" by Pablo Roman Andrioli. License: MIT.
pub const STAR_NEST: &str = r#"
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
