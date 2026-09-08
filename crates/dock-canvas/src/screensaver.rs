//! Retained GPU screensaver for Canvas.
//!
//! The shader renders directly inside iced's render pass. No frame is copied
//! back to the CPU and no per-frame image handle is allocated.

use dock_canvas::BackgroundShader;
use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
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
};
@group(0) @binding(0) var<uniform> u: Uniforms;

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
    let uv = pos.xy / u.res;
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
    let uv = pos.xy / u.res;
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
    let uv = pos.xy / u.res;
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
    let uv = pos.xy / u.res;
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
// Converted from GLSL (ShaderToy) to WGSL for the Dock canvas screensaver.
const SPIRAL: &str = r#"
@fragment
fn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var c: vec3<f32>;
    var z: f32 = u.time;
    var l: f32 = 0.0;
    for (var i: i32 = 0; i < 3; i = i + 1) {
        let base: vec2<f32> = pos.xy / u.res;
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
// Converted from GLSL to WGSL for the Dock canvas screensaver.
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
    var uv = pos.xy / u.res * 2.0 - vec2<f32>(1.0);
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
    let uv = (pos.xy - 0.5 * u.res) / u.res.y;
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
pub fn workspace_index(workspace_id: &str, background: &dock_canvas::Background) -> usize {
    if let dock_canvas::Background::Shader { shader_id, .. } = background {
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

#[derive(Debug, Clone, Copy)]
struct BackgroundProgram {
    index: usize,
    elapsed_seconds: f32,
}

pub fn view<'a, Message: 'a>(index: usize, elapsed: Duration) -> Element<'a, Message> {
    Shader::new(BackgroundProgram {
        index: index % SHADERS.len(),
        elapsed_seconds: elapsed.as_secs_f32(),
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

impl<Message> shader::Program<Message> for BackgroundProgram {
    type State = ();
    type Primitive = BackgroundPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        BackgroundPrimitive {
            index: self.index,
            elapsed_seconds: self.elapsed_seconds,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BackgroundPrimitive {
    index: usize,
    elapsed_seconds: f32,
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
            size: 16,
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
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas screensaver bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
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
        let scale = viewport.scale_factor();
        let uniforms = [
            (bounds.width * scale).max(1.0),
            (bounds.height * scale).max(1.0),
            self.elapsed_seconds,
            0.0,
        ];
        queue.write_buffer(&pipeline.uniform_buffer, 0, unsafe {
            std::slice::from_raw_parts(uniforms.as_ptr().cast::<u8>(), 16)
        });
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        render_pass.set_pipeline(&pipeline.pipelines[self.index % pipeline.pipelines.len()]);
        render_pass.set_bind_group(0, &pipeline.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let background = dock_canvas::Background::default();
        let first = workspace_index("ohm", &background);
        assert_eq!(first, workspace_index("ohm", &background));
        assert!(CALM_WORKSPACE_SHADERS.contains(&SHADERS[first].id));

        let explicit = dock_canvas::Background::Shader {
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
