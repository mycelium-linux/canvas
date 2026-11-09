//! Paint a Lua bar with iced.
//!
//! The Lua file is the whole view: it calls `canvas.rect` and `canvas.text` in
//! the same 8-unit scene space Canvas uses, and this host turns those calls
//! into pixels. Nothing about the bar's look lives in Rust, which is the point —
//! an agent can patch `ui/bar.lua` and the host should not care.
//!
//! The data arrives as `_G.chrome`, the record `spores/chrome/wit/world.wit`
//! declares. Rust fills it from the system; Lua never reads a file or a clock.

use std::{cell::RefCell, path::Path, rc::Rc};

use iced::widget::canvas;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};

#[derive(Debug, Clone)]
enum Prim {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: Color,
        opacity: f32,
    },
    Text {
        content: String,
        x: f32,
        y: f32,
        px: f32,
        color: Color,
    },
}

fn ink(name: &str) -> Color {
    match name {
        "background" => Color::from_rgb8(0x0d, 0x0f, 0x14),
        "surface" => Color::from_rgb8(0x1e, 0x24, 0x30),
        "surface_raised" => Color::from_rgb8(0x26, 0x2e, 0x3c),
        "border" => Color::from_rgb8(0x2a, 0x32, 0x41),
        "text" => Color::from_rgb8(0xe6, 0xeb, 0xf5),
        "primary" => Color::from_rgb8(0xf2, 0xf5, 0xfb),
        "muted" => Color::from_rgb8(0x8b, 0x96, 0xab),
        "accent" => Color::from_rgb8(0x7a, 0xa2, 0xff),
        _ => Color::from_rgb8(0xe6, 0xeb, 0xf5),
    }
}

#[derive(Clone)]
struct Bar {
    prims: Vec<Prim>,
    width: f32,
    height: f32,
}

