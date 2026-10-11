//! Actual GPU video-shader contract checks, independent of decoder transport.
use fpl_gfx::retained_video::{SHADER, Uniforms};
use wgpu::util::DeviceExt;

#[test]
#[ignore = "Requires a native GPU adapter; explicit viewer qualification gate"]
fn native_video_alpha_contain_and_stale_pixels() {
    pollster::block_on(async {
        let adapter = wgpu::Instance::default()
            .request_adapter(&Default::default())
            .await
            .unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let texture = |format, size, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let one = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };
        let frame = texture(
            wgpu::TextureFormat::Rgba8Unorm,
            one,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let alpha = texture(
            wgpu::TextureFormat::R8Unorm,
            one,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        for (texture, bytes, stride) in [
            (&frame, &[32, 64, 96, 128][..], 4),
            (&alpha, &[64][..], 1),
        ] {
            queue.write_texture(
                texture.as_image_copy(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(1),
                },
                one,
            );
        }
        let output = texture(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&Default::default());
        let frame_view = frame.create_view(&Default::default());
        let alpha_view = alpha.create_view(&Default::default());
        let view = output.create_view(&Default::default());
        let capture = |uniforms: Uniforms| {
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &uniforms.to_bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&frame_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&alpha_view),
                    },
                ],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
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
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(64),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().to_vec();
            readback.unmap();
            bytes
        };
        let pixel = |bytes: &[u8], x: usize, y: usize| {
            <[u8; 4]>::try_from(&bytes[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4]).unwrap()
        };
        let near = |actual: [u8; 4], expected: [u8; 4]| {
            for (a, b) in actual.into_iter().zip(expected) {
                assert!(
                    (i16::from(a) - i16::from(b)).abs() <= 1,
                    "{actual:?} != {expected:?}"
                );
            }
        };
        let base = Uniforms {
            viewport: [64., 64.],
            source: [1., 1.],
            ..Default::default()
        };
        near(pixel(&capture(base), 32, 32), [16, 32, 48, 128]);
        near(
            pixel(
                &capture(Uniforms {
                    premultiplied: 1.,
                    ..base
                }),
                32,
                32,
            ),
            [32, 64, 96, 128],
        );
        near(
            pixel(
                &capture(Uniforms {
                    has_alpha: 1.,
                    ..base
                }),
                32,
                32,
            ),
            [4, 8, 12, 32],
        );
        near(
            pixel(
                &capture(Uniforms {
                    has_alpha: 1.,
                    native_alpha: 1.,
                    ..base
                }),
                32,
                32,
            ),
            [16, 32, 48, 128],
        );
        near(
            pixel(
                &capture(Uniforms {
                    staleness: 1.,
                    ..base
                }),
                32,
                32,
            ),
            [6, 11, 17, 128],
        );
        let wide = capture(Uniforms {
            source: [2., 1.],
            ..base
        });
        let tall = capture(Uniforms {
            source: [1., 2.],
            ..base
        });
        assert_eq!(pixel(&wide, 32, 8), [0; 4]);
        assert_eq!(pixel(&tall, 8, 32), [0; 4]);
        near(pixel(&wide, 32, 32), [16, 32, 48, 128]);
        near(pixel(&tall, 32, 32), [16, 32, 48, 128]);
    });
}
