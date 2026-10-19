//! GLES adapter for canonical WGSL. No downloaded shader execution or second catalog.
use canvas_workspace::{BackgroundShader, background_shaders};

pub fn program(id: &str) -> Result<serde_json::Value, String> {
    let shader: BackgroundShader = serde_json::from_value(serde_json::json!(id))
        .map_err(|_| format!("Unknown background shader: {id}"))?;
    let body = background_shaders::source(shader)
        .ok_or_else(|| format!("Fire TV does not yet support shader: {id}"))?;
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
        assert!(program("star-nest").is_err());
    }
}