impl canvas::Program<()> for Bar {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        for prim in &self.prims {
            match prim {
                Prim::Rect {
                    x,
                    y,
                    w,
                    h,
                    radius,
                    color,
                    opacity,
                } => {
                    let path = canvas::Path::rounded_rectangle(
                        Point::new(*x, *y),
                        Size::new(*w, *h),
                        (*radius).into(),
                    );
                    frame.fill(&path, Color { a: *opacity, ..*color });
                }
                Prim::Text {
                    content,
                    x,
                    y,
                    px,
                    color,
                } => {
                    let size = Pixels(*px);
                    let width = content.chars().count() as f32 * px * 0.56;
                    frame.fill_text(canvas::Text {
                        content: content.clone(),
                        position: Point::new(x - width / 2.0, y - px * 0.68),
                        max_width: self.width,
                        size,
                        color: *color,
                        font: Font::default(),
                        ..canvas::Text::default()
                    });
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

struct Host;

impl Host {
    fn prims(&self, width: f32, height: f32) -> Vec<Prim> {
        let prims: Rc<RefCell<Vec<Prim>>> = Rc::new(RefCell::new(Vec::new()));
        let lua = mlua::Lua::new();
        let globals = lua.globals();

        let canvas_table = lua.create_table().expect("canvas table");
        {
            let out = Rc::clone(&prims);
            let rect = lua
                .create_function(move |_, spec: mlua::Table| {
                    let number = |key: &str| spec.get::<f32>(key).unwrap_or(0.0);
                    let translation: mlua::Table = spec.get("translation").unwrap_or_else(|_| unreachable!());
                    let scale: mlua::Table = spec.get("scale").unwrap_or_else(|_| unreachable!());
                    let x = translation.get::<f32>(1).unwrap_or(0.0);
                    let y = translation.get::<f32>(2).unwrap_or(0.0);
                    let w = scale.get::<f32>(1).unwrap_or(0.0);
                    let h = scale.get::<f32>(2).unwrap_or(0.0);
                    let name: String = spec.get("ink").unwrap_or_default();
                    out.borrow_mut().push(Prim::Rect {
                        x: (x + 4.0) / 8.0 * width - w / 8.0 * width / 2.0,
                        y: (4.0 - y) / 8.0 * height - h / 8.0 * height / 2.0,
                        w: w / 8.0 * width,
                        h: h / 8.0 * height,
                        // A radius in scene units would be sheared by the bar's
                        // aspect; keep circles round by using vertical density.
                        radius: number("radius") / 8.0 * height,
                        color: ink(&name),
                        opacity: number("opacity"),
                    });
                    Ok(())
                })
                .expect("rect fn");
            canvas_table.set("rect", rect).expect("set rect");
        }
        {
            let out = Rc::clone(&prims);
            let text_fn = lua
                .create_function(move |_, spec: mlua::Table| {
                    let translation: mlua::Table = spec.get("translation").unwrap_or_else(|_| unreachable!());
                    let x = translation.get::<f32>(1).unwrap_or(0.0);
                    let y = translation.get::<f32>(2).unwrap_or(0.0);
                    out.borrow_mut().push(Prim::Text {
                        content: spec.get("text").unwrap_or_default(),
                        x: (x + 4.0) / 8.0 * width,
                        y: (4.0 - y) / 8.0 * height,
                        px: spec.get::<f32>("px").unwrap_or(12.0),
                        color: ink(&spec.get::<String>("ink").unwrap_or_default()),
                    });
                    Ok(())
                })
                .expect("text fn");
            canvas_table.set("text", text_fn).expect("set text");
        }
        globals.set("canvas", canvas_table).expect("globals canvas");

        let chrome = lua.create_table().expect("chrome table");
        let workspaces = lua.create_table().expect("workspaces");
        for id in 1..=5 {
            let workspace = lua.create_table().expect("workspace");
            workspace.set("id", id).expect("ws id");
            workspace.set("focused", id == 2).expect("ws focus");
            workspace.set("occupied", id <= 3).expect("ws occupied");
            workspaces.push(workspace).expect("push ws");
        }
        chrome.set("workspaces", workspaces).expect("set ws");

        let active = lua.create_table().expect("active");
        active.set("app", "canvas").expect("app");
        active.set("title", "chrome · bar").expect("title");
        chrome.set("active", active).expect("set active");
        chrome.set("clock", clock()).expect("clock");
        chrome
            .set("battery", battery().unwrap_or_default())
            .expect("battery");
        chrome.set("network", "wi-fi").expect("network");
        globals.set("chrome", chrome).expect("globals chrome");

        let path = std::env::var("BAR_LUA").unwrap_or_else(|_| {
            "../../spores/chrome/ui/bar.lua".to_owned()
        });
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {path}: {error}"));
        let module: mlua::Table = lua.load(&source).eval().expect("bar.lua returns a table");
        let render: mlua::Function = module.get("render").expect("bar.lua exports render");
        render.call::<()>(()).expect("bar render");

        let prims = prims.borrow().clone();
        prims
    }
}

fn clock() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

fn battery() -> Option<String> {
    let output = std::process::Command::new("pmset").arg("-g").arg("batt").output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    // Desktops on AC print no battery line at all: absent is absent, never a
    // fabricated 0% or a bare percent sign on the bar.
    let percent = text
        .split('%')
        .next()
        .and_then(|head| head.rsplit(char::is_whitespace).next())
        .filter(|value| !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()))
        .map(str::to_owned)?;
    let charging = text.contains("charging") || text.contains("AC Power");
    Some(format!("{percent}%{}", if charging { "+" } else { "" }))
}

fn view<'a>(bar: &'a Bar) -> iced::Element<'a, ()> {
    canvas::Canvas::new(bar)
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .into()
}


// ---- headless verification ---------------------------------------------------
//
// A window is how a person looks at the bar; these are how an agent does. The
// same primitives that iced paints can be dumped as a tree or rasterized to a
// PNG, so a change can be checked without a display, a screenshot tool or a
// human in the loop.

fn dump(prims: &[Prim]) {
    let mut out = String::from("{\n  \"prims\": [\n");
    for (index, prim) in prims.iter().enumerate() {
        let line = match prim {
            Prim::Rect { x, y, w, h, radius, color, opacity } => format!(
                "    {{\"kind\":\"rect\",\"x\":{x:.2},\"y\":{y:.2},\"w\":{w:.2},\"h\":{h:.2},\"radius\":{radius:.2},\"color\":[{},{},{},{}],\"opacity\":{opacity:.2}}}",
                (color.r * 255.0).round() as u8,
                (color.g * 255.0).round() as u8,
                (color.b * 255.0).round() as u8,
                (color.a * 255.0).round() as u8
            ),
            Prim::Text { content, x, y, px, color } => format!(
                "    {{\"kind\":\"text\",\"text\":{content:?},\"x\":{x:.2},\"y\":{y:.2},\"px\":{px:.2},\"color\":[{},{},{},{}]}}",
                (color.r * 255.0).round() as u8,
                (color.g * 255.0).round() as u8,
                (color.b * 255.0).round() as u8,
                (color.a * 255.0).round() as u8
            ),
        };
        out.push_str(&line);
        if index + 1 < prims.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}");
    println!("{out}");
}

struct Canvas2d {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    font: fontdue::Font,
}

impl Canvas2d {
    fn new(width: u32, height: u32) -> Self {
        let preferred = std::env::var("BAR_FONT")
            .unwrap_or_else(|_| "/System/Library/Fonts/SFNS.ttf".to_owned());
        let bytes = std::fs::read(&preferred)
            .or_else(|_| std::fs::read("/System/Library/Fonts/Supplemental/Arial.ttf"))
            .expect("a system font to measure the bar with");
        eprintln!("bar: font {preferred} ({} bytes)", bytes.len());
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .expect("parse system font");
        Self { width, height, pixels: vec![0; (width * height * 4) as usize], font }
    }

