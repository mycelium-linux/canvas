//! Actual GPU video-shader contract checks, independent of decoder transport.
use fpl_gfx::retained_video::{Pipeline, Uniforms};
use wgpu::util::DeviceExt;

#[test]
#[ignore = "Requires a native GPU adapter; explicit viewer qualification gate"]
fn native_video_alpha_contain_and_stale_pixels() {
    pollster::block_on(async {
        let adapter = wgpu::Instance::default()
            .request_adapter(&Default::default())
            .await
            .unwrap();
        eprintln!("video qualification adapter: {:?}", adapter.get_info());
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
            width: 2,
            height: 2,
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
            (&frame, &[32, 64, 96, 128].repeat(4)[..], 8),
            (&alpha, &[64; 4][..], 2),
        ] {
            queue.write_texture(
                texture.as_image_copy(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(2),
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
        let pipeline = Pipeline::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let srgb_output = texture(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            output.size(),
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let srgb_pipeline = Pipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        let frame_view = frame.create_view(&Default::default());
        let alpha_view = alpha.create_view(&Default::default());
        let capture_target = |uniforms: Uniforms,
                              output: &wgpu::Texture,
                              pipeline: &Pipeline,
                              background: wgpu::Color| {
            let view = output.create_view(&Default::default());
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &uniforms.to_bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&frame_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
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
                            load: wgpu::LoadOp::Clear(background),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline.render_pipeline);
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
        let capture =
            |uniforms| capture_target(uniforms, &output, &pipeline, wgpu::Color::TRANSPARENT);
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
        let sample = |source: &[[u8; 4]; 4], x: usize, y: usize| -> [f64; 4] {
            let tx = (((x as f64 + 0.5) / 64.0) * 2.0 - 0.5).clamp(0.0, 1.0);
            let ty = (((y as f64 + 0.5) / 64.0) * 2.0 - 0.5).clamp(0.0, 1.0);
            std::array::from_fn(|c| {
                let top = f64::from(source[0][c]) * (1.0 - tx) + f64::from(source[1][c]) * tx;
                let bottom = f64::from(source[2][c]) * (1.0 - tx) + f64::from(source[3][c]) * tx;
                (top * (1.0 - ty) + bottom * ty) / 255.0
            })
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

        let background = wgpu::Color {
            r: 0.1,
            g: 0.2,
            b: 0.3,
            a: 1.0,
        };
        // Independent linear-light compositing oracle, including target encoding.
        let encoded = |linear: f64| {
            if linear <= 0.003_130_8 {
                12.92 * linear
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            }
        };
        let alpha_value = 128.0 / 255.0;
        let linear = [32.0, 64.0, 96.0].map(|c| c / 255.0 * alpha_value);
        let composite = std::array::from_fn::<f64, 3, _>(|i| {
            linear[i] + [0.1, 0.2, 0.3][i] * (1.0 - alpha_value)
        });
        let mut expected_linear = [255; 4];
        let mut expected_srgb = [255; 4];
        for i in 0..3 {
            expected_linear[i] = (composite[i] * 255.0).round() as u8;
            expected_srgb[i] = (encoded(composite[i]) * 255.0).round() as u8;
        }
        near(
            pixel(
                &capture_target(base, &output, &pipeline, background),
                32,
                32,
            ),
            expected_linear,
        );
        near(
            pixel(
                &capture_target(base, &srgb_output, &srgb_pipeline, background),
                32,
                32,
            ),
            expected_srgb,
        );

        // An asymmetric source catches flipped axes and nearest filtering.
        let corners = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255; 4],
        ];
        queue.write_texture(
            frame.as_image_copy(),
            &corners.concat(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(2),
            },
            one,
        );
        let native = Uniforms {
            native_alpha: 1.,
            ..base
        };
        let pixels = capture(native);
        for (x, y, color) in [
            (15, 15, corners[0]),
            (48, 15, corners[1]),
            (15, 48, corners[2]),
            (48, 48, corners[3]),
        ] {
            near(pixel(&pixels, x, y), color);
        }
        for y in 0..64 {
            for x in 0..64 {
                let expected = sample(&corners, x, y).map(|c| (c * 255.0).round() as u8);
                near(pixel(&pixels, x, y), expected);
            }
        }

        // Premultiplied transparent edge: filtering and unpremultiplication
        // must preserve foreground energy instead of applying alpha twice.
        let edges = [
            [0, 0, 0, 0],
            [128, 0, 0, 128],
            [0, 64, 0, 64],
            [0, 0, 255, 255],
        ];
        queue.write_texture(
            frame.as_image_copy(),
            &edges.concat(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(2),
            },
            one,
        );
        let pixels = capture_target(
            Uniforms {
                premultiplied: 1.,
                ..native
            },
            &output,
            &pipeline,
            background,
        );
        for y in 0..64 {
            for x in 0..64 {
                let sample = sample(&edges, x, y);
                let expected = std::array::from_fn(|c| {
                    if c == 3 {
                        255
                    } else {
                        ((sample[c] + [0.1, 0.2, 0.3][c] * (1.0 - sample[3])) * 255.0).round() as u8
                    }
                });
                near(pixel(&pixels, x, y), expected);
            }
        }
    });
}
