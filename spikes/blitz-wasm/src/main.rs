use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{build_single_font_ctx, DocumentConfig};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

fn main() {
    let font = include_bytes!("../assets/IBMPlexSans-Regular.ttf");
    let html = "<!doctype html><html><body style=\"background:#0b0d12;color:#fff;font:24px 'IBM Plex Sans', sans-serif\"><h1>Vitals</h1><p>9000 steps</p></body></html>";
    let viewport = Viewport::new(400, 200, 1.0, ColorScheme::Dark);
    let mut document = HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(viewport),
            font_ctx: Some(build_single_font_ctx(font)),
            ..Default::default()
        },
    );
    document.resolve(0.0);
    let mut renderer = VelloCpuImageRenderer::new(400, 200);
    let mut rgba = Vec::new();
    renderer.render_to_vec(
        |scene| blitz_paint::paint_scene(scene, &mut document, 1.0, 400, 200, 0, 0),
        &mut rgba,
    );
    std::fs::write("out.rgba", &rgba).unwrap();
    println!("rgba={}", rgba.len());
}
