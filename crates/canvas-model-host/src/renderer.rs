//! Headless WGPU renderer. GPU pipelines are created once; model changes only
//! replace buffers after background baking has completed.

use crate::{
    Material,
    mesh::{PreparedMesh, Vertex},
};
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use canvas_core::VisualTheme;
use glam::{Mat4, Vec3};
use std::sync::mpsc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    color: [f32; 4],
    params: [f32; 4],
    field_params: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    triangles: wgpu::Buffer,
    edges: wgpu::Buffer,
    triangle_count: u32,
    edge_count: u32,
}

struct ReadbackSlot {
    buffer: wgpu::Buffer,
    pending: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    color: wgpu::Texture,
    depth: wgpu::TextureView,
    readbacks: Vec<ReadbackSlot>,
    next_readback: usize,
    padded_row_bytes: u32,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    fill_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    mesh: Option<GpuMesh>,
}

impl Renderer {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        anyhow::ensure!(
            width > 0 && height > 0 && width <= 3840 && height <= 2160,
            "invalid render size"
        );
        pollster::block_on(Self::new_async(width, height))
    }

    async fn new_async(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .context("no WGPU adapter available")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("canvas-model-host"),
                ..Default::default()
            })
            .await
            .context("request WGPU device")?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("model-color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("model-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth = depth_texture.create_view(&Default::default());
        let padded_row_bytes = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        // Interactive model viewing values recency over throughput. One slot
        // means the next submission collects the immediately previous frame;
        // three slots multiplied by several render workers retained a visibly
        // stale queue before encoding even began.
        let readbacks = (0..1)
            .map(|_| ReadbackSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("model-readback"),
                    size: u64::from(padded_row_bytes) * u64::from(height),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                pending: None,
            })
            .collect();
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model-uniforms"),
            contents: bytemuck::bytes_of(&Uniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model-bind-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model-bind-group"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model-hologram-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model-pipeline-layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let make_pipeline = |topology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("model-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 24,
                                shader_location: 2,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let fill_pipeline = make_pipeline(wgpu::PrimitiveTopology::TriangleList);
        let line_pipeline = make_pipeline(wgpu::PrimitiveTopology::LineList);
        Ok(Self {
            device,
            queue,
            width,
            height,
            color,
            depth,
            readbacks,
            next_readback: 0,
            padded_row_bytes,
            uniforms,
            bind_group,
            fill_pipeline,
            line_pipeline,
            mesh: None,
        })
    }

    pub fn upload(&mut self, mesh: &PreparedMesh) {
        self.mesh = Some(GpuMesh {
            vertices: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("model-vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            triangles: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("model-triangles"),
                    contents: bytemuck::cast_slice(&mesh.triangle_indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
            edges: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("model-edges"),
                    contents: bytemuck::cast_slice(&mesh.edge_indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
            triangle_count: mesh.triangle_indices.len() as u32,
            edge_count: mesh.edge_indices.len() as u32,
        });
    }

    /// Submit one frame and return the frame that completed three submissions
    /// earlier. This overlaps render, GPU->CPU transfer, and VideoToolbox input
    /// instead of forcing a Metal map round-trip into every render frame.
    /// `eye_direction` is where the camera sits, already decided.
    ///
    /// The renderer used to sample the flight and apply the orbit itself,
    /// which left no room for a third source of camera motion. A viewer
    /// dragging one is exactly that, and it has to blend with the other two
    /// rather than race them, so the host now settles on a direction and this
    /// draws it.
    pub fn render(
        &mut self,
        eye_direction: Vec3,
        distance_scale: f32,
        model_rotation_degrees: [f32; 3],
        material: &Material,
        field: Option<&crate::Field>,
        theme: &VisualTheme,
        elapsed: f32,
    ) -> Result<Option<Vec<u8>>> {
        let slot_index = self.next_readback;
        let completed = self.collect_slot(slot_index)?;
        let mesh = self.mesh.as_ref().context("no model uploaded")?;
        // A zero or non-finite direction would put the eye on the model and
        // produce a degenerate view matrix; fall back to the front rather than
        // render a frame nobody can interpret.
        let direction = eye_direction.normalize_or_zero();
        let direction = if direction.length_squared() < 0.5 {
            Vec3::Z
        } else {
            direction
        };
        let eye = direction * 3.2 * distance_scale.clamp(0.2, 4.0);
        let up = if direction.dot(Vec3::Y).abs() > 0.98 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, up);
        let projection = Mat4::perspective_rh(
            38f32.to_radians(),
            self.width as f32 / self.height as f32,
            0.05,
            30.0,
        );
        let rotation = Vec3::from_array(model_rotation_degrees.map(f32::to_radians));
        let model = Mat4::from_euler(glam::EulerRot::XYZ, rotation.x, rotation.y, rotation.z);
        let rgb = material
            .color
            .unwrap_or(theme.palette.primary)
            .map(|value| value as f32 / 255.0);
        let uniform = Uniforms {
            mvp: (projection * view * model).to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            color: [rgb[0], rgb[1], rgb[2], 1.0],
            params: [
                elapsed,
                material.glow,
                material.opacity,
                material.wireframe as u8 as f32,
            ],
            // The mesh carries a value per vertex whether or not a field is
            // declared; this is what decides whether it is shown, so the
            // scene can drop the ramp without rebaking.
            field_params: [
                field.is_some() as u8 as f32,
                field.map(|f| f.colormap.shader_index()).unwrap_or(0.0),
                0.0,
                0.0,
            ],
        };
        self.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniform));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("model-frame"),
            });
        {
            let color_view = self.color.create_view(&Default::default());
            let background = theme.palette.background.map(|v| f64::from(v) / 255.0);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background[0],
                            g: background[1],
                            b: background[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
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
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_pipeline(&self.fill_pipeline);
            pass.set_index_buffer(mesh.triangles.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.triangle_count, 0, 0..1);
            if material.wireframe {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_index_buffer(mesh.edges.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.edge_count, 0, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readbacks[slot_index].buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row_bytes),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::sync_channel(1);
        self.readbacks[slot_index]
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.readbacks[slot_index].pending = Some(rx);
        self.next_readback = (slot_index + 1) % self.readbacks.len();
        Ok(completed)
    }

    fn collect_slot(&mut self, slot_index: usize) -> Result<Option<Vec<u8>>> {
        let Some(receiver) = self.readbacks[slot_index].pending.take() else {
            return Ok(None);
        };
        let _ = self.device.poll(wgpu::PollType::Poll);
        match receiver.try_recv() {
            Ok(result) => result?,
            Err(mpsc::TryRecvError::Empty) => {
                self.device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .context("poll WGPU readback")?;
                receiver.recv().context("WGPU mapping callback dropped")??;
            }
            Err(mpsc::TryRecvError::Disconnected) => anyhow::bail!("WGPU mapping callback dropped"),
        }
        let mapped = self.readbacks[slot_index]
            .buffer
            .slice(..)
            .get_mapped_range();
        let mut rgba = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in mapped
            .chunks_exact(self.padded_row_bytes as usize)
            .take(self.height as usize)
        {
            rgba.extend_from_slice(&row[..(self.width * 4) as usize]);
        }
        drop(mapped);
        self.readbacks[slot_index].buffer.unmap();
        Ok(Some(rgba))
    }
}
