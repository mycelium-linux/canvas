//! Host side of `soil:webgpu@1.0.0`.
//!
//! The host owns the wgpu adapter, device, queue, and every GPU object. A
//! Spore receives Soil-issued u32 handles and typed descriptors. The first
//! linked slice in `webgpu_linker.rs` is buffer/texture/shader/pipeline plus a
//! fullscreen-triangle draw and readback; presentation and resource views are
//! later slices.
//!
//! This module is gated behind `soil-webgpu` so a host that never grants the
//! capability does not carry a second GPU code path.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Mutex;

/// A host-owned wgpu device and the handle table a Spore sees.
pub struct WebGpuHost {
    state: Mutex<GpuState>,
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::AdapterInfo,
    next: u32,
    buffers: HashMap<u32, wgpu::Buffer>,
    textures: HashMap<u32, TextureEntry>,
    shaders: HashMap<u32, wgpu::ShaderModule>,
    pipelines: HashMap<u32, wgpu::RenderPipeline>,
    bind_group_layouts: HashMap<u32, wgpu::BindGroupLayout>,
    bind_groups: HashMap<u32, wgpu::BindGroup>,
    samplers: HashMap<u32, wgpu::Sampler>,
    encoders: HashMap<u32, wgpu::CommandEncoder>,
    passes: HashMap<u32, PassEntry>,
    submission: u64,
}

struct PassEntry {
    encoder: u32,
    texture: u32,
    clear: wgpu::Color,
    commands: Vec<PassCommand>,
}

enum PassCommand {
    SetPipeline(u32),
    SetVertexBuffer {
        slot: u32,
        buffer: u32,
    },
    SetIndexBuffer {
        buffer: u32,
        format: IndexFormat,
    },
    SetBindGroup {
        index: u32,
        group: u32,
    },
    Draw {
        vertices: u32,
        instances: u32,
    },
    DrawIndexed {
        indices: u32,
        base_vertex: u32,
        instances: u32,
    },
}

struct TextureEntry {
    texture: wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferUsage {
    Vertex,
    Index,
    Uniform,
    Storage,
    CopySrc,
    CopyDst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    Depth32Float,
}

impl TextureFormat {
    fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            Self::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            Self::Depth32Float => wgpu::TextureFormat::Depth32Float,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    Float32x2,
    Float32x3,
    Float32x4,
    Uint32,
    Uint32x2,
}

impl VertexFormat {
    fn wgpu(self) -> wgpu::VertexFormat {
        match self {
            Self::Float32x2 => wgpu::VertexFormat::Float32x2,
            Self::Float32x3 => wgpu::VertexFormat::Float32x3,
            Self::Float32x4 => wgpu::VertexFormat::Float32x4,
            Self::Uint32 => wgpu::VertexFormat::Uint32,
            Self::Uint32x2 => wgpu::VertexFormat::Uint32x2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexStep {
    Vertex,
    Instance,
}

#[derive(Clone, Debug)]
pub struct VertexAttribute {
    pub format: VertexFormat,
    pub offset: u64,
    pub shader_location: u32,
}

#[derive(Clone, Debug)]
pub struct VertexBufferLayout {
    pub array_stride: u64,
    pub step_mode: VertexStep,
    pub attributes: Vec<VertexAttribute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    UniformBuffer,
    StorageBuffer,
    Texture,
    Sampler,
}

#[derive(Clone, Debug)]
pub struct BindingLayout {
    pub binding: u32,
    pub visibility: u32,
    pub kind: BindingKind,
}

#[derive(Clone, Debug)]
pub enum BindingResource {
    Buffer(u32),
    Texture(u32),
    Sampler(u32),
}

#[derive(Clone, Debug)]
pub struct BindingEntry {
    pub binding: u32,
    pub resource: BindingResource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerFilter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerAddress {
    ClampToEdge,
    Repeat,
    MirrorRepeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    Uint16,
    Uint32,
}

/// One bounded offscreen render.
pub struct ProbeFrame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row-major.
    pub pixels: Vec<u8>,
}

impl WebGpuHost {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context("no WGPU adapter for soil:webgpu")?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("soil:webgpu"),
            ..Default::default()
        }))
        .context("request WGPU device for soil:webgpu")?;
        Ok(Self {
            state: Mutex::new(GpuState {
                device,
                queue,
                adapter: adapter_info,
                next: 1,
                buffers: HashMap::new(),
                textures: HashMap::new(),
                shaders: HashMap::new(),
                pipelines: HashMap::new(),
                bind_group_layouts: HashMap::new(),
                bind_groups: HashMap::new(),
                samplers: HashMap::new(),
                encoders: HashMap::new(),
                passes: HashMap::new(),
                submission: 0,
            }),
        })
    }

