//! A Soil WebGPU guest fixture: vertex buffer, uniform bind group, pipeline,
//! draw, and readback through Soil-issued handles.

wit_bindgen::generate!({
    path: "wit",
    world: "guest",
    generate_all,
});

use soil::webgpu::types::{
    BindingEntry, BindingKind, BindingLayout, BindingResource, BufferUsage, Color,
    TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat, VertexStep,
};
use soil::webgpu::webgpu::{
    begin_render_pass, create_bind_group, create_bind_group_layout, create_buffer,
    create_command_encoder, create_pipeline, create_shader, create_texture, draw, end_pass,
    read_pixels, set_bind_group, set_pipeline, set_vertex_buffer, submit, write_buffer,
};

const TRIANGLE: &str = r#"
struct Color { value: vec4<f32> };
@group(0) @binding(0) var<uniform> colour: Color;

@vertex
fn vs(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return colour.value;
}
"#;

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

struct Component;

impl Guest for Component {
    fn run() -> i64 {
        let Ok(texture) = create_texture(64, 48, TextureFormat::Rgba8UnormSrgb, true, true) else {
            return -1;
        };
        let Ok(vertices) = create_buffer(
            24,
            &[BufferUsage::Vertex, BufferUsage::CopyDst],
        ) else {
            return -2;
        };
        if write_buffer(
            vertices,
            0,
            &f32_bytes(&[-0.9, -0.9, 0.9, -0.9, 0.0, 0.9]),
        )
        .is_err()
        {
            return -3;
        }
        let Ok(uniform) = create_buffer(16, &[BufferUsage::Uniform, BufferUsage::CopyDst]) else {
            return -4;
        };
        if write_buffer(uniform, 0, &f32_bytes(&[0.32, 0.78, 0.52, 1.0])).is_err() {
            return -5;
        }
        let Ok(layout) = create_bind_group_layout(&[BindingLayout {
            binding: 0,
            visibility: 2,
            kind: BindingKind::UniformBuffer,
        }]) else {
            return -6;
        };
        let Ok(group) = create_bind_group(
            layout,
            &[BindingEntry {
                binding: 0,
                value: BindingResource::Buffer(uniform),
            }],
        ) else {
            return -7;
        };
        let Ok(shader) = create_shader(TRIANGLE) else {
            return -8;
        };
        let Ok(pipeline) = create_pipeline(
            shader,
            TextureFormat::Rgba8UnormSrgb,
            &[VertexBufferLayout {
                array_stride: 8,
                step_mode: VertexStep::Vertex,
                attributes: vec![VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 0,
                    location: 0,
                }],
            }],
            &[layout],
        ) else {
            return -9;
        };
        let Ok(encoder) = create_command_encoder() else {
            return -10;
        };
        let Ok(pass) = begin_render_pass(
            encoder,
            texture,
            Color {
                r: 0.02,
                g: 0.03,
                b: 0.05,
                a: 1.0,
            },
        ) else {
            return -11;
        };
        if set_pipeline(pass, pipeline).is_err()
            || set_vertex_buffer(pass, 0, vertices).is_err()
            || set_bind_group(pass, 0, group).is_err()
            || draw(pass, 3, 1).is_err()
            || end_pass(pass).is_err()
            || submit(encoder).is_err()
        {
            return -12;
        }
        let Ok(pixels) = read_pixels(texture) else {
            return -13;
        };
        pixels.iter().map(|byte| i64::from(*byte)).sum()
    }
}

export!(Component);
