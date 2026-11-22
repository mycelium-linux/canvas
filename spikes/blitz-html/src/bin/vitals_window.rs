//! A native Blitz window for the Soil Vitals view.
//!
//! No browser, no JS. The soil-lang interpreter produces HTML/CSS from the
//! Soil view; Blitz paints it; a click hit-tests a `data-action` attribute and
//! re-runs the Soil view with that action appended.
//!
//! ```sh
//! cargo run --bin vitals-window -- \
//!   --soil /path/to/soil-lang --root /path/to/ajmwagar/soil
//! ```

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, LocalName};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

type SoftContext = softbuffer::Context<Arc<Window>>;
type SoftSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 900;

#[derive(Clone)]
struct Config {
    soil: PathBuf,
    root: PathBuf,
    width: u32,
    height: u32,
}

struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct App {
    config: Config,
    actions: Vec<String>,
    html: String,
    css: String,
    frame: Option<Frame>,
    window: Option<Arc<Window>>,
    context: Option<SoftContext>,
    surface: Option<SoftSurface>,
    pointer: (f32, f32),
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            actions: Vec::new(),
            html: String::new(),
            css: String::new(),
            frame: None,
            window: None,
            context: None,
            surface: None,
            pointer: (0.0, 0.0),
        }
    }

    fn rerender(&mut self) -> Result<(), String> {
        let (html, css) = render_soil(&self.config, &self.actions)?;
        self.html = html;
        self.css = css;
        self.paint_document()?;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(())
    }

    fn document(&self, width: u32, height: u32, scale: f32) -> HtmlDocument {
        let source = format!(
            "<!doctype html><meta charset=utf-8><style>{}</style>{}",
            self.css, self.html
        );
        let viewport = Viewport::new(width, height, scale, ColorScheme::Dark);
        let mut document = HtmlDocument::from_html(
            &source,
            DocumentConfig {
                viewport: Some(viewport),
                ..Default::default()
            },
        );
        document.resolve(0.0);
        document
    }

    fn paint_document(&mut self) -> Result<(), String> {
        let Some(window) = &self.window else {
            return Ok(());
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let scale = window.scale_factor() as f32;
        let mut document = self.document(size.width, size.height, scale);
        let mut renderer = VelloCpuImageRenderer::new(size.width, size.height);
        let mut rgba = Vec::new();
        renderer.render_to_vec(
            |scene| {
                blitz_paint::paint_scene(
                    scene,
                    &mut document,
                    scale as f64,
                    size.width,
                    size.height,
                    0,
                    0,
                )
            },
            &mut rgba,
        );
        self.frame = Some(Frame {
            width: size.width,
            height: size.height,
            rgba,
        });
        Ok(())
    }

    fn paint(&mut self) {
        let (Some(surface), Some(frame), Some(window)) = (
            self.surface.as_mut(),
            self.frame.as_ref(),
            self.window.as_ref(),
        ) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if let Err(error) = surface.resize(width, height) {
            eprintln!("vitals-window: resize: {error}");
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("vitals-window: buffer: {error}");
                return;
            }
        };
        for (pixel, rgba) in buffer.iter_mut().zip(frame.rgba.chunks_exact(4)) {
            *pixel = (u32::from(rgba[0]) << 16) | (u32::from(rgba[1]) << 8) | u32::from(rgba[2]);
        }
        if let Err(error) = buffer.present() {
            eprintln!("vitals-window: present: {error}");
        }
    }

    fn action_at(&self, x: f32, y: f32) -> Option<String> {
        let window = self.window.as_ref()?;
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let document = self.document(size.width, size.height, scale);
        let mut node_id = document.element_from_point(x, y)?;
        loop {
            let node = document.get_node(node_id)?;
            if let Some(action) = node.attr(LocalName::from("data-action")) {
                return Some(action.to_string());
            }
            node_id = node.parent?;
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        let Some(action) = self.action_at(x, y) else {
            return;
        };
        eprintln!("vitals-window: action {action}");
        self.actions.push(action);
        if let Err(error) = self.rerender() {
            eprintln!("vitals-window: render: {error}");
        } else {
            eprintln!(
                "vitals-window: rendered {} action(s), guest={}",
                self.actions.len(),
                self.html.contains("wearer guest")
            );
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Vitals")
            .with_inner_size(LogicalSize::new(
                self.config.width as f64,
                self.config.height as f64,
            ))
            .with_resizable(true);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("vitals-window: window: {error}");
                event_loop.exit();
                return;
            }
        };
        let context = match SoftContext::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("vitals-window: softbuffer context: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match SoftSurface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("vitals-window: softbuffer surface: {error}");
                event_loop.exit();
                return;
            }
        };
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        if let Err(error) = self.rerender() {
            eprintln!("vitals-window: initial render: {error}");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Err(error) = self.paint_document() {
                    eprintln!("vitals-window: rerender: {error}");
                }
                self.paint();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|window| window.scale_factor() as f32)
                    .unwrap_or(1.0);
                self.pointer = (position.x as f32 / scale, position.y as f32 / scale);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.pointer;
                self.click(x, y);
            }
            _ => {}
        }
    }
}