    /// Coverage-blended pixel write, so text and rounded corners are not jagged.
    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha = (coverage * color.a).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let index = ((y as u32 * self.width + x as u32) * 4) as usize;
        for channel in 0..3 {
            let source = [color.r, color.g, color.b][channel] * 255.0;
            let destination = f32::from(self.pixels[index + channel]);
            self.pixels[index + channel] =
                (source * alpha + destination * (1.0 - alpha)).round() as u8;
        }
        self.pixels[index + 3] = 255;
    }

    fn fill_rounded(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, opacity: f32) {
        let radius = radius.min(w / 2.0).min(h / 2.0).max(0.0);
        let (x0, y0) = (x.floor() as i32, y.floor() as i32);
        let (x1, y1) = ((x + w).ceil() as i32, (y + h).ceil() as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                // 2x2 supersampling: cheap, and enough at bar sizes.
                let mut covered = 0.0;
                for sy in [0.25f32, 0.75] {
                    for sx in [0.25f32, 0.75] {
                        let sample_x = px as f32 + sx;
                        let sample_y = py as f32 + sy;
                        if inside_rounded(sample_x, sample_y, x, y, w, h, radius) {
                            covered += 0.25;
                        }
                    }
                }
                // Premultiply opacity into coverage.
                self.blend(px, py, color, covered * opacity);
            }
        }
    }

    fn text(&mut self, content: &str, center_x: f32, center_y: f32, px: f32, color: Color) {
        // Measure first: a bar centres labels, and a host that guesses width
        // from character count is why bars drift by a few pixels per label.
        let glyphs: Vec<(f32, fontdue::Metrics, Vec<u8>)> = content
            .chars()
            .map(|character| {
                let (metrics, bitmap) = self.font.rasterize(character, px);
                (metrics.advance_width, metrics, bitmap)
            })
            .collect();
        let total: f32 = glyphs.iter().map(|(advance, _, _)| advance).sum();
        let mut pen_x = center_x - total / 2.0;
        let baseline = center_y + px * 0.35;
        for (advance, metrics, bitmap) in glyphs {
            let gx = pen_x + metrics.xmin as f32;
            let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let coverage = f32::from(bitmap[row * metrics.width + column]) / 255.0;
                    self.blend(
                        gx.round() as i32 + column as i32,
                        gy.round() as i32 + row as i32,
                        color,
                        coverage,
                    );
                }
            }
            pen_x += advance;
        }
    }

    fn finish(self, path: &Path) -> anyhow::Result<()> {
        image::save_buffer(
            path,
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(())
    }
}

fn inside_rounded(x: f32, y: f32, rx: f32, ry: f32, w: f32, h: f32, radius: f32) -> bool {
    if x < rx || y < ry || x > rx + w || y > ry + h {
        return false;
    }
    let cx = x.clamp(rx + radius, rx + w - radius);
    let cy = y.clamp(ry + radius, ry + h - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

fn render_png(prims: &[Prim], width: u32, height: u32, path: &Path) -> anyhow::Result<()> {
    let mut canvas = Canvas2d::new(width, height);
    for prim in prims {
        match prim {
            Prim::Rect { x, y, w, h, radius, color, opacity } => {
                canvas.fill_rounded(*x, *y, *w, *h, *radius, *color, *opacity);
            }
            Prim::Text { content, x, y, px, color } => {
                canvas.text(content, *x, *y, *px, *color);
            }
        }
    }
    canvas.finish(path)
}

fn main() -> iced::Result {
    let width = 1200.0;
    let height = 44.0;
    let prims = Host.prims(width, height);

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|argument| argument == "--dump") {
        dump(&prims);
        return Ok(());
    }
    if let Some(index) = args.iter().position(|argument| argument == "--png") {
        let path = args.get(index + 1).cloned().unwrap_or_else(|| "bar.png".into());
        render_png(&prims, width as u32, height as u32, Path::new(&path)).expect("render png");
        println!("wrote {path}");
        return Ok(());
    }

    let bar = Bar {
        prims,
        width,
        height,
    };
    iced::application(
        move || bar.clone(),
        |_state: &mut Bar, ()| iced::Task::none(),
        view,
    )
    .title("chrome · bar")
    .theme(|_: &Bar| Theme::TokyoNight)
    .antialiasing(true)
    .window_size((width, height))
    .run()
}
