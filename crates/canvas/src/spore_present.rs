//! What a Spore presents, in the vocabulary Canvas renders.
//!
//! A Spore hands over a `soil_scene::Present`: a scene it wants drawn, pixels it
//! drew itself, or a lease on a texture it rendered. The host's job is the same
//! in all three cases -- put it where the window says, in the z-order the
//! compositor decided, with the theme the user chose.

use canvas_workspace::{
    AssetRef, NodeContent, Scene, SceneNode, SemanticInk, Transform3,
};

/// A scene from a Spore, translated into the host's scene type.
///
/// Translation rather than a shared type on purpose: the wire schema is versioned
/// and stable, the host's scene is whatever the renderer needs this week, and a
/// conversion is where a difference between them shows up as an error instead of
/// as a drawing that is subtly wrong.
pub fn scene(scene: &soil_scene::Scene) -> Scene {
    Scene {
        nodes: scene.nodes.iter().map(node).collect(),
    }
}

fn node(node: &soil_scene::Node) -> SceneNode {
    SceneNode {
        id: node.id.clone(),
        parent_id: node.parent.clone(),
        transform: Transform3 {
            translation: node.transform.translation,
            // The wire carries euler angles because that is what a UI author
            // thinks in; the renderer wants a quaternion.
            rotation: quaternion(node.transform.rotation),
            scale: node.transform.scale,
        },
        content: content(&node.content),
        // A Spore may ask for its own easing; absent means the host default.
        transition_ms: node.transition_ms,
    }
}

fn content(content: &soil_scene::Content) -> NodeContent {
    use soil_scene::Content as From;
    match content {
        From::Group => NodeContent::Group,
        From::Rect(rect) => NodeContent::Rect {
            ink: rect.ink.map(ink),
            rgb: rect.rgb,
            opacity: rect.opacity,
            radius: rect.radius,
        },
        From::Text(text) => NodeContent::Text {
            text: text.text.clone(),
            ink: text.ink.map(ink),
            rgb: text.rgb,
            opacity: text.opacity,
        },
        From::Image(image) => NodeContent::Image {
            // A Spore's image is an asset it declared; the host resolves the id.
            // An unresolvable asset draws nothing rather than guessing a path.
            asset: AssetRef {
                sha256: image.asset.clone(),
                media_type: "application/octet-stream".into(),
                byte_length: 0,
            },
        },
        // A GPU component: the source is data, and the host compiles and caches
        // it. This is what lets a Spore ship a shader without a compositor build.
        From::Shader(shader) => NodeContent::Shader {
            wgsl: shader.wgsl.clone(),
            uniforms: shader.uniforms.clone(),
        },
    }
}

fn ink(ink: soil_scene::Ink) -> SemanticInk {
    use soil_scene::Ink as From;
    match ink {
        From::Text => SemanticInk::Text,
        From::Muted => SemanticInk::Muted,
        From::Surface => SemanticInk::Surface,
        From::Background => SemanticInk::Background,
        From::Primary => SemanticInk::Primary,
        From::Success => SemanticInk::Success,
        From::Warning => SemanticInk::Warning,
        From::Danger => SemanticInk::Danger,
        From::Info => SemanticInk::Info,
    }
}

/// Euler angles in radians, Z-Y-X, into a quaternion.
fn quaternion([x, y, z]: [f32; 3]) -> [f32; 4] {
    if x == 0.0 && y == 0.0 && z == 0.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let (sx, cx) = (x * 0.5).sin_cos();
    let (sy, cy) = (y * 0.5).sin_cos();
    let (sz, cz) = (z * 0.5).sin_cos();
    [
        sx * cy * cz - cx * sy * sz,
        cx * sy * cz + sx * cy * sz,
        cx * cy * sz - sx * sy * cz,
        cx * cy * cz + sx * sy * sz,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spore_scene_becomes_a_drawable_one() {
        let scene = soil_scene::Scene::new(vec![
            soil_scene::Node {
                id: "ground".into(),
                parent: None,
                transform: soil_scene::Transform::default(),
                transition_ms: None,
                content: soil_scene::Content::Rect(soil_scene::Rect {
                    ink: Some(soil_scene::Ink::Surface),
                    rgb: Some([1, 2, 3]),
                    opacity: 0.5,
                    radius: 6.0,
                }),
            },
            soil_scene::Node {
                id: "title".into(),
                parent: Some("ground".into()),
                transform: soil_scene::Transform {
                    translation: [0.0, 1.0, 0.2],
                    ..Default::default()
                },
                transition_ms: None,
                content: soil_scene::Content::Text(soil_scene::Text {
                    text: "Hello".into(),
                    ink: Some(soil_scene::Ink::Primary),
                    rgb: None,
                    opacity: 1.0,
                }),
            },
        ]);
        let rendered = super::scene(&scene);
        assert_eq!(rendered.nodes.len(), 2);
        assert_eq!(rendered.nodes[1].parent_id.as_deref(), Some("ground"));
        assert!(matches!(
            rendered.nodes[0].content,
            NodeContent::Rect {
                opacity: 0.5,
                radius: 6.0,
                rgb: Some([1, 2, 3]),
                ..
            }
        ));
        assert!(matches!(
            rendered.nodes[1].content,
            NodeContent::Text { ref text, .. } if text == "Hello"
        ));
    }

    #[test]
    fn a_rotation_becomes_a_unit_quaternion() {
        let q = quaternion([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((length - 1.0).abs() < 1e-5, "length {length}");
    }
}
