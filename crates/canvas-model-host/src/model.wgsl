struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: vec4<f32>,       // time, glow, opacity, line_mode
    field_params: vec4<f32>, // painted, colormap index, unused, unused
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Already normalised to 0..=1 on the CPU, so the shader never needs the
    // field's unit or range.
    @location(2) field: f32,
};
struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) field: f32,
};

@vertex fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip = uniforms.mvp * vec4<f32>(input.position, 1.0);
    output.world_position = (uniforms.model * vec4<f32>(input.position, 1.0)).xyz;
    output.world_normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);
    // Interpolated across the triangle, which is the whole trick: three vertex
    // samples become a smooth gradient over the face for free.
    output.field = input.field;
    return output;
}


// Perceptually ordered ramps, as six stops interpolated in sRGB.
//
// Stops rather than a polynomial fit because they are readable and checkable
// against the reference ramps. All three rise monotonically in lightness so two
// points can be ranked by eye; a rainbow ramp is deliberately absent, since its
// banding reads as structure that is not in the data.
fn ramp(t: f32, c0: vec3<f32>, c1: vec3<f32>, c2: vec3<f32>,
        c3: vec3<f32>, c4: vec3<f32>, c5: vec3<f32>) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 5.0;
    let i = floor(x);
    let f = x - i;
    if (i < 1.0) { return mix(c0, c1, f); }
    if (i < 2.0) { return mix(c1, c2, f); }
    if (i < 3.0) { return mix(c2, c3, f); }
    if (i < 4.0) { return mix(c3, c4, f); }
    return mix(c4, c5, f);
}

fn colormap(t: f32, which: f32) -> vec3<f32> {
    if (which < 0.5) {
        // inferno
        return ramp(t,
            vec3<f32>(0.001, 0.000, 0.014), vec3<f32>(0.259, 0.039, 0.408),
            vec3<f32>(0.576, 0.149, 0.404), vec3<f32>(0.867, 0.318, 0.227),
            vec3<f32>(0.988, 0.647, 0.039), vec3<f32>(0.988, 1.000, 0.644));
    }
    if (which < 1.5) {
        // viridis
        return ramp(t,
            vec3<f32>(0.267, 0.005, 0.329), vec3<f32>(0.283, 0.141, 0.458),
            vec3<f32>(0.254, 0.265, 0.530), vec3<f32>(0.164, 0.471, 0.558),
            vec3<f32>(0.478, 0.821, 0.318), vec3<f32>(0.993, 0.906, 0.144));
    }
    // magma
    return ramp(t,
        vec3<f32>(0.001, 0.000, 0.014), vec3<f32>(0.232, 0.059, 0.437),
        vec3<f32>(0.550, 0.161, 0.505), vec3<f32>(0.868, 0.288, 0.409),
        vec3<f32>(0.996, 0.624, 0.427), vec3<f32>(0.987, 0.991, 0.750));
}

@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.field < -0.5) {
        return vec4<f32>(0.38, 0.41, 0.46, 0.42);
    }
    let normal = normalize(input.world_normal);
    let key = max(dot(normal, normalize(vec3<f32>(0.55, 0.8, 0.45))), 0.0);
    let fill = max(dot(normal, normalize(vec3<f32>(-0.65, 0.25, -0.35))), 0.0);
    let energy = 0.24 + key * 0.62 + fill * 0.22;
    let line_boost = mix(1.0, 1.65, uniforms.params.w);
    // When a field is painted the ramp carries the meaning, so it replaces the
    // scene colour outright — tinting it would make two different values look
    // the same. The rim and scan stay, at reduced strength, so the part still
    // reads as a hologram without the lighting distorting the reading.
    let painted = uniforms.field_params.x;
    let ramp_color = colormap(input.field, uniforms.field_params.y);
    let base = mix(uniforms.color.rgb * energy, ramp_color * energy, painted);
    return vec4<f32>(base * line_boost, uniforms.params.z);
}