fn render_soil(config: &Config, actions: &[String]) -> Result<(String, String), String> {
    let action_list = actions
        .iter()
        .map(|action| format!("{action:?}"))
        .collect::<Vec<_>>()
        .join(" ");
    let main = format!(
        "(defun main ()\n  (println\n    (json-encode\n      (list\n        (list :css (vitals-ui/styles))\n        (list :html (render-html (vitals-ui/view (vitals-ui/apply-actions (vitals-ui/initial-state) (list {action_list})))))))))\n"
    );
    let path = std::env::temp_dir().join(format!("vitals-window-{}.soil", std::process::id()));
    std::fs::write(&path, main).map_err(|error| format!("write {}: {error}", path.display()))?;
    let output = Command::new(&config.soil)
        .current_dir(&config.root)
        .args([
            "--prelude",
            "core/prelude.soil",
            "--call",
            "main",
            "core/ui.soil",
            "examples/vitals/vitals.soil",
            "examples/vitals/ui.soil",
        ])
        .arg(&path)
        .output()
        .map_err(|error| format!("run {}: {error}", config.soil.display()))?;
    let _ = std::fs::remove_file(&path);
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().last().ok_or("soil produced no output")?;
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|error| format!("parse soil: {error}: {line}"))?;
    let html = value
        .get("html")
        .and_then(|value| value.as_str())
        .ok_or("soil output has no html")?
        .to_string();
    let css = value
        .get("css")
        .and_then(|value| value.as_str())
        .ok_or("soil output has no css")?
        .to_string();
    Ok((html, css))
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut soil = None;
    let mut root = None;
    let mut width = DEFAULT_WIDTH;
    let mut height = DEFAULT_HEIGHT;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--soil" => {
                soil = Some(PathBuf::from(next(args, &mut index, "--soil")?));
            }
            "--root" => {
                root = Some(PathBuf::from(next(args, &mut index, "--root")?));
            }
            "--width" => {
                width = next(args, &mut index, "--width")?
                    .parse()
                    .map_err(|_| "--width must be a number".to_string())?;
            }
            "--height" => {
                height = next(args, &mut index, "--height")?
                    .parse()
                    .map_err(|_| "--height must be a number".to_string())?;
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
        index += 1;
    }
    Ok(Config {
        soil: soil.ok_or("--soil is required")?,
        root: root.ok_or("--root is required")?,
        width,
        height,
    })
}

fn next(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = parse(&args)?;
    if !Path::new(&config.soil).is_file() {
        return Err(format!("soil interpreter not found: {}", config.soil.display()));
    }
    let event_loop = EventLoop::new().map_err(|error| format!("event loop: {error}"))?;
    let mut app = App::new(config);
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("run: {error}"))
}
