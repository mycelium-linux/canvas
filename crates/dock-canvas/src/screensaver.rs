//! Retained GPU screensaver for Canvas.
//!
//! The shader renders directly inside iced's render pass. No frame is copied
//! back to the CPU and no per-frame image handle is allocated.

use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use std::time::{Duration, Instant};

pub const ROTATION_PERIOD: Duration = Duration::from_secs(45);
pub const CREDIT: &str = "Danilo Guanabara (ShaderToy) / FPL";
pub const LICENSE: &str = "MIT";

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
        c[i] = 0.01 / length(mod(uv, vec2<f32>(1.0)) - vec2<f32>(0.5));
    }
    return vec4<f32>(c / l, u.time);
}
"#;

#[derive(Debug)]
pub struct ShaderPiece {
    pub name: &'static str,
    pub wgsl: &'static str,
}

pub const SHADERS: &[ShaderPiece] = &[
    ShaderPiece {
        name: "Tidepool",
        wgsl: TIDEPOOL,
    },
    ShaderPiece {
        name: "Aurora Drift",
        wgsl: AURORA_DRIFT,
    },
    ShaderPiece {
        name: "Dusk Ember",
        wgsl: DUSK_EMBER,
    },
    ShaderPiece {
        name: "Slow Ripple",
        wgsl: SLOW_RIPPLE,
    },
    ShaderPiece {
        name: "Spirales",
        wgsl: SPIRAL,
    },
];

pub fn attribution(index: usize) -> String {
    let shader = &SHADERS[index % SHADERS.len()];
    format!("{} — {} ({})", shader.name, CREDIT, LICENSE)
}

pub fn advance(index: usize, piece_started: Instant, now: Instant) -> (usize, Instant) {
    let mut index = index;
    let mut piece_started = piece_started;
    while now.duration_since(piece_started) >= ROTATION_PERIOD {
        index = (index + 1) % SHADERS.len();
        piece_started += ROTATION_PERIOD;
    }
    (index, piece_started)
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
        assert_eq!(SHADERS.len(), 4);
        for (index, shader) in SHADERS.iter().enumerate() {
            assert!(!shader.name.is_empty());
            assert!(shader.wgsl.contains("@fragment"));
            assert!(attribution(index).contains(CREDIT));
            assert!(attribution(index).contains(LICENSE));
        }
    }

    #[test]
    fn rotation_advances_only_at_period_boundaries() {
        let started = Instant::now();
        assert_eq!(advance(0, started, started + ROTATION_PERIOD / 2).0, 0);
        assert_eq!(advance(0, started, started + ROTATION_PERIOD).0, 1);
        assert_eq!(advance(0, started, started + ROTATION_PERIOD * 5).0, 1);
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
