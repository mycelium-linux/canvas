//! SPIKE: can a Stylo/Blitz-class Rust HTML+CSS renderer preview doc-forge templates?
//!
//! Not production code. Two jobs:
//!   1. `render`  - Tera-render a REAL doc-forge template with REAL sample data, feed the
//!                  identical HTML to Blitz (headless, to an RGBA8 buffer) and to WeasyPrint,
//!                  and write both PNGs for a human to compare.
//!   2. `probe`   - render ~16 isolated CSS feature probes and report, per feature, whether
//!                  Blitz produced the geometry the feature implies. Against templates we own
//!                  the CSS surface is enumerable, which is the whole premise of this path.

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib, Value as LuaValue};
use serde::Deserialize;
use std::io::Read;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const DOC_FORGE: &str = "/Users/ajmwagar/src/fpl/doc-forge";
const OUT: &str = "out";

/// CSS px per mm at the 96dpi CSS reference: 96 / 25.4.
const PX_PER_MM: f32 = 96.0 / 25.4;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "render".into());
    std::fs::create_dir_all(OUT).unwrap();
    match mode.as_str() {
        "render" => {
            render_target(A6_SLIP);
        }
        "invoice" => {
            render_target(A4_INVOICE);
        }
        "probe" => probe_css(),
        "compare" => compare_outputs(),
        // `dashboard <template.lua> <source-id> <width> <height>` reads one
        // bounded JSON snapshot from stdin. Sandboxed Lua maps data to HTML
        // and CSS; Blitz owns pixels; Canvas only reloads the atomic frame.
        "dashboard" => {
            let a: Vec<String> = std::env::args().collect();
            render_dashboard(&a[2], &a[3], &a[4], &a[5]);
        }
        // `file <in.html> <out.png> <css_width> <scale>` - render any HTML file.
        "file" => {
            let a: Vec<String> = std::env::args().collect();
            let inp = &a[2];
            let outp = &a[3];
            let w: f32 = a[4].parse().unwrap();
            let sc: f32 = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(2.0);
            let html = std::fs::read_to_string(inp).unwrap();
            let t0 = std::time::Instant::now();
            let r = render_html(&html, w, sc, 2000.0, true);
            println!(
                "{inp}: {}x{} in {:?} (content {:.1} css px)",
                r.width,
                r.height,
                t0.elapsed(),
                r.content_css_height
            );
            save_png(&r, outp);
        }
        other => panic!("unknown mode {other}"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HtmlView {
    html: String,
    css: String,
}

fn render_dashboard(template: &str, source_id: &str, width: &str, height: &str) {
    assert!(
        canvas_core::is_safe_frame_id(source_id),
        "unsafe frame source id"
    );
    let width: u32 = width.parse().expect("width");
    let height: u32 = height.parse().expect("height");
    assert!((320..=4096).contains(&width) && (240..=2160).contains(&height));
    let source = std::fs::read_to_string(template).expect("read Lua template");
    assert!(!source.is_empty() && source.len() <= 32 * 1024);
    let mut input = Vec::new();
    std::io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut input)
        .expect("read dashboard JSON");
    assert!(input.len() <= 4 * 1024 * 1024);
    let data: serde_json::Value = serde_json::from_slice(&input).expect("dashboard JSON");
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .expect("Lua VM");
    lua.set_memory_limit(4 * 1024 * 1024)
        .expect("Lua memory limit");
    let interrupts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&interrupts);
    lua.set_interrupt(move |_| {
        if counter.fetch_add(1, Ordering::Relaxed) >= 10_000 {
            Err(mlua::Error::RuntimeError(
                "HTML template instruction budget exceeded".into(),
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    for name in [
        "dofile",
        "loadfile",
        "load",
        "require",
        "collectgarbage",
        "debug",
        "io",
        "os",
        "package",
    ] {
        lua.globals()
            .raw_set(name, LuaValue::Nil)
            .expect("sandbox Lua");
    }
    lua.globals()
        .set("data", lua.to_value(&data).expect("serialize dashboard"))
        .expect("install dashboard");
    lua.sandbox(true).expect("sandbox Lua");
    let value = lua
        .load(&source)
        .set_name(template)
        .eval()
        .expect("run Lua template");
    let view: HtmlView = lua.from_value(value).expect("Lua HTML view");
    assert!(view.html.len() <= 128 * 1024 && view.css.len() <= 64 * 1024);
    let document = format!(
        "<!doctype html><meta charset=utf-8><style>{}</style>{}",
        view.css, view.html
    );
    let rendered = render_html(&document, width as f32, 1.0, height as f32, false);
    let image = image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.rgba)
        .expect("Blitz RGBA dimensions");
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
        .encode_image(&image)
        .expect("encode dashboard JPEG");
    let root = canvas_core::runtime_root().expect("Canvas runtime root");
    let path = canvas_core::frame_file(&root, source_id).expect("frame path");
    canvas_core::write_atomic(&path, &jpeg).expect("write dashboard frame");
    println!("{}", path.display());
}

// ---------------------------------------------------------------------------
// Core: HTML -> RGBA8, headless. No window, no surface, no swapchain.
// ---------------------------------------------------------------------------

struct Rendered {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    /// Content height Blitz laid out, in CSS px, before we clamped to the buffer.
    content_css_height: f32,
}

fn render_html(
    html: &str,
    css_width: f32,
    scale: f32,
    max_css_height: f32,
    trim: bool,
) -> Rendered {
    // Lay out and paint in a deliberately tall viewport so nothing is height-constrained.
    let vp = Viewport::new(
        (css_width * scale) as u32,
        (max_css_height * scale) as u32,
        scale,
        ColorScheme::Light,
    );
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(vp),
            ..Default::default()
        },
    );
    doc.resolve(0.0);

    let pw = (css_width * scale).round() as u32;
    let ph = (max_css_height * scale).round() as u32;

    let mut renderer = VelloCpuImageRenderer::new(pw, ph);
    let mut rgba = Vec::new();
    let base = &mut *doc;
    renderer.render_to_vec(
        |scene| blitz_paint::paint_scene(scene, base, scale as f64, pw, ph, 0, 0),
        &mut rgba,
    );

    // Measure content height from the raster rather than the DOM: scan up from the
    // bottom for the last row that differs from the page background. Same answer,
    // no dependence on layout internals.
    let bg = {
        let i = (((ph - 1) * pw) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    let mut last_ink = 0u32;
    'rows: for y in (0..ph).rev() {
        for x in 0..pw {
            let i = ((y * pw + x) * 4) as usize;
            let d = (rgba[i] as i32 - bg[0] as i32).abs()
                + (rgba[i + 1] as i32 - bg[1] as i32).abs()
                + (rgba[i + 2] as i32 - bg[2] as i32).abs();
            if d > 24 {
                last_ink = y;
                break 'rows;
            }
        }
    }
    let content_css_height = (last_ink + 1) as f32 / scale;

    if trim {
        let keep = ((last_ink + 1).min(ph)).max(1);
        rgba.truncate((keep * pw * 4) as usize);
        return Rendered {
            rgba,
            width: pw,
            height: keep,
            content_css_height,
        };
    }

    Rendered {
        rgba,
        width: pw,
        height: ph,
        content_css_height,
    }
}

fn save_png(r: &Rendered, path: &str) {
    image::save_buffer(
        path,
        &r.rgba,
        r.width,
        r.height,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Job 1: the real doc-forge A6 thermal packing slip.
// ---------------------------------------------------------------------------

struct Target {
    key: &'static str,
    body: &'static str,
    letterhead: &'static str,
    css: &'static str,
    /// Inline JSON payload, or a path relative to the spike dir.
    data: &'static str,
    /// @page geometry the stylesheet declares, in mm: (page_w, page_h, margin_x, margin_y).
    page_mm: (f32, f32, f32, f32),
}

const A6_SLIP: Target = Target {
    key: "blitz",
    body: "fixtures/templates/packing_slip_a6.html",
    letterhead: "fixtures/letterheads/thermal_packing_letterhead.html",
    css: "fixtures/css/packing_slip_a6.css",
    data: "",
    page_mm: (105.0, 148.0, 6.0, 6.0),
};

const A4_INVOICE: Target = Target {
    key: "invoice",
    body: "fixtures/templates/gsg_html_invoice.html",
    letterhead: "fixtures/letterheads/gsg_html_letterhead.html",
    css: "fixtures/css/gsg_invoice.css",
    data: "invoice_data.json",
    page_mm: (210.0, 297.0, 14.0, 16.0),
};

fn render_target(t: Target) {
    let body_tpl = std::fs::read_to_string(format!("{DOC_FORGE}/{}", t.body)).unwrap();
    let letterhead_tpl = std::fs::read_to_string(format!("{DOC_FORGE}/{}", t.letterhead)).unwrap();
    let css = std::fs::read_to_string(format!("{DOC_FORGE}/{}", t.css)).unwrap();

    // Payloads are lifted verbatim from doc-forge's own real-weasyprint tests.
    let raw = if t.data.is_empty() {
        SAMPLE_DATA.to_string()
    } else {
        std::fs::read_to_string(t.data).unwrap()
    };
    let data: serde_json::Value = serde_json::from_str(&raw).unwrap();

    // Mirror doc-forge/src/render.rs: render body, then inject as `body | safe`
    // into the letterhead with the same context.
    let ctx = tera::Context::from_value(data.clone()).unwrap();
    let body = tera::Tera::one_off(&body_tpl, &ctx, false).expect("render body");
    let mut merged = data;
    merged["body"] = serde_json::Value::String(body);
    let ctx2 = tera::Context::from_value(merged).unwrap();
    let doc_html = tera::Tera::one_off(&letterhead_tpl, &ctx2, false).expect("render letterhead");

    // doc-forge hands the CSS to weasyprint as `-s stylesheet`. Blitz has no such
    // side channel, so inline it into <head> and give BOTH engines the same file.
    let final_html = doc_html.replacen("</head>", &format!("<style>\n{css}\n</style>\n</head>"), 1);
    let html_path = format!("{OUT}/{}.html", t.key);
    std::fs::write(&html_path, &final_html).unwrap();

    // Blitz has no @page. Shim it: viewport width = page width minus both margins.
    let (page_w, page_h, margin_x, margin_y) = t.page_mm;
    let content_mm = page_w - 2.0 * margin_x;
    let css_width = content_mm * PX_PER_MM;
    let scale = 4.0; // ~384 dpi, enough to judge text quality

    let t0 = std::time::Instant::now();
    let r = render_html(&final_html, css_width, scale, 2000.0, true);
    let elapsed = t0.elapsed();

    save_png(&r, &format!("{OUT}/{}.png", t.key));

    let content_h_mm = r.content_css_height / PX_PER_MM;
    let usable_mm = page_h - 2.0 * margin_y;
    println!("== BLITZ: {} ==", t.body);
    println!("  html bytes         : {}", final_html.len());
    println!("  page (@page)       : {page_w}x{page_h}mm, margins {margin_x}/{margin_y}mm");
    println!("  viewport css width : {css_width:.1}px ({content_mm:.0}mm content box)");
    println!(
        "  content height     : {:.1}px = {content_h_mm:.1}mm",
        r.content_css_height
    );
    println!(
        "  fits one page?     : {} (usable ~{usable_mm:.0}mm; Blitz itself does NOT paginate)",
        if content_h_mm <= usable_mm {
            "yes"
        } else {
            "NO - would need N pages"
        }
    );
    println!("  implied page count : {:.2}", content_h_mm / usable_mm);
    println!("  raster             : {}x{} @ {scale}x", r.width, r.height);
    println!("  wall time          : {elapsed:?}");
    println!(
        "  wrote              : {OUT}/{}.png and {OUT}/{}.html",
        t.key, t.key
    );
}

const SAMPLE_DATA: &str = r#"{
  "company": {
    "name": "Grey Summit Gear",
    "legal_name": "GSG Mfg LLC",
    "short_name": "GSG",
    "store_url": "store.greysummitgear.com"
  },
  "order": {
    "id": "PO-2048",
    "ship_date": "2024-04-14",
    "shipping": {
      "name": "Jordan Safety",
      "address_line1": "845 Harbor Way",
      "address_line2": "Dock 4",
      "city": "Seattle",
      "region": "WA",
      "postal_code": "98101",
      "country": "USA"
    },
    "line_items": [
      { "sku": "GSG-04", "name": "Rounded Detent Pin", "qty": 5 },
      { "sku": "GSG-22", "name": "Safety Cable", "qty": 2 }
    ],
    "summary": { "item_count": 7, "notes": "Leave dock door unlocked" }
  },
  "marketing": {
    "discount": "SAFETYFIRST",
    "percent": "15",
    "thank_you": "Thank you for keeping safety first!"
  }
}"#;

// ---------------------------------------------------------------------------
// Job 2: enumerate CSS support by probing real rendered pixels.
// ---------------------------------------------------------------------------

/// Each probe paints red only where the feature under test, if implemented,
/// places it. Checks are `(x, y, expect_ink)` in CSS px at scale 1.
struct Probe {
    name: &'static str,
    why: &'static str,
    css: &'static str,
    body: &'static str,
    checks: &'static [(u32, u32, bool)],
}

const PROBES: &[Probe] = &[
    Probe {
        name: "mm length units",
        why: "packing_slip_a6.css sizes padding/gap in mm throughout",
        css: ".b{width:10mm;height:20px;background:#f00}",
        // 10mm == 37.8 CSS px
        body: "<div class=b></div>",
        checks: &[(30, 10, true), (45, 10, false)],
    },
    Probe {
        name: "flexbox justify-content",
        why: ".header uses display:flex; justify-content:space-between",
        css: ".c{display:flex;justify-content:flex-end;width:100px}.b{width:20px;height:20px;background:#f00}",
        body: "<div class=c><div class=b></div></div>",
        checks: &[(90, 10, true), (10, 10, false)],
    },
    Probe {
        name: "flexbox gap",
        why: ".slip uses gap:4.2mm between sections",
        css: ".c{display:flex;gap:40px}.b{width:20px;height:20px;background:#f00}",
        body: "<div class=c><div class=b></div><div class=b></div></div>",
        checks: &[(10, 10, true), (30, 10, false), (70, 10, true)],
    },
    Probe {
        name: "grid-template-columns %",
        why: ".ship-to uses grid-template-columns: 72% 28%",
        css: ".c{display:grid;grid-template-columns:72% 28%;width:100px}.b{height:20px;background:#f00}",
        body: "<div class=c><div></div><div class=b></div></div>",
        checks: &[(80, 10, true), (50, 10, false)],
    },
    Probe {
        name: "grid-column: 1 / -1",
        why: ".summary-note spans both columns",
        css: ".c{display:grid;grid-template-columns:50px 50px}.b{height:20px;background:#f00;grid-column:1 / -1}",
        body: "<div class=c><div class=b></div></div>",
        checks: &[(10, 10, true), (90, 10, true)],
    },
    Probe {
        name: "border-radius",
        why: ".slip and .rewards .code use border-radius",
        css: ".b{width:40px;height:40px;background:#f00;border-radius:20px}",
        body: "<div class=b></div>",
        checks: &[(1, 1, false), (20, 20, true)],
    },
    Probe {
        name: "dashed border",
        why: ".rewards uses border-top: 1px dashed #999",
        css: ".b{width:100px;height:20px;border-top:4px dashed #f00}",
        body: "<div class=b></div>",
        checks: &[(2, 2, true), (50, 12, false)],
    },
    Probe {
        name: "table border-collapse",
        why: ".items table uses border-collapse: collapse",
        css: "table{border-collapse:collapse}td{border:4px solid #f00;padding:0;width:40px;height:20px}",
        body: "<table><tr><td></td><td></td></tr></table>",
        // Collapsed: the shared edge is one 4px rule near x=40, not two.
        checks: &[(2, 10, true), (41, 10, true), (20, 10, false)],
    },
    Probe {
        name: ":last-child selector",
        why: ".items tbody tr:last-child td { border-bottom: none }",
        css: "div div{height:20px;background:#00f}div div:last-child{background:#f00}",
        body: "<div><div></div><div></div></div>",
        checks: &[(10, 30, true)],
    },
    Probe {
        name: "text-transform: uppercase",
        why: ".brand and .block-title are uppercased in CSS, not in the template",
        // Lowercase 'x' is short; uppercase 'X' reaches the cap line. Ink at the
        // cap line only appears if the transform ran.
        css: "p{margin:0;font:40px/1 monospace;text-transform:uppercase;color:#f00}",
        body: "<p>xxxx</p>",
        checks: &[(8, 12, true)],
    },
    Probe {
        name: "letter-spacing",
        why: ".brand uses letter-spacing: 0.12em",
        css: "p{margin:0;font:20px/1 monospace;letter-spacing:30px;color:#f00;display:inline-block;background:#f00}",
        body: "<p>ab</p>",
        // 2 chars * (12px advance + 30px spacing) ~= 84px wide, vs ~24px unspaced.
        checks: &[(60, 10, true)],
    },
    Probe {
        name: "position: absolute",
        why: "not in the A6 slip, but gsg_catalog.css and letterheads use it",
        css: ".c{position:relative;width:100px;height:60px}.b{position:absolute;left:60px;top:40px;width:20px;height:15px;background:#f00}",
        body: "<div class=c><div class=b></div></div>",
        checks: &[(70, 45, true), (10, 10, false)],
    },
    Probe {
        name: "linear-gradient background",
        why: "used by gsg_sales_docs.css header bands",
        css: ".b{width:100px;height:20px;background:linear-gradient(to right,#f00,#f00)}",
        body: "<div class=b></div>",
        checks: &[(50, 10, true)],
    },
    Probe {
        name: "box-shadow",
        why: "card treatments in the sales/catalog stylesheets",
        css: "body{background:#fff}.b{margin:20px;width:40px;height:20px;background:#fff;box-shadow:0 0 0 6px #f00}",
        body: "<div class=b></div>",
        checks: &[(17, 30, true)],
    },
    Probe {
        name: "transform: rotate",
        why: "watermark / rotated labels",
        css: ".c{width:100px;height:100px}.b{width:40px;height:4px;background:#f00;transform:rotate(90deg);transform-origin:0 0}",
        body: "<div class=c><div class=b></div></div>",
        checks: &[(2, 30, true)],
    },
    Probe {
        name: "opacity",
        why: "muted text and rules",
        css: ".b{width:40px;height:20px;background:#f00;opacity:0.5}",
        body: "<div class=b></div>",
        // Half-opacity red over white is pink: still ink, but not saturated.
        checks: &[(20, 10, true)],
    },
    Probe {
        name: "@media print",
        why: "WeasyPrint evaluates print media; a preview must agree with it",
        css: "@media print{.b{background:#f00}}.b{width:40px;height:20px;background:#fff}",
        body: "<div class=b></div>",
        checks: &[(20, 10, true)],
    },
    Probe {
        name: "custom properties + var()",
        why: "every doc-forge stylesheet defines --brand-* on :root and reads them with var()",
        css: ":root{--c:#f00}.b{width:40px;height:20px;background:var(--c)}",
        body: "<div class=b></div>",
        checks: &[(20, 10, true)],
    },
    Probe {
        name: "::before + content",
        why: "gsg_catalog.css and gsg_sales_docs.css draw rules and badges with ::before/::after",
        css: ".b::before{content:'';display:block;width:40px;height:20px;background:#f00}",
        body: "<div class=b></div>",
        checks: &[(20, 10, true)],
    },
    Probe {
        name: ":not(:last-child)",
        why: "used for row separators in the catalog stylesheet",
        css: "div div{height:20px;background:#fff}div div:not(:last-child){background:#f00}",
        body: "<div><div></div><div></div></div>",
        checks: &[(10, 10, true), (10, 30, false)],
    },
    Probe {
        name: "table-layout: fixed",
        why: "gsg_sku_catalog / bulk quote tables pin column widths",
        css: "table{table-layout:fixed;width:100px;border-collapse:collapse}td{padding:0;height:20px}.r{background:#f00}",
        body: "<table><tr><td class=r>wwwwwwwwwwwwwwww</td><td></td></tr></table>",
        checks: &[(40, 10, true), (60, 10, false)],
    },
    Probe {
        name: "grid repeat() / minmax()",
        why: "grid-template-columns: repeat(N, minmax(0,1fr)) in the catalog grid",
        css: ".c{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));width:100px}.b{height:20px;background:#f00}",
        body: "<div class=c><div></div><div class=b></div></div>",
        checks: &[(75, 10, true), (25, 10, false)],
    },
    Probe {
        name: "radial-gradient",
        why: "used for the accent wash in gsg_sales_docs.css",
        css: ".b{width:100px;height:40px;background:radial-gradient(circle,#f00,#f00)}",
        body: "<div class=b></div>",
        checks: &[(50, 20, true)],
    },
    Probe {
        name: "page-break-before: always",
        why: "doc-forge uses page-break-* to control multi-page documents",
        // Blitz has no pagination, so this should be inert. The probe records that
        // fact rather than asserting a break.
        css: ".a{height:20px;background:#f00}.b{page-break-before:always;height:20px;background:#f00}",
        body: "<div class=a></div><div class=b></div>",
        // Expect BLANK: if pagination worked, .b would move off this page.
        checks: &[(10, 30, false)],
    },
    Probe {
        name: "@page size / pagination",
        why: "@page size:A6; margin:6mm is how doc-forge sets page geometry",
        // If @page were honoured as page geometry, content would break at the page
        // box. Blitz lays out one continuous viewport, so this probe asks only
        // whether the rule changes anything at all.
        css: "@page{size:50px 30px;margin:0}.b{width:200px;height:200px;background:#f00}",
        body: "<div class=b></div>",
        // Expect BLANK: a 50x30 page box would clip a 200x200 block.
        checks: &[(150, 150, false)],
    },
];

fn probe_css() {
    const W: f32 = 200.0;
    const H: f32 = 120.0;
    let mut supported = 0usize;
    let mut failed: Vec<&str> = Vec::new();

    println!(
        "{:<30} {:<10} {}",
        "CSS FEATURE", "RESULT", "WHY IT MATTERS"
    );
    println!("{}", "-".repeat(100));

    for p in PROBES {
        let html = format!(
            "<html><head><style>html,body{{margin:0;padding:0;background:#fff;height:120px}}{}</style></head><body>{}</body></html>",
            p.css, p.body
        );
        let r = render_html(&html, W, 1.0, H, false);
        let ok = p
            .checks
            .iter()
            .all(|&(x, y, expect_ink)| has_ink(&r, x, y) == expect_ink);
        if ok {
            supported += 1;
        } else {
            failed.push(p.name);
        }
        println!(
            "{:<30} {:<10} {}",
            p.name,
            if ok { "ok" } else { "MISMATCH" },
            p.why
        );
        if !ok {
            for &(x, y, expect_ink) in p.checks {
                let got = has_ink(&r, x, y);
                if got != expect_ink {
                    println!(
                        "    at ({x},{y}): expected {}, got {} {:?}",
                        if expect_ink { "ink" } else { "blank" },
                        if got { "ink" } else { "blank" },
                        px(&r, x, y)
                    );
                }
            }
        }
    }

    println!("{}", "-".repeat(100));
    println!("{supported}/{} probes matched", PROBES.len());
    if !failed.is_empty() {
        println!("mismatched: {failed:?}");
    }
}

fn px(r: &Rendered, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * r.width + x) * 4) as usize;
    if i + 3 >= r.rgba.len() {
        return [255, 255, 255, 255];
    }
    [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], r.rgba[i + 3]]
}

/// "Ink" = meaningfully darker or more saturated than the white page.
fn has_ink(r: &Rendered, x: u32, y: u32) -> bool {
    let [rr, gg, bb, _] = px(r, x, y);
    let min = rr.min(gg).min(bb) as i32;
    (255 - min) > 40
}

// ---------------------------------------------------------------------------
// Job 1b: objective structural comparison of the two rasters.
//
// Pixel identity is the wrong question. The question is whether the two engines
// put the same content bands in the same relative places. So: normalise both
// images to 100 vertical bands, measure ink coverage per band, and compare.
// ---------------------------------------------------------------------------

const BANDS: usize = 100;

fn ink_profile(path: &str) -> Option<(Vec<f32>, u32, u32, f32)> {
    let img = image::open(path).ok()?.to_luma8();
    let (w, h) = img.dimensions();
    let mut bands = vec![0f32; BANDS];
    let mut counts = vec![0f32; BANDS];
    let mut total_ink = 0f32;
    for y in 0..h {
        let b = ((y as usize) * BANDS / (h as usize)).min(BANDS - 1);
        for x in 0..w {
            let v = img.get_pixel(x, y).0[0];
            let ink = if v < 215 { 1.0 } else { 0.0 };
            bands[b] += ink;
            counts[b] += 1.0;
            total_ink += ink;
        }
    }
    for i in 0..BANDS {
        if counts[i] > 0.0 {
            bands[i] /= counts[i];
        }
    }
    Some((bands, w, h, total_ink / (w as f32 * h as f32)))
}

fn compare_outputs() {
    let a = std::env::args()
        .nth(2)
        .map(|k| format!("{OUT}/{k}.png"))
        .unwrap_or_else(|| format!("{OUT}/blitz.png"));
    let b = std::env::args()
        .nth(2)
        .map(|k| format!("{OUT}/{k}.weasy.png"))
        .unwrap_or_else(|| format!("{OUT}/blitz.weasy.png"));
    let (pa, wa, ha, inka) = match ink_profile(&a) {
        Some(v) => v,
        None => {
            println!("missing {a} - run `render` first");
            return;
        }
    };
    let (pb, wb, hb, inkb) = match ink_profile(&b) {
        Some(v) => v,
        None => {
            println!("missing {b} - run weasyprint + pdftoppm first (see compare.sh)");
            return;
        }
    };

    println!("blitz      : {wa}x{ha}  ink coverage {:.2}%", inka * 100.0);
    println!("weasyprint : {wb}x{hb}  ink coverage {:.2}%", inkb * 100.0);
    println!(
        "aspect     : blitz {:.3}  weasy {:.3}",
        wa as f32 / ha as f32,
        wb as f32 / hb as f32
    );

    // Pearson correlation over the 100-band ink profile.
    let n = BANDS as f32;
    let (ma, mb) = (pa.iter().sum::<f32>() / n, pb.iter().sum::<f32>() / n);
    let mut num = 0f32;
    let (mut da, mut db) = (0f32, 0f32);
    for i in 0..BANDS {
        let (x, y) = (pa[i] - ma, pb[i] - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    let corr = if da > 0.0 && db > 0.0 {
        num / (da.sqrt() * db.sqrt())
    } else {
        f32::NAN
    };
    println!("\nvertical ink-profile correlation: {corr:.3}  (1.0 = same content bands in the same places)");

    println!("\nband  blitz                    weasyprint");
    for i in (0..BANDS).step_by(4) {
        let bar = |v: f32| "#".repeat((v * 40.0).round() as usize);
        println!("{i:>4}  {:<24} {}", bar(pa[i]), bar(pb[i]));
    }
}
