//! Retained WGPU renderer for Canvas spatial surfaces.
//!
//! The renderer owns pipelines and reconstructible buffers, never a window or
//! swapchain. [`GlobeRenderer::render_into`] encodes directly into a
//! caller-owned texture view, which is the seam used by external GPU buffers.

use bytemuck::{Pod, Zeroable};
use canvas::map::{
    globe::{GlobeFramePlan, geodetic_to_ecef},
    spatial::{SpatialFeatureSet, SpatialGeometry, SpatialPalette},
};
use std::borrow::Cow;
use wgpu::util::DeviceExt;

const EARTH_LATITUDE_SEGMENTS: u32 = 64;
const EARTH_LONGITUDE_SEGMENTS: u32 = 128;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADER: &str = r#"
struct FrameUniforms {
    view_projection: mat4x4<f32>,
    earth: vec4<f32>,
    land: vec4<f32>,
    boundary: vec4<f32>,
    atmosphere: vec4<f32>,
    viewport: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniforms;

struct EarthInput {
    @location(0) position: vec3<f32>,
    @location(1) geo_degrees: vec2<f32>,
};

struct EarthOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) geo_degrees: vec2<f32>,
};

@vertex
fn earth_vertex(input: EarthInput) -> EarthOutput {
    var output: EarthOutput;
    output.position = frame.view_projection * vec4<f32>(input.position, 1.0);
    output.geo_degrees = input.geo_degrees;
    return output;
}

@fragment
fn earth_fragment(input: EarthOutput) -> @location(0) vec4<f32> {
    let latitude_phase = fract((input.geo_degrees.x + 90.0) / 10.0);
    let longitude_phase = fract((input.geo_degrees.y + 180.0) / 10.0);
    let latitude_cell = min(latitude_phase, 1.0 - latitude_phase);
    let longitude_cell = min(longitude_phase, 1.0 - longitude_phase);
    let latitude_width = max(fwidth(latitude_cell), 0.0001);
    let longitude_width = max(fwidth(longitude_cell), 0.0001);
    let latitude_line = 1.0 - smoothstep(0.0, latitude_width * 1.5, latitude_cell);
    let longitude_line = 1.0 - smoothstep(0.0, longitude_width * 1.5, longitude_cell);
    let grid = clamp(max(latitude_line, longitude_line), 0.0, 1.0);
    return mix(frame.land, frame.boundary, grid * frame.boundary.a);
}

struct PointInput {
    @location(0) center: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) size_px: f32,
};

struct PointOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
};

@vertex
fn point_vertex(input: PointInput, @builtin(vertex_index) vertex_index: u32) -> PointOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0)
    );
    let local = corners[vertex_index];
    var clip = frame.view_projection * vec4<f32>(input.center, 1.0);
    let pixel_to_clip = vec2<f32>(2.0 / frame.viewport.x, 2.0 / frame.viewport.y);
    let offset = local * input.size_px * pixel_to_clip * clip.w;
    clip.x += offset.x;
    clip.y += offset.y;
    var output: PointOutput;
    output.position = clip;
    output.color = input.color;
    output.local = local;
    return output;
}

