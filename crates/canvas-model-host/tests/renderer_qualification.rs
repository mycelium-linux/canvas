//! Native GPU acceptance; run explicitly with --ignored on a GPU host.
use canvas_core::{Accent, ThemeMode, ThemeSelection};
use canvas_model_host::{
    CameraPreset, Material,
    mesh::{PreparedMesh, Vertex},
    renderer::Renderer,
};
use fpl_gfx::mesh::Finish;

#[test]
#[ignore = "Requires a native GPU adapter; explicit viewer qualification gate"]
fn native_fill_edges_and_quantitative_colors() {
    let mesh = PreparedMesh {
        asset: "authored-triangle.stl".into(),
        vertices: [[-1., -1., 0.], [1., -1., 0.], [0., 1., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                field: 0.5,
            })
            .to_vec(),
        triangle_indices: vec![0, 1, 2],
        edge_indices: vec![0, 1, 1, 2, 2, 0],
        source_center: [0.; 3],
        source_extent: 2.,
    };
    let mut renderer = Renderer::new(64, 64).unwrap();
    renderer.upload(&mesh);
    let theme = ThemeSelection::coherence(ThemeMode::Dark, Accent::Info).visual_theme();
    let mut material = Material {
        color: Some([72, 154, 184]),
        opacity: 1.,
        ..Default::default()
    };
    let mut capture = |finish, edges, rotation, time, field: Option<&canvas_model_host::Field>| {
        renderer.set_finish(finish);
        material.wireframe = edges;
        assert!(
            renderer
                .render(
                    CameraPreset::Front.direction(),
                    1.,
                    rotation,
                    &material,
                    field,
                    &theme,
                    time
                )
                .unwrap()
                .is_none()
        );
        let pixels = renderer.drain().unwrap().unwrap();
        <[u8; 4]>::try_from(&pixels[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4]).unwrap()
    };
    let plain = capture(Finish::Hologram, false, [0.; 3], 0., None);
    assert_eq!(plain, capture(Finish::Hologram, true, [0.; 3], 0., None));
    let flat = capture(Finish::Flat, false, [0.; 3], 0., None);
    assert_eq!(flat, capture(Finish::Flat, true, [0.; 3], 7., None));
    assert_eq!(plain, capture(Finish::Legacy, false, [0.; 3], 0., None));
    assert_eq!(flat, capture(Finish::Legacy, true, [0.; 3], 0., None));
    assert_ne!(plain, flat);
    // A field is only used to enable the quantitative shader mode: the supplied
    // mesh already carries the independently authored normalized scalar 0.5.
    let field: canvas_model_host::Field = serde_json::from_value(serde_json::json!({
        "name":"temperature","unit":"C","range":[20,30],"colormap":"viridis",
        "source":{"kind":"gradient","axis":"x","from":20,"to":30}
    }))
    .unwrap();
    let expected = [53_i16, 94, 139, 255]; // midpoint of Viridis stops 2 and 3
    for finish in [Finish::Legacy, Finish::Hologram, Finish::Flat] {
        for edges in [false, true] {
            for (rotation, time) in [([0.; 3], 0.), ([0., 0., 30.], 7.)] {
                let color = capture(finish, edges, rotation, time, Some(&field));
                for (actual, expected) in color.into_iter().zip(expected) {
                    assert!(
                        (i16::from(actual) - expected).abs() <= 1,
                        "palette color {color:?}"
                    );
                }
            }
        }
    }
}
