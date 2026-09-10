//! Spike: can a Stylo/Blitz-class Rust HTML+CSS renderer serve both Canvas
//! surfaces and doc-forge templates?
//!
//! This binary does exactly two things:
//!
//!   `tera`  — reproduce doc-forge's Tera + letterhead assembly, emitting the
//!             final HTML string that doc-forge would hand to WeasyPrint.
//!   `shot`  — parse that HTML with blitz-html/blitz-dom (Stylo + Taffy +
//!             Parley), paint it into an anyrender scene, and rasterise to an
//!             RGBA8 buffer via vello_cpu. No window is ever created.
//!
//! The `shot` path is the one that matters for Canvas: `render_to_vec` hands
//! back a plain `Vec<u8>` of RGBA8 pixels, which is what you would upload as a
//! wgpu texture. Writing a PNG here is only so a human can look at it.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage:\n  \
        spike tera <template.html> <letterhead.html> <letterhead.css> <data.json> <out.html>\n  \
        spike shot <in.html> <out.png> <css_w> <css_h> <scale>";
    let Some(cmd) = args.get(1) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };

    match cmd.as_str() {
        "tera" => {
            if args.len() != 7 {
                eprintln!("{usage}");
                std::process::exit(2);
            }
            let html = assemble(
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
                Path::new(&args[5]),
            );
            fs::write(&args[6], html.as_bytes()).expect("write assembled html");
            println!("wrote {}", args[6]);
        }
        "shot" => {
            if args.len() != 7 {
                eprintln!("{usage}");
                std::process::exit(2);
            }
            let html = fs::read_to_string(&args[2]).expect("read html");
            let w: u32 = args[4].parse().expect("css_w");
            let h: u32 = args[5].parse().expect("css_h");
            let scale: f32 = args[6].parse().expect("scale");
            shot(&html, Path::new(&args[3]), w, h, scale);
        }
        other => {
            eprintln!("unknown command `{other}`\n{usage}");
            std::process::exit(2);
        }
    }
}

/// Mirror of doc-forge's HTML path: render the template body with Tera, then
/// render the letterhead (with `{{CSS}}` already substituted, as
/// `gsg_thermal_packing_slip/publish.py` does) with `body` bound to the result.
///
/// See doc-forge `src/render.rs::render_content` and
/// `src/render.rs::wrap_html_with_letterhead`.
fn assemble(template: &Path, letterhead: &Path, css: &Path, data: &Path) -> String {
    let template_body = fs::read_to_string(template).expect("read template");
    let letterhead_body = fs::read_to_string(letterhead).expect("read letterhead");
    let css_body = fs::read_to_string(css).expect("read css");
    let data_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(data).expect("read data")).expect("parse data");

    let ctx = tera::Context::from_value(data_json.clone()).expect("context");
    let mut tera = tera::Tera::default();
    let body = tera
        .render_str(&template_body, &ctx)
        .unwrap_or_else(|e| panic!("tera template render failed: {e:?}"));

    // publish.py substitutes {{CSS}} literally before upload.
    let letterhead_body = letterhead_body.replace("{{CSS}}", &css_body);

    let mut merged = data_json;
    merged
        .as_object_mut()
        .expect("data must be an object")
        .insert("body".into(), serde_json::Value::String(body));
    let ctx = tera::Context::from_value(merged).expect("letterhead context");
    tera.render_str(&letterhead_body, &ctx)
        .unwrap_or_else(|e| panic!("tera letterhead render failed: {e:?}"))
}

/// Headless render: HTML string -> RGBA8 pixels -> PNG on disk.
fn shot(html: &str, out: &Path, css_w: u32, css_h: u32, scale: f32) {
    let t_parse = Instant::now();
    let mut doc = HtmlDocument::from_html(html, DocumentConfig::default());
    let parse_ms = t_parse.elapsed().as_secs_f64() * 1000.0;

    doc.set_viewport(Viewport::new(css_w, css_h, scale, ColorScheme::Light));

    let t_style = Instant::now();
    doc.resolve(0.0);
    let style_layout_ms = t_style.elapsed().as_secs_f64() * 1000.0;

    let px_w = (css_w as f32 * scale).round() as u32;
    let px_h = (css_h as f32 * scale).round() as u32;

    let t_paint = Instant::now();
    let mut renderer = VelloCpuImageRenderer::new(px_w, px_h);
    let mut buf: Vec<u8> = Vec::with_capacity((px_w * px_h * 4) as usize);
    renderer.render_to_vec(
        |scene| {
            blitz_paint::paint_scene(scene, &mut doc, scale as f64, px_w, px_h, 0, 0);
        },
        &mut buf,
    );
    let paint_ms = t_paint.elapsed().as_secs_f64() * 1000.0;

    assert_eq!(
        buf.len(),
        (px_w as usize) * (px_h as usize) * 4,
        "expected RGBA8 buffer"
    );

    // The buffer here is exactly what Canvas would hand to
    // `wgpu::Queue::write_texture`. PNG is only for human inspection.
    let file = fs::File::create(out).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), px_w, px_h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&buf).expect("png data");
    drop(writer);

    println!(
        "{{\"out\":\"{}\",\"px\":[{},{}],\"parse_ms\":{:.1},\"style_layout_ms\":{:.1},\"paint_rasterise_ms\":{:.1},\"total_ms\":{:.1}}}",
        out.display(),
        px_w,
        px_h,
        parse_ms,
        style_layout_ms,
        paint_ms,
        parse_ms + style_layout_ms + paint_ms
    );
}
