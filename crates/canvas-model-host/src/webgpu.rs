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

    pub fn create_pipeline(&self, shader: u32, format: TextureFormat) -> Result<u32> {
        let mut state = self.state.lock().expect("soil:webgpu state poisoned");
        let module = state
            .shaders
            .get(&shader)
            .context("unknown shader handle")?;
        let pipeline = state
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("soil-webgpu-pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    buffers: &[],
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

    pub fn render_probe(&self, width: u32, height: u32) -> Result<ProbeFrame> {
        anyhow::ensure!(
            (1..=3840).contains(&width) && (1..=2160).contains(&height),
            "probe size must be 1..=3840 by 1..=2160"
        );
        let texture =
            self.create_texture(width, height, TextureFormat::Rgba8UnormSrgb, true, true)?;
        let shader = self.create_shader(PROBE_WGSL)?;
        let pipeline = self.create_pipeline(shader, TextureFormat::Rgba8UnormSrgb)?;
        self.draw_triangle(texture, pipeline)?;
        let pixels = self.read_pixels(texture)?;
        Ok(ProbeFrame {
            width,
            height,
            pixels,
        })
    }
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
