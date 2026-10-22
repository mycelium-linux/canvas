// Packed layout and color/geometry conversion are provided by iced_graphics::gradient::pack.
// Match iced_wgpu 0.14's smoothstep interpolation, premultiplication and fine dither.
struct Packed {
    colors: array<vec4<u32>, 4>,
    offsets: vec4<u32>,
    direction: vec4<f32>,
}
@group(0) @binding(0) var<uniform> gradient: Packed;
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vertex_main(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Vertex(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}
fn unpack_color(value: vec2<u32>) -> vec4<f32> {
    let rg = unpack2x16float(value.x);
    let ba = unpack2x16float(value.y);
    let c = vec4<f32>(rg.y, rg.x, ba.y, ba.x);
    return vec4<f32>(c.rgb * c.a, c.a);
}
@fragment fn bake_main(vertex: Vertex) -> @location(0) vec4<f32> {
    // The retained theme API constructs exactly two stops at 0 and 1.
    let first = unpack_color(gradient.colors[0].xy);
    let last = unpack_color(gradient.colors[0].zw);
    let direction = gradient.direction.zw - gradient.direction.xy;
    let offset = dot(normalize(direction), vertex.position.xy - gradient.direction.xy) / length(direction);
    let noise = fract(sin(dot(vertex.position.xy, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    return mix(first, last, smoothstep(0.0, 1.0, offset)) + mix(-0.3 / 255.0, 0.3 / 255.0, noise);
}
@fragment fn blit_main(vertex: Vertex) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, vertex.uv);
}
