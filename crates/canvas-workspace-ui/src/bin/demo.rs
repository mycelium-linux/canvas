struct App(canvas_workspace_ui::shell::Shell);
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.show(ctx);
    }
}
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
        Box::new(|_| {
            Ok(Box::new(App(
                canvas_workspace_ui::shell::Shell::demo().map_err(std::io::Error::other)?
            )))
        }),
    )
}
