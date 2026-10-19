//! GLES adapter for canonical WGSL. No downloaded shader execution or second catalog.
use canvas_workspace::{BackgroundShader, background_shaders};

pub fn program(id: &str) -> Result<serde_json::Value, String> {
    if id == "ambient-rotation" {
        let mut body = String::new();
        const ENTRY: &str =
            "@fragment\nfn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32>";
        for (index, shader) in AMBIENT_SHADERS.iter().enumerate() {
            if !background_shaders::ambient_redistributable(*shader) {
                return Err("Ambient shader license is not cleared".into());
            }
            let source =
                background_shaders::source(*shader).ok_or("Ambient shader is unavailable")?;
            if !source.contains(ENTRY) {
                return Err("Ambient shader entry contract changed".into());
            }
            body.push_str(&source.replacen(
                ENTRY,
                &format!("fn ambient_{index}(pos: vec4<f32>) -> vec4<f32>"),
                1,
            ));
        }
        body.push_str(
            "\nfn ambient_sample(index: i32, pos: vec4<f32>) -> vec3<f32> {\nswitch index {\n",
        );
        for index in 0..AMBIENT_SHADERS.len() {
            body.push_str(&format!("case {index}: {{ return clamp(ambient_{index}(pos).xyz,vec3<f32>(0.0),vec3<f32>(1.0)); }}\n"));
        }
        body.push_str("default: { return vec3<f32>(0.0); }\n}\n}\n@fragment\nfn main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {\nlet outgoing=ambient_sample(i32(u._pad2.x),pos);\nif u._pad<=0.0 { return vec4<f32>(outgoing,1.0); }\nlet incoming=ambient_sample(i32(u._pad2.y),pos);\nreturn vec4<f32>(mix(outgoing,incoming,u._pad),1.0);\n}\n");
        let mut program = compile(&body)?;
        program["ambient"] = serde_json::json!(true);
        program["shaders"] = serde_json::json!(AMBIENT_SHADERS);
        program["rotation_ms"] = serde_json::json!(ROTATION_MS);
        program["crossfade_ms"] = serde_json::json!(CROSSFADE_MS);
        program["max_fps"] = serde_json::json!(20);
        program["max_width"] = serde_json::json!(640);
        program["max_height"] = serde_json::json!(360);
        return Ok(program);
    }
    let shader: BackgroundShader = serde_json::from_value(serde_json::json!(id))
        .map_err(|_| format!("Unknown background shader: {id}"))?;
    if !background_shaders::ambient_redistributable(shader) {
        return Err(format!("Fire TV shader license is not cleared: {id}"));
    }
    let body = background_shaders::source(shader)
        .ok_or_else(|| format!("Fire TV does not yet support shader: {id}"))?;
    compile(body)
}

pub const ROTATION_MS: u64 = 300_000;
pub const CROSSFADE_MS: u64 = 4_000;
pub const AMBIENT_SHADERS: [BackgroundShader; 5] = [
    BackgroundShader::StarNest,
    BackgroundShader::AuroraDrift,
    BackgroundShader::SlowRipple,
    BackgroundShader::Tidepool,
    BackgroundShader::DuskEmber,
];

/// Monotonic absolute phase: no timer drift or catch-up loops after a stall.
pub fn ambient_frame(elapsed_ms: u64) -> [f32; 3] {
    let cycle = elapsed_ms / ROTATION_MS;
    let phase = elapsed_ms % ROTATION_MS;
    let next = (cycle % AMBIENT_SHADERS.len() as u64) as f32;
    if cycle == 0 || phase >= CROSSFADE_MS {
        return [next, next, 0.0];
    }
    let previous = ((cycle - 1) % AMBIENT_SHADERS.len() as u64) as f32;
    let t = phase as f32 / CROSSFADE_MS as f32;
    [previous, next, t * t * (3.0 - 2.0 * t)]
}

