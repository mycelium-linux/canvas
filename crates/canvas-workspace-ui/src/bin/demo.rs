fn main() -> eframe::Result {
    eframe::run_native(
        "Canvas workspace · demo data",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1600.0, 1000.0])
                .with_min_inner_size([680.0, 540.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            Ok(Box::new(
                canvas_workspace_ui::preview_host::App::new(cc).map_err(std::io::Error::other)?,
            ))
        }),
    )
}
