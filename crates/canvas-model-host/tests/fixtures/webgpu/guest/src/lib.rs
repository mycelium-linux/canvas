//! A Soil WebGPU guest fixture: build a texture/shader/pipeline, draw a
//! triangle, and return the pixels. The host linker owns the device.

wit_bindgen::generate!({
    path: "wit",
    world: "guest",
    generate_all,
});

use soil::webgpu::webgpu::{
    create_pipeline, create_shader, create_texture, draw_triangle, read_pixels, TextureFormat,
};

const TRIANGLE: &str = r#"
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-0.9, -0.9),
        vec2<f32>( 0.9, -0.9),
        vec2<f32>( 0.0,  0.9),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.32, 0.78, 0.52, 1.0);
}
"#;

struct Component;

impl Guest for Component {
    fn run() -> i64 {
        let Ok(texture) = create_texture(64, 48, TextureFormat::Rgba8UnormSrgb, true, true) else {
            return -1;
        };
        let Ok(shader) = create_shader(TRIANGLE) else {
            return -2;
        };
        let Ok(pipeline) = create_pipeline(shader, TextureFormat::Rgba8UnormSrgb) else {
            return -3;
        };
        if draw_triangle(texture, pipeline).is_err() {
            return -4;
        }
        let Ok(pixels) = read_pixels(texture) else {
            return -5;
        };
        pixels.iter().map(|byte| i64::from(*byte)).sum()
    }
}

export!(Component);
