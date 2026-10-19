//! WebGPU preview of the same host-embedded component. No authentication or I/O.
use wasm_bindgen::prelude::*;
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
        self.runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    Ok(Box::new(
                        crate::preview_host::App::new(cc).map_err(std::io::Error::other)?,
                    ))
                }),
            )
            .await
    }
    pub fn destroy(&mut self) {
        self.runner.destroy();
    }
}
