struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: vec4<f32>, // time, glow, opacity, line_mode
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};
struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
};

@vertex fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip = uniforms.mvp * vec4<f32>(input.position, 1.0);
    output.world_position = (uniforms.model * vec4<f32>(input.position, 1.0)).xyz;
    output.world_normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);
    return output;
}

@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let view = normalize(vec3<f32>(0.0, 0.0, 1.0));
    let rim = pow(1.0 - abs(dot(normalize(input.world_normal), view)), 2.2);
    let scan = 0.74 + 0.26 * sin(input.world_position.y * 95.0 - uniforms.params.x * 4.5);
    let pulse = 0.88 + 0.12 * sin(uniforms.params.x * 1.7);
    let energy = (0.28 + rim * uniforms.params.y) * scan * pulse;
    let line_boost = mix(1.0, 1.65, uniforms.params.w);
    return vec4<f32>(uniforms.color.rgb * energy * line_boost, uniforms.params.z);
}