    pub fn adapter_info(&self) -> (String, String, String) {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        (
            state.adapter.name.clone(),
            format!("{:?}", state.adapter.backend),
            format!("{:?}", state.adapter.device_type),
        )
    }

    pub fn create_buffer(&self, size: u64, usages: &[BufferUsage]) -> Result<u32> {
        anyhow::ensure!(
            size > 0 && size <= 256 * 1024 * 1024,
            "buffer size outside 1..=256 MiB"
        );
        let mut usage = wgpu::BufferUsages::empty();
        for request in usages {
            usage |= match request {
                BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
                BufferUsage::Index => wgpu::BufferUsages::INDEX,
                BufferUsage::Uniform => wgpu::BufferUsages::UNIFORM,
                BufferUsage::Storage => wgpu::BufferUsages::STORAGE,
                BufferUsage::CopySrc => wgpu::BufferUsages::COPY_SRC,
                BufferUsage::CopyDst => wgpu::BufferUsages::COPY_DST,
            };
        }
        anyhow::ensure!(!usage.is_empty(), "buffer needs at least one usage");
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("soil-webgpu-buffer"),
            size,
            usage,
            mapped_at_creation: false,
        });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.buffers.insert(handle, buffer);
        Ok(handle)
    }

    pub fn write_buffer(&self, handle: u32, offset: u64, data: &[u8]) -> Result<()> {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let buffer = state
            .buffers
            .get(&handle)
            .context("unknown buffer handle")?;
        anyhow::ensure!(
            offset.saturating_add(data.len() as u64) <= buffer.size(),
            "write exceeds buffer size"
        );
        state.queue.write_buffer(buffer, offset, data);
        Ok(())
    }

    pub fn create_texture(
        &self,
        width: u32,
        height: u32,
        format: TextureFormat,
        render_attachment: bool,
        copy_src: bool,
    ) -> Result<u32> {
        anyhow::ensure!(
            (1..=3840).contains(&width) && (1..=2160).contains(&height),
            "texture size must be 1..=3840 by 1..=2160"
        );
        anyhow::ensure!(
            render_attachment || copy_src,
            "texture needs render-attachment or copy-src"
        );
        let mut usage = wgpu::TextureUsages::empty();
        if render_attachment {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        if copy_src {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let texture = state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("soil-webgpu-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: format.wgpu(),
            usage,
            view_formats: &[],
        });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.textures.insert(
            handle,
            TextureEntry {
                texture,
                format: format.wgpu(),
                width,
                height,
            },
        );
        Ok(handle)
    }

    pub fn create_shader(&self, wgsl: &str) -> Result<u32> {
        anyhow::ensure!(
            !wgsl.is_empty() && wgsl.len() <= 256 * 1024,
            "shader outside 1..=256 KiB"
        );
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let shader = state
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("soil-webgpu-shader"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.shaders.insert(handle, shader);
        Ok(handle)
    }

    pub fn create_pipeline(
        &self,
        shader: u32,
        format: TextureFormat,
        vertex_buffers: &[VertexBufferLayout],
        bind_group_layouts: &[u32],
    ) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let module = state
            .shaders
            .get(&shader)
            .context("unknown shader handle")?;
        let layouts: Vec<&wgpu::BindGroupLayout> = bind_group_layouts
            .iter()
            .map(|handle| {
                state
                    .bind_group_layouts
                    .get(handle)
                    .context("unknown bind-group-layout handle")
            })
            .collect::<Result<_>>()?;
        let pipeline_layout =
            state
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("soil-webgpu-pipeline-layout"),
                    bind_group_layouts: &layouts,
                    push_constant_ranges: &[],
                });
        let attribute_sets: Vec<Vec<wgpu::VertexAttribute>> = vertex_buffers
            .iter()
            .map(|layout| {
                layout
                    .attributes
                    .iter()
                    .map(|attribute| wgpu::VertexAttribute {
                        format: attribute.format.wgpu(),
                        offset: attribute.offset,
                        shader_location: attribute.shader_location,
                    })
                    .collect()
            })
            .collect();
        let vertex_layouts: Vec<wgpu::VertexBufferLayout<'_>> = vertex_buffers
            .iter()
            .zip(&attribute_sets)
            .map(|(layout, attributes)| wgpu::VertexBufferLayout {
                array_stride: layout.array_stride,
                step_mode: match layout.step_mode {
                    VertexStep::Vertex => wgpu::VertexStepMode::Vertex,
                    VertexStep::Instance => wgpu::VertexStepMode::Instance,
                },
                attributes,
            })
            .collect();
        let pipeline = state
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("soil-webgpu-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    buffers: &vertex_layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: format.wgpu(),
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.pipelines.insert(handle, pipeline);
        Ok(handle)
    }

    pub fn create_bind_group_layout(&self, entries: &[BindingLayout]) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let entries: Vec<wgpu::BindGroupLayoutEntry> = entries
            .iter()
            .map(|entry| wgpu::BindGroupLayoutEntry {
                binding: entry.binding,
                visibility: visibility(entry.visibility),
                ty: match entry.kind {
                    BindingKind::UniformBuffer => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    BindingKind::StorageBuffer => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    BindingKind::Texture => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    BindingKind::Sampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                    }
                },
                count: None,
            })
            .collect();
        let layout = state
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("soil-webgpu-bind-group-layout"),
                entries: &entries,
            });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.bind_group_layouts.insert(handle, layout);
        Ok(handle)
    }

    pub fn create_bind_group(&self, layout: u32, entries: &[BindingEntry]) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let bind_group_layout = state
            .bind_group_layouts
            .get(&layout)
            .context("unknown bind-group-layout handle")?
            .clone();
        let mut views: Vec<wgpu::TextureView> = Vec::new();
        let mut texture_slots: Vec<Option<usize>> = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry.resource {
                BindingResource::Texture(handle) => {
                    let texture = state
                        .textures
                        .get(&handle)
                        .context("unknown texture in bind group")?;
                    views.push(
                        texture
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    );
                    texture_slots.push(Some(views.len() - 1));
                }
                _ => texture_slots.push(None),
            }
        }
        let mut bind_group_entries = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let resource = match entry.resource {
                BindingResource::Buffer(handle) => state
                    .buffers
                    .get(&handle)
                    .context("unknown buffer in bind group")?
                    .as_entire_binding(),
                BindingResource::Texture(_) => {
                    wgpu::BindingResource::TextureView(&views[texture_slots[index].unwrap()])
                }
                BindingResource::Sampler(handle) => wgpu::BindingResource::Sampler(
                    state
                        .samplers
                        .get(&handle)
                        .context("unknown sampler in bind group")?,
                ),
            };
            bind_group_entries.push(wgpu::BindGroupEntry {
                binding: entry.binding,
                resource,
            });
        }
        let entries = bind_group_entries;
        let bind_group = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("soil-webgpu-bind-group"),
            layout: &bind_group_layout,
            entries: &entries,
        });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.bind_groups.insert(handle, bind_group);
        Ok(handle)
    }

    pub fn create_sampler(&self, filter: SamplerFilter, address: SamplerAddress) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let filter = match filter {
            SamplerFilter::Nearest => wgpu::FilterMode::Nearest,
            SamplerFilter::Linear => wgpu::FilterMode::Linear,
        };
        let address = match address {
            SamplerAddress::ClampToEdge => wgpu::AddressMode::ClampToEdge,
            SamplerAddress::Repeat => wgpu::AddressMode::Repeat,
            SamplerAddress::MirrorRepeat => wgpu::AddressMode::MirrorRepeat,
        };
        let sampler = state.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("soil-webgpu-sampler"),
            address_mode_u: address,
            address_mode_v: address,
            address_mode_w: address,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.samplers.insert(handle, sampler);
        Ok(handle)
    }

    pub fn create_command_encoder(&self) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("soil-webgpu-encoder"),
            });
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.encoders.insert(handle, encoder);
        Ok(handle)
    }

    pub fn begin_render_pass(&self, encoder: u32, texture: u32, clear: [f32; 4]) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        anyhow::ensure!(
            state.encoders.contains_key(&encoder),
            "unknown encoder handle"
        );
        anyhow::ensure!(
            state.textures.contains_key(&texture),
            "unknown texture handle"
        );
        let handle = state.next;
        state.next = state
            .next
            .checked_add(1)
            .context("soil:webgpu handles exhausted")?;
        state.passes.insert(
            handle,
            PassEntry {
                encoder,
                texture,
                clear: wgpu::Color {
                    r: f64::from(clear[0]),
                    g: f64::from(clear[1]),
                    b: f64::from(clear[2]),
                    a: f64::from(clear[3]),
                },
                commands: Vec::new(),
            },
        );
        Ok(handle)
    }

    fn pass_mut(state: &mut GpuState, pass: u32) -> Result<&mut PassEntry> {
        state.passes.get_mut(&pass).context("unknown pass handle")
    }

    pub fn set_pipeline(&self, pass: u32, pipeline: u32) -> Result<()> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::SetPipeline(pipeline));
        Ok(())
    }

    pub fn set_vertex_buffer(&self, pass: u32, slot: u32, buffer: u32) -> Result<()> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::SetVertexBuffer { slot, buffer });
        Ok(())
    }

    pub fn set_index_buffer(&self, pass: u32, buffer: u32, format: IndexFormat) -> Result<()> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::SetIndexBuffer { buffer, format });
        Ok(())
    }

    pub fn set_bind_group(&self, pass: u32, index: u32, group: u32) -> Result<()> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::SetBindGroup { index, group });
        Ok(())
    }

    pub fn draw(&self, pass: u32, vertices: u32, instances: u32) -> Result<()> {
        anyhow::ensure!(
            vertices > 0 && instances > 0,
            "draw needs vertices and instances"
        );
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::Draw {
                vertices,
                instances,
            });
        Ok(())
    }

    pub fn draw_indexed(
        &self,
        pass: u32,
        indices: u32,
        base_vertex: u32,
        instances: u32,
    ) -> Result<()> {
        anyhow::ensure!(
            indices > 0 && instances > 0,
            "draw needs indices and instances"
        );
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        Self::pass_mut(&mut state, pass)?
            .commands
            .push(PassCommand::DrawIndexed {
                indices,
                base_vertex,
                instances,
            });
        Ok(())
    }

    pub fn end_pass(&self, pass: u32) -> Result<()> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let entry = state.passes.remove(&pass).context("unknown pass handle")?;
        let mut encoder = state
            .encoders
            .remove(&entry.encoder)
            .context("unknown encoder")?;
        let target = state
            .textures
            .get(&entry.texture)
            .context("unknown texture")?;
        let view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("soil-webgpu-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(entry.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for command in &entry.commands {
                match command {
                    PassCommand::SetPipeline(handle) => {
                        let pipeline = state.pipelines.get(handle).context("unknown pipeline")?;
                        render.set_pipeline(pipeline);
                    }
                    PassCommand::SetVertexBuffer { slot, buffer } => {
                        let buffer = state.buffers.get(buffer).context("unknown vertex buffer")?;
                        render.set_vertex_buffer(*slot, buffer.slice(..));
                    }
                    PassCommand::SetIndexBuffer { buffer, format } => {
                        let buffer = state.buffers.get(buffer).context("unknown index buffer")?;
                        render.set_index_buffer(
                            buffer.slice(..),
                            match format {
                                IndexFormat::Uint16 => wgpu::IndexFormat::Uint16,
                                IndexFormat::Uint32 => wgpu::IndexFormat::Uint32,
                            },
                        );
                    }
                    PassCommand::SetBindGroup { index, group } => {
                        let group = state.bind_groups.get(group).context("unknown bind group")?;
                        render.set_bind_group(*index, group, &[]);
                    }
                    PassCommand::Draw {
                        vertices,
                        instances,
                    } => {
                        render.draw(0..*vertices, 0..*instances);
                    }
                    PassCommand::DrawIndexed {
                        indices,
                        base_vertex,
                        instances,
                    } => {
                        render.draw_indexed(
                            0..*indices,
                            i32::try_from(*base_vertex).context("base vertex exceeds i32")?,
                            0..*instances,
                        );
                    }
                }
            }
        }
        state.encoders.insert(entry.encoder, encoder);
        Ok(())
    }

    pub fn submit(&self, encoder: u32) -> Result<u64> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let encoder = state
            .encoders
            .remove(&encoder)
            .context("unknown encoder handle")?;
        state.queue.submit(Some(encoder.finish()));
        state.submission += 1;
        Ok(state.submission)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_mesh(
        &self,
        texture: u32,
        pipeline: u32,
        vertex_buffer: u32,
        index_buffer: Option<u32>,
        index_format: IndexFormat,
        index_count: u32,
        instances: u32,
        bind_groups: &[u32],
    ) -> Result<()> {
        anyhow::ensure!(
            index_count > 0 && instances > 0,
            "draw needs vertices and instances"
        );
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let target = state
            .textures
            .get(&texture)
            .context("unknown texture handle")?;
        let pipeline = state
            .pipelines
            .get(&pipeline)
            .context("unknown pipeline handle")?;
        let vertex_buffer = state
            .buffers
            .get(&vertex_buffer)
            .context("unknown vertex buffer")?;
        let index_buffer = index_buffer
            .map(|handle| state.buffers.get(&handle).context("unknown index buffer"))
            .transpose()?;
        let bind_groups: Vec<&wgpu::BindGroup> = bind_groups
            .iter()
            .map(|handle| state.bind_groups.get(handle).context("unknown bind group"))
            .collect::<Result<_>>()?;
        let view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("soil-webgpu-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("soil-webgpu-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.03,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            for (index, group) in bind_groups.iter().enumerate() {
                pass.set_bind_group(index as u32, *group, &[]);
            }
            if let Some(index_buffer) = index_buffer {
                pass.set_index_buffer(
                    index_buffer.slice(..),
                    match index_format {
                        IndexFormat::Uint16 => wgpu::IndexFormat::Uint16,
                        IndexFormat::Uint32 => wgpu::IndexFormat::Uint32,
                    },
                );
                pass.draw_indexed(0..index_count, 0, 0..instances);
            }
        }
        state.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    pub fn draw_triangle(&self, texture: u32, pipeline: u32) -> Result<()> {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let target = state
            .textures
            .get(&texture)
            .context("unknown texture handle")?;
        let pipeline = state
            .pipelines
            .get(&pipeline)
            .context("unknown pipeline handle")?;
        let view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("soil-webgpu-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("soil-webgpu-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.03,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.draw(0..3, 0..1);
        }
        state.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    pub fn read_pixels(&self, texture: u32) -> Result<Vec<u8>> {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let target = state
            .textures
            .get(&texture)
            .context("unknown texture handle")?;
        let width = target.width;
        let height = target.height;
        let format = target.format;
        let unpadded_row = width as usize * format.block_copy_size(None).unwrap_or(4) as usize;
        let padded_row = unpadded_row.div_ceil(256) * 256;
        let readback = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("soil-webgpu-readback"),
            size: (padded_row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("soil-webgpu-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        state.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        state
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("poll soil:webgpu readback")?;
        receiver
            .recv()
            .context("soil:webgpu mapping callback dropped")?
            .context("map soil:webgpu readback")?;
        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity(unpadded_row * height as usize);
        for row in 0..height as usize {
            let start = row * padded_row;
            pixels.extend_from_slice(&mapped[start..start + unpadded_row]);
        }
        drop(mapped);
        readback.unmap();
        Ok(pixels)
    }

    /// Read a texture and write it as a PNG for a Canvas `live_image` surface.
    /// This is the presentation handoff: the host owns the file and the
    /// surface; a Spore never names a display.
    pub fn write_png(&self, texture: u32, path: &std::path::Path) -> Result<()> {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let target = state
            .textures
            .get(&texture)
            .context("unknown texture handle")?;
        anyhow::ensure!(
            target.format == wgpu::TextureFormat::Rgba8Unorm
                || target.format == wgpu::TextureFormat::Rgba8UnormSrgb,
            "PNG presentation requires an RGBA8 texture"
        );
        drop(state);
        let pixels = self.read_pixels(texture)?;
        let image = image::RgbaImage::from_raw(
            self.texture_size(texture)?.0,
            self.texture_size(texture)?.1,
            pixels,
        )
        .context("RGBA8 length does not match texture size")?;
        image.save(path).context("write PNG presentation frame")?;
        Ok(())
    }

    fn texture_size(&self, texture: u32) -> Result<(u32, u32)> {
        let state = self.state.lock().expect("soil:webgpu state poisoned");
        let target = state
            .textures
            .get(&texture)
            .context("unknown texture handle")?;
        Ok((target.width, target.height))
    }

    pub fn render_probe(&self, width: u32, height: u32) -> Result<ProbeFrame> {
        anyhow::ensure!(
            (1..=3840).contains(&width) && (1..=2160).contains(&height),
            "probe size must be 1..=3840 by 1..=2160"
        );
        let texture =
            self.create_texture(width, height, TextureFormat::Rgba8UnormSrgb, true, true)?;
        let shader = self.create_shader(PROBE_WGSL)?;
        let pipeline = self.create_pipeline(shader, TextureFormat::Rgba8UnormSrgb, &[], &[])?;
        self.draw_triangle(texture, pipeline)?;
        let pixels = self.read_pixels(texture)?;
        Ok(ProbeFrame {
            width,
            height,
            pixels,
        })
    }
}

fn visibility(bits: u32) -> wgpu::ShaderStages {
    let mut stages = wgpu::ShaderStages::empty();
    if bits & 1 != 0 {
        stages |= wgpu::ShaderStages::VERTEX;
    }
    if bits & 2 != 0 {
        stages |= wgpu::ShaderStages::FRAGMENT;
    }
    if bits & 4 != 0 {
        stages |= wgpu::ShaderStages::COMPUTE;
    }
    stages
}

const PROBE_WGSL: &str = r#"
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-0.9, -0.9),
        vec2<f32>( 0.9, -0.9),
        vec2<f32>( 0.0,  0.9),
    );
    let p = positions[index];
    return vec4<f32>(p, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.32, 0.78, 0.52, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_renders_a_bounded_rgba_frame() {
        let Ok(host) = WebGpuHost::new() else {
            eprintln!("no WGPU adapter; skipping soil:webgpu probe test");
            return;
        };
        let frame = host.render_probe(64, 48).unwrap();
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 48);
        assert_eq!(frame.pixels.len(), 64 * 48 * 4);
        let center = (24 * 64 + 32) * 4;
        assert!(
            frame.pixels[center + 1] > frame.pixels[center],
            "triangle green should dominate the centre"
        );
    }

    #[test]
    fn probe_rejects_sizes_outside_the_desktop_ceiling() {
        let Ok(host) = WebGpuHost::new() else {
            return;
        };
        assert!(host.render_probe(0, 48).is_err());
        assert!(host.render_probe(64, 2161).is_err());
    }

    #[test]
    fn presentation_writes_a_png_frame() {
        let Ok(host) = WebGpuHost::new() else {
            return;
        };
        let texture = host
            .create_texture(32, 16, TextureFormat::Rgba8UnormSrgb, true, true)
            .unwrap();
        let shader = host.create_shader(PROBE_WGSL).unwrap();
        let pipeline = host
            .create_pipeline(shader, TextureFormat::Rgba8UnormSrgb, &[], &[])
            .unwrap();
        host.draw_triangle(texture, pipeline).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.png");
        host.write_png(texture, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn handles_are_soil_issued_and_bounded() {
        let Ok(host) = WebGpuHost::new() else {
            return;
        };
        let buffer = host
            .create_buffer(16, &[BufferUsage::Uniform, BufferUsage::CopyDst])
            .unwrap();
        assert!(buffer > 0);
        host.write_buffer(buffer, 0, &[0; 16]).unwrap();
        assert!(host.write_buffer(buffer, 12, &[0; 8]).is_err());
        assert!(host.write_buffer(u32::MAX, 0, &[0]).is_err());
    }
}
