//! Retained, two-stop theme ground. Palette and gradient geometry remain iced-owned.
//! Rasterization happens only when packed gradient, physical size or scale changes.
use iced::widget::shader::{self, Shader};
use iced::{Color, Element, Length, Rectangle};
use iced_wgpu::{graphics, wgpu};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct State(u64);
impl Default for State {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
#[derive(Debug, Clone)]
struct Program(iced::gradient::Linear);
#[derive(Debug, Clone)]
struct Primitive {
    ticket: u64,
    gradient: iced::gradient::Linear,
}
impl Default for Primitive {
    fn default() -> Self {
        Self {
            ticket: 0,
            gradient: iced::gradient::Linear::new(0.0),
        }
    }
}

/// The theme's existing two-stop gradient; no palette constants live here.
pub fn view<'a, Message: 'a>(first: Color, last: Color) -> Element<'a, Message> {
    Shader::new(Program(
        iced::gradient::Linear::new(core::f32::consts::FRAC_PI_2)
            .add_stop(0.0, first)
            .add_stop(1.0, last),
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
impl<Message> shader::Program<Message> for Program {
    type State = State;
    type Primitive = Primitive;
    fn draw(&self, state: &State, _: iced::mouse::Cursor, _: Rectangle) -> Primitive {
        Primitive {
            ticket: state.0,
            gradient: self.0,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    pixels: [u32; 2],
    scale: u32,
    packed: Vec<u8>,
}
fn key(gradient: iced::gradient::Linear, bounds: Rectangle, scale: f32) -> Key {
    let pixels = [
        (bounds.width * scale).ceil().max(1.0) as u32,
        (bounds.height * scale).ceil().max(1.0) as u32,
    ];
    let packed = graphics::gradient::pack(
        &gradient.into(),
        Rectangle::new(
            iced::Point::ORIGIN,
            iced::Size::new(bounds.width * scale, bounds.height * scale),
        ),
    );
    Key {
        pixels,
        scale: scale.to_bits(),
        packed: bytemuck::bytes_of(&packed).to_vec(),
    }
}
struct Entry {
    key: Key,
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
    generation: u64,
}
struct Pipeline {
    format: wgpu::TextureFormat,
    bake: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    entries: BTreeMap<u64, Entry>,
    generation: u64,
}
impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retained gradient uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retained gradient image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("retained theme gradient"),
            source: wgpu::ShaderSource::Wgsl(include_str!("retained_gradient.wgsl").into()),
        });
        let make = |layout: &wgpu::BindGroupLayout, entry, blend| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        Self {
            format,
            bake: make(&uniform_layout, "bake_main", None),
            blit: make(
                &texture_layout,
                "blit_main",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            uniform_layout,
            texture_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            entries: BTreeMap::new(),
            generation: 0,
        }
    }
    fn trim(&mut self) {
        self.generation = self.generation.saturating_add(1);
        // iced trims shared storage after EACH window presentation. Give every
        // retained window two full rounds, rather than evicting window 1 when
        // window 3 presents. Capacity below bounds churn from replaced widgets.
        let rounds = 2 * self.entries.len().max(1) as u64;
        let oldest = self.generation.saturating_sub(rounds);
        self.entries.retain(|_, entry| entry.generation >= oldest);
    }
}
impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;
    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let key = key(self.gradient, *bounds, viewport.scale_factor());
        if let Some(entry) = pipeline
            .entries
            .get_mut(&self.ticket)
            .filter(|entry| entry.key == key)
        {
            entry.generation = pipeline.generation;
            return;
        }
        assert!(
            key.pixels
                .iter()
                .all(|&size| size <= device.limits().max_texture_dimension_2d),
            "theme gradient exceeds device texture limits"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cached theme gradient"),
            size: wgpu::Extent3d {
                width: key.pixels[0],
                height: key.pixels[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: pipeline.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        use wgpu::util::DeviceExt;
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("packed iced theme gradient"),
            contents: &key.packed,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("theme gradient cache miss"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline.bake);
            pass.set_bind_group(0, &uniform_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        eprintln!(
            "canvas: theme gradient cache miss {}x{} scale={}",
            key.pixels[0],
            key.pixels[1],
            viewport.scale_factor()
        );
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });
        if pipeline.entries.len() >= 16 && !pipeline.entries.contains_key(&self.ticket) {
            let oldest = *pipeline
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.generation)
                .expect("nonempty cache at capacity")
                .0;
            pipeline.entries.remove(&oldest);
        }
        pipeline.entries.insert(
            self.ticket,
            Entry {
                key,
                bind,
                _texture: texture,
                generation: pipeline.generation,
            },
        );
    }
    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let entry = pipeline
            .entries
            .get(&self.ticket)
            .expect("gradient prepared before drawing");
        pass.set_pipeline(&pipeline.blit);
        pass.set_bind_group(0, &entry.bind, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_tracks_colors_geometry_and_scale_not_position() {
        let g = iced::gradient::Linear::new(core::f32::consts::FRAC_PI_2)
            .add_stop(0.0, Color::BLACK)
            .add_stop(1.0, Color::WHITE);
        let b = Rectangle::new(iced::Point::ORIGIN, iced::Size::new(1920.0, 1080.0));
        let k = key(g, b, 1.0);
        assert!(k == key(g, b, 1.0));
        assert!(k != key(g, b, 2.0));
        assert!(k != key(g.add_stop(1.0, Color::BLACK), b, 1.0));
        assert!(k != key(g, Rectangle { width: 1280.0, ..b }, 1.0));
        assert!(
            k == key(
                g,
                Rectangle {
                    x: 50.0,
                    y: 25.0,
                    ..b
                },
                1.0
            )
        );
        assert_eq!(k.packed.len(), 96);
    }

    /// Explicit opt-in: verifies actual WGSL validation, color parity and cache lifetime.
    #[test]
    #[ignore = "requires a headless Vulkan adapter; run explicitly on Agora"]
    fn gpu_matches_iced_and_reuses_then_releases_texture() {
        for (first, last, scale) in [
            (
                Color::from_rgb8(10, 12, 16),
                Color::from_rgb8(28, 31, 37),
                1.0,
            ),
            (
                Color::from_rgb8(248, 248, 252),
                Color::from_rgb8(220, 230, 240),
                2.0,
            ),
            (
                Color::from_rgb8(0, 100, 255),
                Color::from_rgb8(255, 50, 0),
                1.25,
            ),
        ] {
            gpu_parity(first, last, scale);
        }
    }
    fn gpu_parity(first: Color, last: Color, scale: f32) {
        use iced_wgpu::core::Renderer as _;
        use shader::{Pipeline as _, Primitive as _};
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits {
                max_color_attachments: 1,
                ..wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits())
            },
            ..Default::default()
        }))
        .unwrap();
        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let viewport = graphics::Viewport::with_physical_size(iced::Size::new(256, 128), scale);
        let bounds = Rectangle::new(
            iced::Point::ORIGIN,
            iced::Size::new(256.0 / scale, 128.0 / scale),
        );
        let gradient = iced::gradient::Linear::new(core::f32::consts::FRAC_PI_2)
            .add_stop(0.0, first)
            .add_stop(1.0, last);
        let mut pipeline = Pipeline::new(&device, &queue, format);
        let primitive = Primitive {
            ticket: 1,
            gradient,
        };
        primitive.prepare(&mut pipeline, &device, &queue, &bounds, &viewport);
        let original = pipeline.entries[&1]._texture.clone();
        pipeline.trim();
        primitive.prepare(&mut pipeline, &device, &queue, &bounds, &viewport);
        assert_eq!(
            original, pipeline.entries[&1]._texture,
            "cache hit must reuse allocation"
        );
        let target = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 256,
                    height: 128,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let cached = target();
        let reference = target();
        let view = cached.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            primitive.draw(&pipeline, &mut pass);
        }
        queue.submit([encoder.finish()]);
        let engine = iced_wgpu::Engine::new(
            &adapter,
            device.clone(),
            queue.clone(),
            format,
            None,
            graphics::Shell::headless(),
        );
        let mut renderer =
            iced_wgpu::Renderer::new(engine, iced::Font::DEFAULT, iced::Pixels(16.0));
        renderer.fill_quad(
            iced_wgpu::core::renderer::Quad {
                bounds,
                ..Default::default()
            },
            gradient,
        );
        renderer.present(
            Some(Color::TRANSPARENT),
            format,
            &reference.create_view(&Default::default()),
            &viewport,
        );
        let read = |texture: &wgpu::Texture| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 128 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(1024),
                        rows_per_image: None,
                    },
                },
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            buffer.slice(..).get_mapped_range().to_vec()
        };
        let a = read(&cached);
        let b = read(&reference);
        for y in 1..127 {
            for x in 1..255 {
                let i = (y * 256 + x) * 4;
                for channel in 0..4 {
                    assert!(
                        a[i + channel].abs_diff(b[i + channel]) <= 1,
                        "pixel {x},{y} channel {channel}: {} vs {}",
                        a[i + channel],
                        b[i + channel]
                    );
                }
            }
        }
        for ticket in 2..=4 {
            Primitive {
                ticket,
                ..primitive.clone()
            }
            .prepare(&mut pipeline, &device, &queue, &bounds, &viewport);
            pipeline.trim();
        }
        let textures: BTreeMap<_, _> = pipeline
            .entries
            .iter()
            .map(|(ticket, entry)| (*ticket, entry._texture.clone()))
            .collect();
        for _ in 0..3 {
            for ticket in 1..=4 {
                Primitive {
                    ticket,
                    ..primitive.clone()
                }
                .prepare(&mut pipeline, &device, &queue, &bounds, &viewport);
                pipeline.trim();
                assert_eq!(
                    textures[&ticket], pipeline.entries[&ticket]._texture,
                    "all windows must hit the cache"
                );
            }
        }
        for _ in 0..16 {
            pipeline.trim();
        }
        assert!(
            pipeline.entries.is_empty(),
            "unused windows must release GPU textures"
        );
    }
}
