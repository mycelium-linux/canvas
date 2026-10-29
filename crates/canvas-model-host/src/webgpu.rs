//! Host side of `soil:webgpu@1.0.0`.
//!
//! The first slice is an offscreen render probe: it proves a Soil Spore can
//! ask the host for GPU work through WIT, with the host owning the wgpu device
//! and returning bounded pixels. Resource creation (buffers, textures,
//! pipelines, passes) is the next slice; see `wit/soil-webgpu.wit`.
//!
//! This module is gated behind `soil-webgpu` so a host that never grants the
//! capability does not carry a second GPU code path.

use anyhow::{Context, Result};

/// A host-owned wgpu device used to service Soil WebGPU probes.
pub struct WebGpuHost {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::AdapterInfo,
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
            device,
            queue,
            adapter: adapter_info,
        })
    }

    pub fn adapter_info(&self) -> (String, String, String) {
        (
            self.adapter.name.clone(),
            format!("{:?}", self.adapter.backend),
            format!("{:?}", self.adapter.device_type),
        )
    }

    /// Render a fullscreen triangle and read it back. The result is the
    /// smallest proof that the WIT/host boundary can carry real GPU work.
    pub fn render_probe(&self, width: u32, height: u32) -> Result<ProbeFrame> {
        anyhow::ensure!(
            (1..=3840).contains(&width) && (1..=2160).contains(&height),
            "probe size must be 1..=3840 by 1..=2160"
        );

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("soil-webgpu-probe-target"),
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
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("soil-webgpu-probe-shader"),
                source: wgpu::ShaderSource::Wgsl(PROBE_WGSL.into()),
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("soil-webgpu-probe-pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("soil-webgpu-probe-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("soil-webgpu-probe-pass"),
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
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }

        let unpadded_row = width as usize * 4;
        let padded_row = unpadded_row.div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("soil-webgpu-probe-readback"),
            size: (padded_row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
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
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("poll soil:webgpu probe readback")?;
        receiver
            .recv()
            .context("soil:webgpu probe mapping callback dropped")?
            .context("map soil:webgpu probe readback")?;

        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity(unpadded_row * height as usize);
        for row in 0..height as usize {
            let start = row * padded_row;
            pixels.extend_from_slice(&mapped[start..start + unpadded_row]);
        }
        drop(mapped);
        readback.unmap();

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
}
