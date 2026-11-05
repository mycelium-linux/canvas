//! What a Spore presents, in the vocabulary Canvas renders.
//!
//! A Spore hands over a `soil_scene::Present`: a scene it wants drawn, pixels it
//! drew itself, or a lease on a texture it rendered. The host's job is the same
//! in all three cases -- put it where the window says, in the z-order the
//! compositor decided, with the theme the user chose.

use canvas_workspace::{AssetRef, NodeContent, Scene, SceneNode, SemanticInk, Transform3};

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

/// The inverse translation: a Canvas scene presented through the Spore surface
/// path. This is what lets a module-authored scene use the same bounded payload,
/// pacing and future Soil renderer as a Spore-owned surface.
pub fn soil_scene(scene: &Scene) -> soil_scene::Scene {
    soil_scene::Scene::new(scene.nodes.iter().map(soil_node).collect())
}

pub fn soil_present(scene: &Scene) -> soil_scene::Present {
    soil_scene::Present::Scene(soil_scene(scene))
}

fn soil_node(node: &SceneNode) -> soil_scene::Node {
    soil_scene::Node {
        id: node.id.clone(),
        parent: node.parent_id.clone(),
        transition_ms: None,
        transform: soil_scene::Transform {
            translation: node.transform.translation,
            rotation: euler(node.transform.rotation),
            scale: node.transform.scale,
        },
        content: soil_content(&node.content),
    }
}

fn soil_content(content: &NodeContent) -> soil_scene::Content {
    use soil_scene::Content as To;
    match content {
        NodeContent::Group => To::Group,
        NodeContent::Rect {
            ink,
            rgb,
            opacity,
            radius,
        } => To::Rect(soil_scene::Rect {
            ink: ink.map(soil_ink),
            rgb: *rgb,
            opacity: *opacity,
            radius: *radius,
        }),
        NodeContent::Text { text, ink, rgb } => To::Text(soil_scene::Text {
            text: text.clone(),
            ink: ink.map(soil_ink),
            rgb: *rgb,
            opacity: 1.0,
        }),
        NodeContent::Image { asset } => To::Image(soil_scene::Image {
            asset: asset.sha256.clone(),
        }),
        // The soil-scene vocabulary has no 3D node yet. Keep the node and its
        // children addressable rather than refusing the whole scene.
        NodeContent::Mesh { .. } | NodeContent::Camera { .. } | NodeContent::Light { .. } => {
            To::Group
        }
    }
}

fn soil_ink(ink: SemanticInk) -> soil_scene::Ink {
    use soil_scene::Ink as To;
    match ink {
        SemanticInk::Text => To::Text,
        SemanticInk::Surface => To::Surface,
        SemanticInk::Background => To::Background,
        SemanticInk::Muted => To::Muted,
        SemanticInk::Primary => To::Primary,
        SemanticInk::Success => To::Success,
        SemanticInk::Warning => To::Warning,
        SemanticInk::Danger => To::Danger,
        SemanticInk::Info => To::Info,
    }
}

fn euler([x, y, z, w]: [f32; 4]) -> [f32; 3] {
    let length = (x * x + y * y + z * z + w * w).sqrt();
    if length == 0.0 || !length.is_finite() {
        return [0.0; 3];
    }
    let (x, y, z, w) = (x / length, y / length, z / length, w / length);
    let roll = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
    let pitch = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
    [roll, pitch, yaw]
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
        // Shaders are accepted by the schema and not drawn by this build: the
        // node survives a round trip so a newer host can render it, and the older
        // one draws its children instead of failing the whole scene.
        // Canvas's 2D retained scene has no polyline yet. Keep the node as a
        // group so a GPU scene survives the fallback path without inventing a
        // second chart renderer in the compositor.
        From::Area(_) | From::Polyline(_) | From::Shader(_) => NodeContent::Group,
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
    fn a_canvas_scene_becomes_a_soil_scene_and_back() {
        let scene = Scene {
            nodes: vec![
                SceneNode {
                    id: "ground".into(),
                    parent_id: None,
                    transform: Transform3::default(),
                    content: NodeContent::Rect {
                        ink: Some(SemanticInk::Surface),
                        rgb: Some([1, 2, 3]),
                        opacity: 0.5,
                        radius: 6.0,
                    },
                },
                SceneNode {
                    id: "title".into(),
                    parent_id: Some("ground".into()),
                    transform: Transform3 {
                        translation: [0.0, 1.0, 0.2],
                        rotation: quaternion([0.0, 0.0, std::f32::consts::FRAC_PI_2]),
                        scale: [1.0; 3],
                    },
                    content: NodeContent::Text {
                        text: "Hello".into(),
                        ink: Some(SemanticInk::Primary),
                        rgb: None,
                    },
                },
            ],
        };
        let soil = soil_scene(&scene);
        assert_eq!(soil.nodes.len(), 2);
        assert_eq!(soil.nodes[1].parent.as_deref(), Some("ground"));
        assert!(matches!(
            soil.nodes[0].content,
            soil_scene::Content::Rect(soil_scene::Rect {
                opacity: 0.5,
                radius: 6.0,
                rgb: Some([1, 2, 3]),
                ..
            })
        ));
        assert!(matches!(
            soil.nodes[1].content,
            soil_scene::Content::Text(ref text) if text.text == "Hello"
        ));
        let back = super::scene(&soil);
        assert_eq!(back.nodes.len(), 2);
        assert_eq!(back.nodes[1].parent_id.as_deref(), Some("ground"));
    }

    #[test]
    fn a_rotation_becomes_a_unit_quaternion() {
        let q = quaternion([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((length - 1.0).abs() < 1e-5, "length {length}");
    }
}