/// Adapt the render buffer, not the display. Stable scenes own quality changes;
/// never resize mid-crossfade. A heavy shader gets a bounded 16:9 raster.
pub fn ambient_raster_width(current: u32, mean_work_ms: f32, transitioning: bool) -> u32 {
    if transitioning
        || !(320..=640).contains(&current)
        || !mean_work_ms.is_finite()
        || mean_work_ms < 0.0
    {
        return current;
    }
    if mean_work_ms > 60.0 {
        let desired = (current as f32 * (40.0 / mean_work_ms).sqrt() / 16.0).floor() as u32 * 16;
        return desired.clamp(320, 640).min(current);
    }
    if mean_work_ms < 15.0 {
        return 640;
    }
    current
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeAmbientRasterWidth(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    work: jni::sys::jfloat,
    transitioning: jni::sys::jboolean,
) -> jni::sys::jint {
    ambient_raster_width(width.max(0) as u32, work, transitioning != 0) as i32
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeAmbientFrame(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    elapsed: jni::sys::jlong,
) -> jni::sys::jfloatArray {
    let Ok(elapsed) = u64::try_from(elapsed) else {
        return std::ptr::null_mut();
    };
    let Ok(array) = env.new_float_array(3) else {
        return std::ptr::null_mut();
    };
    if env
        .set_float_array_region(&array, 0, &ambient_frame(elapsed))
        .is_err()
    {
        return std::ptr::null_mut();
    }
    array.into_raw()
}

fn compile(body: &str) -> Result<serde_json::Value, String> {
    // Platform color adaptation uses the shared semantic background token.
    // GLES fragment coordinates start at the bottom; Canvas/WGSL starts at top.
    let uniforms = background_shaders::UNIFORM_WGSL
        .replace(
            "_pad2: vec2<f32>,",
            "_pad2: vec2<f32>,\n    theme: vec4<f32>,",
        )
        .replace(
            "return pos.xy - u.origin;",
            "return vec2<f32>(pos.x, u.res.y - pos.y) - u.origin;",
        );
    let body = body.replace(
        "vec4<f32>(c, 1.0)",
        "vec4<f32>(mix(c, u.theme.xyz, 0.18), 1.0)",
    );
    let source = format!("{uniforms}\n{body}");
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| e.to_string())?;
    let compile = |stage, entry_point: &str| -> Result<String, String> {
        let mut output = String::new();
        let mut options = naga::back::glsl::Options::default();
        options.binding_map.insert(
            naga::ResourceBinding {
                group: 0,
                binding: 0,
            },
            0,
        );
        let pipeline = naga::back::glsl::PipelineOptions {
            shader_stage: stage,
            entry_point: entry_point.into(),
            multiview: None,
        };
        naga::back::glsl::Writer::new(
            &mut output,
            &module,
            &info,
            &options,
            &pipeline,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .map_err(|e| e.to_string())?
        .write()
        .map_err(|e| e.to_string())?;
        Ok(output)
    };
    Ok(serde_json::json!({
        "vertex": compile(naga::ShaderStage::Vertex, "vs_main")?,
        "fragment": compile(naga::ShaderStage::Fragment, "main")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calm_catalog_translates_to_gles() {
        for id in [
            "tidepool",
            "aurora-drift",
            "dusk-ember",
            "slow-ripple",
            "cat-toy-ball",
            "dvd-bounce",
        ] {
            let program = program(id).unwrap();
            assert!(
                program["vertex"]
                    .as_str()
                    .unwrap()
                    .contains("#version 310 es")
            );
            assert!(
                program["fragment"]
                    .as_str()
                    .unwrap()
                    .contains("binding = 0")
            );
        }
        assert!(program("downloaded-code").is_err());
        assert!(program("spirales").is_err());
        assert!(program("star-nest").is_ok());
    }

    #[test]
    fn ambient_rotation_blends_at_exact_five_minute_boundaries() {
        assert_eq!(ambient_frame(0), [0.0, 0.0, 0.0]);
        assert_eq!(ambient_frame(299_999), [0.0, 0.0, 0.0]);
        assert_eq!(ambient_frame(300_000), [0.0, 1.0, 0.0]);
        assert_eq!(ambient_frame(302_000), [0.0, 1.0, 0.5]);
        assert_eq!(ambient_frame(304_000), [1.0, 1.0, 0.0]);
        assert_eq!(ambient_frame(1_500_000), [4.0, 0.0, 0.0]);
        assert_eq!(ambient_frame(u64::MAX)[2], 0.0);
    }

    #[test]
    fn ambient_program_is_local_and_valid_gles() {
        let program = program("ambient-rotation").unwrap();
        assert_eq!(program["rotation_ms"], 300000);
        assert_eq!(program["crossfade_ms"], 4000);
        assert_eq!(program["shaders"].as_array().unwrap().len(), 5);
        assert!(
            program["fragment"]
                .as_str()
                .unwrap()
                .contains("ambient_sample")
        );
        assert!(program["max_fps"].as_u64().unwrap() <= 30);
    }

    #[test]
    fn ambient_quality_responds_to_work_without_resizing_a_crossfade() {
        assert_eq!(ambient_raster_width(640, 165.0, false), 320);
        assert_eq!(ambient_raster_width(320, 41.0, false), 320);
        assert_eq!(ambient_raster_width(320, 1.0, false), 640);
        assert_eq!(ambient_raster_width(640, 165.0, true), 640);
        assert_eq!(ambient_raster_width(320, f32::NAN, false), 320);
        assert_eq!(ambient_raster_width(640, -1.0, false), 640);
    }
}