@fragment
fn point_fragment(input: PointOutput) -> @location(0) vec4<f32> {
    let radius = length(input.local);
    let edge = 1.0 - smoothstep(0.76, 1.0, radius);
    if edge <= 0.0 { discard; }
    return vec4<f32>(input.color.rgb, input.color.a * edge);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniforms {
    view_projection: [f32; 16],
    earth: [f32; 4],
    land: [f32; 4],
    boundary: [f32; 4],
    atmosphere: [f32; 4],
    viewport: [f32; 2],
    padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EarthVertex {
    position: [f32; 3],
    geo_degrees: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PointInstance {
    center: [f32; 3],
    color: [f32; 4],
    size_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    pub earth_triangles: u32,
    pub point_instances: u32,
}

#[derive(Debug)]
pub enum RenderError {
    InvalidTarget,
    TooManyPoints,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget => formatter.write_str("external render target has zero size"),
            Self::TooManyPoints => formatter.write_str("spatial point count exceeds u32 capacity"),
        }
    }
}

impl std::error::Error for RenderError {}

struct PointSource {
    ecef_m: [f64; 3],
    style_role: String,
    size_px: f32,
}

struct CanonicalEarthVertex {
    ecef_m: [f64; 3],
    geo_degrees: [f32; 2],
}

struct DepthTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

pub struct GlobeRenderer {
    earth_pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    earth_vertex_buffer: wgpu::Buffer,
    earth_index_buffer: wgpu::Buffer,
    point_instance_buffer: wgpu::Buffer,
    earth_canonical: Vec<CanonicalEarthVertex>,
    earth_relative: Vec<EarthVertex>,
    earth_indices: Vec<u32>,
    point_sources: Vec<PointSource>,
    point_instances: Vec<PointInstance>,
    point_capacity: usize,
    last_origin: Option<[f64; 3]>,
    last_palette: Option<SpatialPalette>,
    depth: Option<DepthTarget>,
}

impl GlobeRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let (earth_canonical, earth_indices) =
            earth_mesh(EARTH_LATITUDE_SEGMENTS, EARTH_LONGITUDE_SEGMENTS);
        let earth_relative = vec![EarthVertex::zeroed(); earth_canonical.len()];
        let earth_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas globe camera-relative earth vertices"),
            size: (earth_relative.len() * std::mem::size_of::<EarthVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let earth_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Canvas globe earth indices"),
            contents: bytemuck::cast_slice(&earth_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let point_capacity = 1;
        let point_instance_buffer = point_buffer(device, point_capacity);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas globe frame uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Canvas globe frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<FrameUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas globe frame bindings"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Canvas globe pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Canvas globe shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let earth_pipeline = earth_pipeline(device, &pipeline_layout, &shader, target_format);
        let point_pipeline = point_pipeline(device, &pipeline_layout, &shader, target_format);

        Self {
            earth_pipeline,
            point_pipeline,
            bind_group,
            uniform_buffer,
            earth_vertex_buffer,
            earth_index_buffer,
            point_instance_buffer,
            earth_canonical,
            earth_relative,
            earth_indices,
            point_sources: Vec::new(),
            point_instances: Vec::new(),
            point_capacity,
            last_origin: None,
            last_palette: None,
            depth: None,
        }
    }

    pub fn set_features(
        &mut self,
        device: &wgpu::Device,
        features: &SpatialFeatureSet,
    ) -> Result<(), RenderError> {
        self.point_sources.clear();
        for feature in &features.features {
            if let SpatialGeometry::Point(position) = feature.geometry {
                let count = feature.measures.get("count").copied().unwrap_or(1.0);
                self.point_sources.push(PointSource {
                    ecef_m: geodetic_to_ecef(
                        position.latitude,
                        position.longitude,
                        position.altitude_m.unwrap_or(0.0),
                    ),
                    style_role: feature.style_role.clone(),
                    size_px: (5.0 + count.max(0.0).sqrt() as f32).clamp(5.0, 24.0),
                });
            }
        }
        if self.point_sources.len() > u32::MAX as usize {
            return Err(RenderError::TooManyPoints);
        }
        if self.point_sources.len() > self.point_capacity {
            self.point_capacity = self.point_sources.len().next_power_of_two();
            self.point_instance_buffer = point_buffer(device, self.point_capacity);
        }
        self.point_instances
            .resize(self.point_sources.len(), PointInstance::zeroed());
        self.last_origin = None;
        Ok(())
    }

    /// Encode into a caller-owned target. The caller owns texture allocation,
    /// external-memory import, synchronization, and queue submission.
    pub fn render_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        plan: &GlobeFramePlan,
    ) -> Result<RenderStats, RenderError> {
        if plan.viewport_px.contains(&0) {
            return Err(RenderError::InvalidTarget);
        }
        self.ensure_depth(device, plan.viewport_px);
        self.update_relative_buffers(queue, plan);
        let uniforms = FrameUniforms {
            view_projection: plan.view_projection,
            earth: plan.palette.earth,
            land: plan.palette.land,
            boundary: plan.palette.boundary,
            atmosphere: plan.palette.atmosphere,
            viewport: [plan.viewport_px[0] as f32, plan.viewport_px[1] as f32],
            padding: [0.0; 2],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let clear = plan.palette.earth;
        let depth = self.depth.as_ref().expect("depth target created");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Canvas globe external-target pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(clear[0]),
                        g: f64::from(clear[1]),
                        b: f64::from(clear[2]),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(&self.earth_pipeline);
        pass.set_vertex_buffer(0, self.earth_vertex_buffer.slice(..));
        pass.set_index_buffer(self.earth_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.earth_indices.len() as u32, 0, 0..1);
        if !self.point_instances.is_empty() {
            pass.set_pipeline(&self.point_pipeline);
            pass.set_vertex_buffer(0, self.point_instance_buffer.slice(..));
            pass.draw(0..6, 0..self.point_instances.len() as u32);
        }

        Ok(RenderStats {
            earth_triangles: self.earth_indices.len() as u32 / 3,
            point_instances: self.point_instances.len() as u32,
        })
    }

    fn ensure_depth(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.depth.as_ref().is_some_and(|depth| depth.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Canvas globe retained depth"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some(DepthTarget {
            _texture: texture,
            view,
            size,
        });
    }

    fn update_relative_buffers(&mut self, queue: &wgpu::Queue, plan: &GlobeFramePlan) {
        if self.last_origin == Some(plan.world_origin_ecef_m)
            && self.last_palette == Some(plan.palette)
        {
            return;
        }
        for (target, source) in self
            .earth_relative
            .iter_mut()
            .zip(self.earth_canonical.iter())
        {
            *target = EarthVertex {
                position: relative_f32(source.ecef_m, plan.world_origin_ecef_m),
                geo_degrees: source.geo_degrees,
            };
        }
        for (target, source) in self.point_instances.iter_mut().zip(&self.point_sources) {
            *target = PointInstance {
                center: relative_f32(source.ecef_m, plan.world_origin_ecef_m),
                color: plan.palette.color_for_role(&source.style_role),
                size_px: source.size_px,
            };
        }
        queue.write_buffer(
            &self.earth_vertex_buffer,
            0,
            bytemuck::cast_slice(&self.earth_relative),
        );
        if !self.point_instances.is_empty() {
            queue.write_buffer(
                &self.point_instance_buffer,
                0,
                bytemuck::cast_slice(&self.point_instances),
            );
        }
        self.last_origin = Some(plan.world_origin_ecef_m);
        self.last_palette = Some(plan.palette);
    }
}

