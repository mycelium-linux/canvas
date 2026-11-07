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
        NodeContent::Mesh { asset } => To::Mesh(soil_scene::Mesh {
            asset: asset.sha256.clone(),
        }),
        NodeContent::Camera { projection } => To::Camera(soil_scene::Camera {
            projection: soil_projection(*projection),
        }),
        NodeContent::Light { light } => To::Light(soil_light(*light)),
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

/// One canonical transform: `soil_scene::Transform` is the shared answer, and
/// this file is the only place the renderer's quaternion is turned into it.
/// The wire carries euler radians applied X then Y then Z because two hosts
/// that disagreed about the order would draw the same scene differently.
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
        // A Spore's image is an asset it declared; the host resolves the id. An
        // unresolvable asset draws nothing rather than guessing a path.
        From::Image(image) => NodeContent::Image {
            asset: declared_asset(&image.asset),
        },
        From::Mesh(mesh) => NodeContent::Mesh {
            asset: declared_asset(&mesh.asset),
        },
        From::Camera(camera) => NodeContent::Camera {
            projection: canvas_projection(camera.projection),
        },
        From::Light(light) => NodeContent::Light {
            light: canvas_light(*light),
        },
        // The rest are accepted by the schema and not drawn by this build. Their
        // position in the tree still matters: keeping the node as a group lets a
        // tree survive the translation -- children land where the producer put
        // them -- instead of failing the whole scene or inventing a second
        // renderer for each vocabulary Canvas's retained 2D scene lacks.
        From::Area(_)
        | From::Band(_)
        | From::Points(_)
        | From::Heatmap(_)
        | From::Candles(_)
        | From::Polyline(_)
        | From::Shader(_)
        | From::GpuDense(_)
        | From::GpuPoints3D(_) => NodeContent::Group,
    }
}

/// An asset a Spore declared, as the host records it.
///
/// The wire carries the id and nothing else: media type and length belong to
/// the resolution step, and stating them here would assert a fact nobody
/// checked.
fn declared_asset(id: &str) -> AssetRef {
    AssetRef {
        sha256: id.to_owned(),
        media_type: "application/octet-stream".into(),
        byte_length: 0,
    }
}

fn soil_projection(projection: canvas_workspace::Projection) -> soil_scene::Projection {
    match projection {
        canvas_workspace::Projection::Perspective {
            vertical_fov_degrees,
            near,
            far,
        } => soil_scene::Projection::Perspective {
            vertical_fov_degrees,
            near,
            far,
        },
        canvas_workspace::Projection::Orthographic { height, near, far } => {
            soil_scene::Projection::Orthographic { height, near, far }
        }
    }
}

fn canvas_projection(projection: soil_scene::Projection) -> canvas_workspace::Projection {
    match projection {
        soil_scene::Projection::Perspective {
            vertical_fov_degrees,
            near,
            far,
        } => canvas_workspace::Projection::Perspective {
            vertical_fov_degrees,
            near,
            far,
        },
        soil_scene::Projection::Orthographic { height, near, far } => {
            canvas_workspace::Projection::Orthographic { height, near, far }
        }
    }
}

fn soil_light(light: canvas_workspace::Light) -> soil_scene::Light {
    match light {
        canvas_workspace::Light::Ambient { intensity } => soil_scene::Light::Ambient { intensity },
        canvas_workspace::Light::Directional { intensity, color } => {
            soil_scene::Light::Directional { intensity, color }
        }
        canvas_workspace::Light::Point { intensity, color } => {
            soil_scene::Light::Point { intensity, color }
        }
    }
}

fn canvas_light(light: soil_scene::Light) -> canvas_workspace::Light {
    match light {
        soil_scene::Light::Ambient { intensity } => canvas_workspace::Light::Ambient { intensity },
        soil_scene::Light::Directional { intensity, color } => {
            canvas_workspace::Light::Directional { intensity, color }
        }
        soil_scene::Light::Point { intensity, color } => {
            canvas_workspace::Light::Point { intensity, color }
        }
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

/// Euler angles in radians, applied X then Y then Z, into a quaternion.
///
/// The order is the scene contract rather than a local choice:
/// `soil_scene::Transform` states the same order, so a producer and this
/// conversion cannot disagree about what a rotation means.
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
                transition_ms: None,
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
                transition_ms: None,
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

    /// The three 3D node kinds reach the wire as themselves. Before this they
    /// were flattened into groups, so a mesh, a camera and a light all arrived
    /// at a Soil renderer as empty containers.
    #[test]
    fn mesh_camera_and_light_keep_their_own_kind() {
        let scene = Scene {
            nodes: vec![
                SceneNode {
                    id: "rig".into(),
                    parent_id: None,
                    transform: Transform3::default(),
                    content: NodeContent::Mesh {
                        asset: AssetRef {
                            sha256: "model:robot".into(),
                            media_type: "model/gltf-binary".into(),
                            byte_length: 4096,
                        },
                    },
                },
                SceneNode {
                    id: "eye".into(),
                    parent_id: Some("rig".into()),
                    transform: Transform3::default(),
                    content: NodeContent::Camera {
                        projection: canvas_workspace::Projection::Perspective {
                            vertical_fov_degrees: 60.0,
                            near: 0.1,
                            far: 100.0,
                        },
                    },
                },
                SceneNode {
                    id: "lamp".into(),
                    parent_id: Some("rig".into()),
                    transform: Transform3 {
                        translation: [0.0, 3.0, 0.0],
                        rotation: quaternion([0.0, 0.0, 0.0]),
                        scale: [1.0; 3],
                    },
                    content: NodeContent::Light {
                        light: canvas_workspace::Light::Directional {
                            intensity: 0.8,
                            color: [1.0, 0.95, 0.9],
                        },
                    },
                },
            ],
        };
        let soil = super::soil_scene(&scene);
        assert!(matches!(
            soil.nodes[0].content,
            soil_scene::Content::Mesh(ref mesh) if mesh.asset == "model:robot"
        ));
        assert!(matches!(
            soil.nodes[1].content,
            soil_scene::Content::Camera(soil_scene::Camera {
                projection: soil_scene::Projection::Perspective {
                    vertical_fov_degrees: 60.0,
                    ..
                }
            })
        ));
        assert!(matches!(
            soil.nodes[2].content,
            soil_scene::Content::Light(soil_scene::Light::Directional { .. })
        ));

        // Round trip: nothing degrades into a group on the way back.
        let back = super::scene(&soil);
        assert!(matches!(back.nodes[0].content, NodeContent::Mesh { .. }));
        assert!(matches!(back.nodes[1].content, NodeContent::Camera { .. }));
        assert!(matches!(back.nodes[2].content, NodeContent::Light { .. }));
        assert_eq!(back.nodes[2].parent_id.as_deref(), Some("rig"));
    }

    #[test]
    fn a_rotation_becomes_a_unit_quaternion() {
        let q = quaternion([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((length - 1.0).abs() < 1e-5, "length {length}");
    }
}
