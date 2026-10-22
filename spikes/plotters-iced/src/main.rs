//! Spike: can plotters-iced2 draw Canvas-quality charts on iced 0.14, driven
//! from Lua?
//!
//!   cargo run --                 launch the iced/wgpu window (live-reloads the script)
//!   cargo run -- --audit         primitive counts + dropped-request report, no GPU
//!   cargo run -- --contract      exercise the Lua validation boundary

mod audit;
mod modes;
mod palette;
mod render;
mod spec;

use iced::widget::{Column, Container, Text};
use iced::{Element, Length, Task};
use palette::{DEMO_DARK, Palette};
use plotters::chart::ChartBuilder;
use plotters_backend::DrawingBackend;
use plotters_iced2::{Chart, ChartWidget, Renderer};
use spec::PlotSpec;
use std::time::{Duration, Instant};

use modes::read_script;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "--audit" => modes::audit_mode(),
        "--contract" => modes::contract_mode(),
        _ => gui_mode(),
    }
}

// ------------------------------------------------------------------ gui mode

#[derive(Debug, Clone)]
enum Message {
    Tick,
    /// Ground truth: the composited wgpu surface, read back by iced itself.
    Shot(iced::window::Screenshot),
}

/// 32-bit BMP so the spike needs no image encoder; `sips` converts it to PNG.
fn write_bmp(path: &str, w: u32, h: u32, rgba: &[u8]) {
    let pixel_bytes = (w as usize) * (h as usize) * 4;
    let mut out = Vec::with_capacity(54 + pixel_bytes);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixel_bytes) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes()); // negative = top-down
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    std::fs::write(path, out).expect("write bmp");
}

struct PlotView {
    spec: PlotSpec,
    palette: Palette,
    cache: iced::widget::canvas::Cache,
    /// `--ydesc` selects plotters' own rotated y-axis description.
    rotate_y_desc: bool,
}

impl Chart<Message> for PlotView {
    type State = ();

    fn draw<R: Renderer, F: Fn(&mut iced::widget::canvas::Frame)>(
        &self,
        renderer: &R,
        bounds: iced::Size,
        draw_fn: F,
    ) -> iced::widget::canvas::Geometry {
        renderer.draw_cache(&self.cache, bounds, draw_fn)
    }

    fn build_chart<DB: DrawingBackend>(&self, _state: &Self::State, builder: ChartBuilder<DB>) {
        render::draw_spec(&self.spec, &self.palette, builder, self.rotate_y_desc);
    }
}

struct App {
    view: PlotView,
    started: Instant,
    status: String,
    ticks: u32,
    shot: bool,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let palette = DEMO_DARK;
        let spec = spec::load(&read_script(), &palette, 0.0).expect("initial plot script");
        (
            Self {
                view: PlotView {
                    spec,
                    palette,
                    cache: Default::default(),
                    rotate_y_desc: std::env::args().any(|a| a == "--ydesc"),
                },
                started: Instant::now(),
                status: String::from("loaded"),
                ticks: 0,
                shot: std::env::args().any(|a| a == "--shot"),
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Shot(shot) => {
                write_bmp(
                    "/tmp/spike-iced-surface.bmp",
                    shot.size.width,
                    shot.size.height,
                    &shot.rgba,
                );
                println!(
                    "wrote /tmp/spike-iced-surface.bmp {}x{} scale {}",
                    shot.size.width, shot.size.height, shot.scale_factor
                );
                return iced::exit();
            }
            Message::Tick => {
                let tick = self.started.elapsed().as_secs_f64();
                // Re-read from disk every frame: edit the .lua and watch it change.
                let began = Instant::now();
                match spec::load(&read_script(), &self.view.palette, tick) {
                    Ok(spec) => {
                        self.view.spec = spec;
                        self.status = format!(
                            "{} series / {} points, lua {:?}",
                            self.view.spec.series.len(),
                            self.view.spec.point_count(),
                            began.elapsed()
                        );
                    }
                    // Fail loud: the chart freezes and the error is on screen.
                    Err(e) => self.status = format!("SCRIPT ERROR: {e}"),
                }
                self.view.cache.clear();
                self.ticks += 1;
                // Give wgpu a few frames to present before reading the surface.
                if self.shot && self.ticks == 10 {
                    return iced::window::latest()
                        .and_then(iced::window::screenshot)
                        .map(Message::Shot);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let [r, g, b] = self.view.palette.text_muted;
        Container::new(
            Column::new()
                .spacing(8)
                .push(ChartWidget::new(&self.view).height(Length::Fill))
                .push(
                    Text::new(&self.status)
                        .size(12)
                        .color(iced::Color::from_rgb8(r, g, b)),
                ),
        )
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn gui_mode() {
    let [r, g, b] = DEMO_DARK.background;
    iced::application(App::new, App::update, App::view)
        .title("spike: plotters-iced2 on iced 0.14")
        .antialiasing(true)
        // Pinned so the spike can screenshot exactly this region.
        .window_size(iced::Size::new(1100.0, 520.0))
        .position(iced::window::Position::Specific(iced::Point::new(40.0, 60.0)))
        .style(move |_state, _theme| iced::theme::Style {
            background_color: iced::Color::from_rgb8(r, g, b),
            text_color: iced::Color::from_rgb8(
                DEMO_DARK.text[0],
                DEMO_DARK.text[1],
                DEMO_DARK.text[2],
            ),
        })
        .subscription(|_| iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick))
        .run()
        .expect("iced application");
}
