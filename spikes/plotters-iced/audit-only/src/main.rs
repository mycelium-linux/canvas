//! Headless half of the spike: identical spec/render/audit code, no iced.
#[path = "../../src/audit.rs"]
mod audit;
#[path = "../../src/modes.rs"]
mod modes;
#[path = "../../src/palette.rs"]
mod palette;
#[path = "../../src/render.rs"]
mod render;
#[path = "../../src/spec.rs"]
mod spec;

mod svgish;

use plotters::chart::ChartBuilder;
use plotters::drawing::IntoDrawingArea;

fn svg_mode() {
    let source = modes::read_script();
    let spec = spec::load(&source, &palette::DEMO_DARK, 0.0).expect("plot script");

    for (faithful, rotate, name) in [
        (true, true, "a-as-iced-draws-it-ydesc"),
        (true, false, "b-as-iced-draws-it-caption"),
        (false, false, "c-with-metrics-bug-fixed"),
        // Wall-scale type: the measurement bug's blast radius grows with the
        // gap between the requested size and iced's 16px default.
        (true, false, "d-wall-type-as-iced-draws-it"),
        (false, false, "e-wall-type-bug-fixed"),
    ] {
        if name.starts_with("d-") || name.starts_with("e-") {
            unsafe {
                std::env::set_var("SPIKE_LABEL_SIZE", "34");
                std::env::set_var("SPIKE_TITLE_SIZE", "46");
            }
        }
        let (w, h) = (1100u32, 500u32);
        let backend = svgish::SvgishBackend::new(w, h, palette::DEMO_DARK.background, faithful);
        let body = backend.handle();
        {
            let area = backend.into_drawing_area();
            render::draw_spec(&spec, &palette::DEMO_DARK, ChartBuilder::on(&area), rotate);
            area.present().expect("present");
        }
        let out = format!("/tmp/spike-plot-{name}.svg");
        std::fs::write(&out, svgish::SvgishBackend::wrap(w, h, &body.borrow())).expect("write svg");
        println!("wrote {out}");
    }
}

fn main() {
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "--contract" => modes::contract_mode(),
        "--svg" => svg_mode(),
        _ => modes::audit_mode(),
    }
}
