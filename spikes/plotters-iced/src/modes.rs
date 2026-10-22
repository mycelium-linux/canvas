//! Headless evidence modes, shared by both crates in this spike.

use crate::audit;
use crate::palette::DEMO_DARK;
use crate::render;
use crate::spec;
use plotters::chart::ChartBuilder;
use plotters::drawing::IntoDrawingArea;
use std::time::Instant;

/// `plots/rack-thermal.lua`, resolved from either crate root.
pub fn script_path() -> std::path::PathBuf {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let direct = here.join("plots/rack-thermal.lua");
    if direct.exists() {
        return direct;
    }
    here.join("../plots/rack-thermal.lua")
}

pub fn read_script() -> String {
    let p = script_path();
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

pub fn audit_mode() {
    let source = read_script();
    let started = Instant::now();
    let spec = spec::load(&source, &DEMO_DARK, 0.0).expect("plot script");
    let lua_elapsed = started.elapsed();

    println!("== Lua ==");
    println!("  script bytes      {}", source.len());
    println!("  eval + validate   {lua_elapsed:?}");
    println!(
        "  produced          {} series / {} points",
        spec.series.len(),
        spec.point_count()
    );
    let many = Instant::now();
    for i in 0..100 {
        spec::load(&source, &DEMO_DARK, i as f64 * 0.05).expect("plot script");
    }
    let per = many.elapsed() / 100;
    println!("  100x eval         {:?}  ({per:?} each)", many.elapsed());

    for (label, size) in [
        ("widget 900x420", (900u32, 420u32)),
        ("wall 5760x1200", (5760u32, 1200u32)),
    ] {
        for rotate in [true, false] {
            let backend = audit::AuditBackend::new(size.0, size.1);
            let counts = backend.handle();
            {
                let area = backend.into_drawing_area();
                render::draw_spec(&spec, &DEMO_DARK, ChartBuilder::on(&area), rotate);
                area.present().expect("present");
            }
            let c = counts.borrow();
            println!(
                "\n== {label} / y label via {} ==",
                if rotate { "plotters y_desc" } else { "horizontal caption" }
            );
            println!("  frame ops         {}", c.frame_ops());
            println!(
                "    paths {} ({} verts) | lines {} | rect fill {} | rect stroke {} | circles {} | polys {} ({} verts) | text {} | pixels {}",
                c.paths, c.path_vertices, c.lines, c.rects_filled, c.rects_stroked,
                c.circles, c.polygons, c.polygon_vertices, c.texts, c.pixels
            );
            println!("  bitmap blits silently dropped by plotters-iced2: {}", c.bitmaps_dropped);
            let rotated = c.rotated_texts();
            if rotated.is_empty() {
                println!("  rotated text requested: none");
            } else {
                println!("  rotated text requested -> SILENTLY DROPPED by plotters-iced2:");
                for (content, t) in rotated {
                    println!("    {t:>10}  {content:?}");
                }
            }
            let strings: Vec<&str> = c.text_calls.iter().map(|(s, _)| s.as_str()).collect();
            println!("  strings drawn ({}): {strings:?}", strings.len());
        }
    }
}

pub fn contract_mode() {
    let cases: [(&str, &str); 7] = [
        (
            "unknown palette role",
            r#"return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,1},
                series={{ name="s", role="hotpink", points={{0,0},{1,1}} }} }"#,
        ),
        (
            "raw RGB instead of a role",
            r#"return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,1},
                series={{ name="s", role={255,0,0}, points={{0,0}} }} }"#,
        ),
        (
            "too many points",
            r#"local p={} for i=1,5000 do p[i]={i,i} end
               return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,5000},
                series={{ name="s", role="primary", points=p }} }"#,
        ),
        (
            "too many series",
            r#"local s={} for i=1,12 do s[i]={name="s"..i, role="primary", points={{0,0}}} end
               return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,1}, series=s }"#,
        ),
        (
            "non-finite point",
            r#"return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,1},
                series={{ name="s", role="primary", points={{0, 0/0}} }} }"#,
        ),
        (
            "degenerate y range",
            r#"return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={5,5},
                series={{ name="s", role="primary", points={{0,5}} }} }"#,
        ),
        (
            "filesystem escape attempt",
            r#"local f = io.open("/etc/passwd","r")
               return { title="t", x_label="x", y_label="y", x_range={0,1}, y_range={0,1},
                series={{ name="s", role="primary", points={{0,0}} }} }"#,
        ),
    ];

    println!("== boundary behaviour (every case must be rejected) ==");
    let mut leaked = 0;
    for (name, src) in cases {
        match spec::load(src, &DEMO_DARK, 0.0) {
            Ok(_) => {
                leaked += 1;
                println!("  ACCEPTED (bad!)  {name}");
            }
            Err(e) => println!("  rejected  {name}\n              -> {e}"),
        }
    }
    let big = format!("-- {}\nreturn {{}}", "x".repeat(spec::MAX_SOURCE_BYTES));
    match spec::load(&big, &DEMO_DARK, 0.0) {
        Ok(_) => {
            leaked += 1;
            println!("  ACCEPTED (bad!)  oversized source");
        }
        Err(e) => println!("  rejected  oversized source\n              -> {e}"),
    }

    // A runaway script must be stopped by the heap bound, not by the OOM killer.
    let bomb = r#"local t={} local i=1 while true do t[i]=string.rep("x",1024) i=i+1 end"#;
    match spec::load(bomb, &DEMO_DARK, 0.0) {
        Ok(_) => {
            leaked += 1;
            println!("  ACCEPTED (bad!)  allocation bomb");
        }
        Err(e) => {
            let short: String = e.chars().take(110).collect();
            println!("  rejected  allocation bomb\n              -> {short}");
        }
    }

    // A non-allocating infinite loop: the heap cap cannot see this one.
    let spin = r#"local i=0 while true do i=i+1 end"#;
    match spec::load(spin, &DEMO_DARK, 0.0) {
        Ok(_) => {
            leaked += 1;
            println!("  ACCEPTED (bad!)  non-allocating spin loop");
        }
        Err(e) => {
            let short: String = e.chars().take(110).collect();
            println!("  rejected  non-allocating spin loop\n              -> {short}");
        }
    }

    println!("\n{leaked} case(s) leaked through");
}
