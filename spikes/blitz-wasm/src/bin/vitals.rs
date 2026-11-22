use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{build_single_font_ctx, DocumentConfig};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

fn main() {
    let font = include_bytes!("../../assets/IBMPlexSans-Regular.ttf");
    let html = format!(
        "{}<style>*{{font-family:'IBM Plex Sans',sans-serif !important}}svg text{{font-family:'IBM Plex Sans',sans-serif !important}}</style>",
        include_str!("../../fixtures/vitals.html")
    );
    let viewport = Viewport::new(1200, 900, 1.0, ColorScheme::Dark);
    let mut document = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            viewport: Some(viewport),
            font_ctx: Some(build_single_font_ctx(font)),
            ..Default::default()
        },
    );
    document.resolve(0.0);
    let mut renderer = VelloCpuImageRenderer::new(1200, 900);
    let mut rgba = Vec::new();
    renderer.render_to_vec(
        |scene| blitz_paint::paint_scene(scene, &mut document, 1.0, 1200, 900, 0, 0),
        &mut rgba,
    );
    std::fs::write("vitals.rgba", &rgba).unwrap();
    println!("rgba={}", rgba.len());
}
