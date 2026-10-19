//! WebGPU preview of the same host-embedded component. No authentication or I/O.
use wasm_bindgen::prelude::*;
struct App(crate::shell::Shell);
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.show(ctx);
    }
}
#[wasm_bindgen]
pub struct WebPreview {
    runner: eframe::WebRunner,
}
impl Default for WebPreview {
    fn default() -> Self {
        Self::new()
    }
}
#[wasm_bindgen]
impl WebPreview {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            runner: eframe::WebRunner::new(),
        }
    }
    pub async fn start(&mut self, canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
        let shell = crate::shell::Shell::demo().map_err(|e| JsValue::from_str(&e))?;
        self.runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |_| Ok(Box::new(App(shell)))),
            )
            .await
    }
    pub fn destroy(&mut self) {
        self.runner.destroy();
    }
}