fn earth_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Canvas globe earth pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("earth_vertex"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<EarthVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 12,
                        shader_location: 1,
                    },
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("earth_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(depth_state()),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn point_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Canvas globe instanced point pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("point_vertex"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<PointInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 12,
                        shader_location: 1,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32,
                        offset: 28,
                        shader_location: 2,
                    },
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("point_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(depth_state()),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: true,
        depth_compare: wgpu::CompareFunction::Less,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn point_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Canvas globe retained point instances"),
        size: (capacity.max(1) * std::mem::size_of::<PointInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn earth_mesh(
    latitude_segments: u32,
    longitude_segments: u32,
) -> (Vec<CanonicalEarthVertex>, Vec<u32>) {
    let mut vertices =
        Vec::with_capacity(((latitude_segments + 1) * (longitude_segments + 1)) as usize);
    for latitude_index in 0..=latitude_segments {
        let latitude = -90.0 + 180.0 * f64::from(latitude_index) / f64::from(latitude_segments);
        for longitude_index in 0..=longitude_segments {
            let longitude =
                -180.0 + 360.0 * f64::from(longitude_index) / f64::from(longitude_segments);
            vertices.push(CanonicalEarthVertex {
                ecef_m: geodetic_to_ecef(latitude, longitude, 0.0),
                geo_degrees: [latitude as f32, longitude as f32],
            });
        }
    }
    let mut indices = Vec::with_capacity((latitude_segments * longitude_segments * 6) as usize);
    let stride = longitude_segments + 1;
    for latitude in 0..latitude_segments {
        for longitude in 0..longitude_segments {
            let northwest = latitude * stride + longitude;
            let southwest = (latitude + 1) * stride + longitude;
            indices.extend_from_slice(&[
                northwest,
                northwest + 1,
                southwest,
                northwest + 1,
                southwest + 1,
                southwest,
            ]);
        }
    }
    (vertices, indices)
}

fn relative_f32(position: [f64; 3], origin: [f64; 3]) -> [f32; 3] {
    [
        (position[0] - origin[0]) as f32,
        (position[1] - origin[1]) as f32,
        (position[2] - origin[2]) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::backend::SurfaceMetrics;
    use canvas::map::{
        globe::GlobeCamera,
        spatial::{SpatialPalette, decode_panopticon_snapshot},
    };
    use canvas_core::ThemePalette;

    #[test]
    fn earth_mesh_is_closed_and_indexed() {
        let (vertices, indices) = earth_mesh(8, 16);
        assert_eq!(vertices.len(), 9 * 17);
        assert_eq!(indices.len(), 8 * 16 * 6);
        assert!(
            indices
                .iter()
                .all(|index| (*index as usize) < vertices.len())
        );
        for latitude in 0..=8 {
            let west = vertices[latitude * 17].ecef_m;
            let east = vertices[latitude * 17 + 16].ecef_m;
            for axis in 0..3 {
                assert!((west[axis] - east[axis]).abs() < 0.000_001);
            }
        }
    }

    #[test]
    fn gpu_struct_layouts_match_wgsl_contract() {
        assert_eq!(std::mem::size_of::<FrameUniforms>(), 144);
        assert_eq!(std::mem::size_of::<EarthVertex>(), 20);
        assert_eq!(std::mem::size_of::<PointInstance>(), 32);
    }

    #[test]
    fn camera_relative_conversion_preserves_local_precision() {
        let origin = geodetic_to_ecef(47.6, -122.3, 0.0);
        let elevated = geodetic_to_ecef(47.6, -122.3, 25.0);
        let relative = relative_f32(elevated, origin);
        let distance = (relative[0].powi(2) + relative[1].powi(2) + relative[2].powi(2)).sqrt();
        assert!((distance - 25.0).abs() < 0.01);
    }

    #[test]
    fn validates_pipeline_and_encodes_into_caller_owned_texture() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
                .expect("WGPU adapter required for renderer validation");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("canvas-map-wgpu test"),
                    ..Default::default()
                })
                .await
                .expect("WGPU device");
            device.push_error_scope(wgpu::ErrorFilter::Validation);

            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("caller-owned spatial test target"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut renderer = GlobeRenderer::new(&device, format);
            let features = decode_panopticon_snapshot(include_bytes!(
                "../../canvas/tests/fixtures/spatial_snapshot_v1.json"
            ))
            .unwrap();
            renderer.set_features(&device, &features).unwrap();
            let theme: ThemePalette = serde_json::from_str(
                r##"{
                    "background":"#101014","background_strong":"#08090c",
                    "surface":"#20242c","surface_muted":"#171a20",
                    "text":"#f2f4f8","text_muted":"#9299a6","primary":"#66ccff",
                    "success":"#57d38c","warning":"#f0bd5a","danger":"#f06a73",
                    "info":"#7aa7ff","border":"#343a46","border_strong":"#525b6b"
                }"##,
            )
            .unwrap();
            let plan = GlobeCamera {
                latitude: 47.6,
                longitude: -122.3,
                distance_m: 12_000_000.0,
                bearing_degrees: 0.0,
                pitch_degrees: 0.0,
            }
            .frame_plan(
                SurfaceMetrics {
                    logical_width: 64.0,
                    logical_height: 64.0,
                    scale_factor: 1.0,
                },
                SpatialPalette::from_theme(&theme),
            )
            .unwrap();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("caller-owned spatial test encoder"),
            });
            let stats = renderer
                .render_into(&device, &queue, &mut encoder, &target_view, &plan)
                .unwrap();
            assert!(stats.earth_triangles > 1_000);
            assert_eq!(stats.point_instances, 1);
            queue.submit([encoder.finish()]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll");
            let validation_error = device.pop_error_scope().await;
            assert!(
                validation_error.is_none(),
                "WGPU validation failed: {validation_error:?}"
            );
        });
    }
}
