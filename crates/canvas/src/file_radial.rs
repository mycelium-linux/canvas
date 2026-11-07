//! DragShift-inspired file gestures, implemented in Canvas's native view plane.
//! Interaction reference: https://github.com/llamaboi55/DragShift
use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};
use std::{
    f32::consts::{FRAC_PI_2, TAU},
    path::PathBuf,
    time::{Duration, Instant},
};

const INNER: f32 = 48.0;
const OUTER: f32 = 148.0;
const MAX_FILES: usize = ::canvas::control::MAX_FILE_ACTION_FILES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    Open,
    Png,
    Jpeg,
    Webp,
    HalfSize,
    Rotate,
    Grayscale,
}

impl FileAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open in Canvas",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::HalfSize => "Half size",
            Self::Rotate => "Rotate 90°",
            Self::Grayscale => "Grayscale",
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::HalfSize => "half",
            Self::Rotate => "rotated",
            Self::Grayscale => "grayscale",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Convert,
    Tools,
}

#[derive(Debug, Default)]
pub struct Menu {
    pub window: Option<iced::window::Id>,
    pub visible: bool,
    pub dropped: bool,
    pub center: Point,
    pub selected: Option<usize>,
    pub page: Page,
    pub files: Vec<PathBuf>,
    pub shift: bool,
    queued: Option<FileAction>,
    last_drop: Option<Instant>,
}

impl Menu {
    pub fn actions(&self) -> &'static [FileAction] {
        match self.page {
            Page::Convert => &[
                FileAction::Open,
                FileAction::Png,
                FileAction::Jpeg,
                FileAction::Webp,
            ],
            Page::Tools => &[
                FileAction::Open,
                FileAction::HalfSize,
                FileAction::Rotate,
                FileAction::Grayscale,
            ],
        }
    }
    pub fn add(&mut self, path: PathBuf) -> Result<(), String> {
        if !self.files.contains(&path) {
            if self.files.len() == MAX_FILES {
                return Err(format!("Drop at most {MAX_FILES} files at a time"));
            }
            self.files.push(path);
        }
        Ok(())
    }
    pub fn show(&mut self, window: iced::window::Id, cursor: Option<Point>, size: Size) {
        self.window = Some(window);
        if self.visible {
            return;
        }
        let point = cursor.unwrap_or(Point::new(size.width / 2.0, size.height / 2.0));
        self.center = Point::new(
            clamp_axis(point.x, size.width),
            if size.height >= OUTER * 2.0 + 72.0 {
                point.y.clamp(OUTER + 8.0, size.height - OUTER - 60.0)
            } else {
                clamp_axis(point.y, size.height)
            },
        );
        self.visible = true;
        self.selected = None;
    }
    pub fn toggle_page(&mut self) {
        self.page = match self.page {
            Page::Convert => Page::Tools,
            Page::Tools => Page::Convert,
        };
        self.selected = None;
    }
    pub fn point(&mut self, point: Point) {
        self.selected = hit(self.center, point, self.actions().len());
    }
    pub fn drop_file(&mut self, path: PathBuf, now: Instant) -> Result<(), String> {
        self.add(path)?;
        // Preserve a chosen slice across the OS's per-file drop events.
        if !self.dropped {
            self.queued = self.selected.map(|i| self.actions()[i]);
        }
        self.dropped = true;
        self.last_drop = Some(now);
        Ok(())
    }
    pub fn choose(&mut self) {
        if self.dropped {
            self.queued = self.selected.map(|i| self.actions()[i]);
        }
    }
    pub fn ready(&mut self, now: Instant) -> Option<(FileAction, Vec<PathBuf>)> {
        if !self
            .last_drop
            .is_some_and(|last| now.saturating_duration_since(last) >= Duration::from_millis(100))
        {
            return None;
        }
        let action = self.queued?;
        let files = std::mem::take(&mut self.files);
        self.cancel();
        Some((action, files))
    }
    pub fn cancel(&mut self) {
        let shift = self.shift;
        *self = Self {
            shift,
            ..Self::default()
        };
    }
    pub fn leave(&mut self) {
        if !self.dropped {
            self.cancel();
        }
    }
}

fn clamp_axis(value: f32, extent: f32) -> f32 {
    if extent < OUTER * 2.0 + 16.0 {
        extent / 2.0
    } else {
        value.clamp(OUTER + 8.0, extent - OUTER - 8.0)
    }
}

fn hit(center: Point, point: Point, count: usize) -> Option<usize> {
    let dx = point.x - center.x;
    let dy = point.y - center.y;
    let distance = dx.hypot(dy);
    if !(INNER..=OUTER).contains(&distance) {
        return None;
    }
    let sector = TAU / count as f32;
    Some((((dy.atan2(dx) + FRAC_PI_2 + sector / 2.0).rem_euclid(TAU)) / sector) as usize % count)
}

pub fn view<'a, Message: 'a>(
    menu: &'a Menu,
    palette: canvas_core::ThemePalette,
) -> Element<'a, Message> {
    canvas(Radial { menu, palette })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

struct Radial<'a> {
    menu: &'a Menu,
    palette: canvas_core::ThemePalette,
}

impl<Message> canvas::Program<Message> for Radial<'_> {
    type State = ();
    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        // The subscription handles input; capturing prevents clicks reaching the workspace underneath.
        matches!(event, canvas::Event::Mouse(_) | canvas::Event::Keyboard(_))
            .then(canvas::Action::capture)
    }
    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let color = |rgb: [u8; 3]| Color::from_rgb8(rgb[0], rgb[1], rgb[2]);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = self.menu.center;
        let palette = self.palette;
        let actions = self.menu.actions();
        let sector = TAU / actions.len() as f32;
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color {
                a: 0.24,
                ..color(palette.background)
            },
        );
        for (index, action) in actions.iter().enumerate() {
            let angle = index as f32 * sector - FRAC_PI_2;
            let start = angle - sector / 2.0 + 0.025;
            let end = angle + sector / 2.0 - 0.025;
            let path = canvas::Path::new(|path| {
                for step in 0..=32 {
                    let a = start + (end - start) * step as f32 / 32.0;
                    let p = Point::new(center.x + OUTER * a.cos(), center.y + OUTER * a.sin());
                    if step == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                for step in (0..=32).rev() {
                    let a = start + (end - start) * step as f32 / 32.0;
                    path.line_to(Point::new(
                        center.x + INNER * a.cos(),
                        center.y + INNER * a.sin(),
                    ));
                }
                path.close();
            });
            let selected = self.menu.selected == Some(index);
            frame.fill(
                &path,
                color(if selected {
                    palette.primary
                } else {
                    palette.surface
                }),
            );
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(color(palette.surface_muted)),
            );
            let radius = (INNER + OUTER) / 2.0;
            frame.fill_text(canvas::Text {
                content: format!("{}\n{}", index + 1, action.label()),
                position: Point::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                ),
                color: color(if selected {
                    palette.background
                } else {
                    palette.text
                }),
                size: 13.0.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        frame.fill(
            &canvas::Path::circle(center, INNER - 4.0),
            color(palette.background_strong),
        );
        frame.fill_text(canvas::Text {
            content: match self.menu.page {
                Page::Convert => "Convert",
                Page::Tools => "Tools",
            }
            .into(),
            position: center,
            color: color(palette.text),
            size: 13.0.into(),
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        let label = if self.menu.files.len() == 1 {
            self.menu.files[0]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .take(44)
                .collect::<String>()
        } else {
            format!("{} files", self.menu.files.len())
        };
        frame.fill_text(canvas::Text {
            content: format!("{label}\nShift: switch tools · 1–4: choose · Esc: cancel"),
            position: Point::new(
                center.x,
                (center.y + OUTER + 16.0).min(bounds.height - 40.0),
            ),
            color: color(palette.text),
            size: 12.0.into(),
            align_x: iced::alignment::Horizontal::Center.into(),
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }
    fn mouse_interaction(&self, _: &(), _: Rectangle, _: mouse::Cursor) -> mouse::Interaction {
        mouse::Interaction::Pointer
    }
}

/// Use Canvas's bounded decoder and the already-adopted image library. Native
/// exports deliberately cover Canvas's image formats, not DragShift's full
/// document toolchain. The source is never opened for writing.
pub fn transform(path: &std::path::Path, action: FileAction) -> Result<PathBuf, String> {
    use image::{DynamicImage, ImageFormat};
    use std::{
        fs::{self, OpenOptions},
        io::{BufWriter, Write},
    };
    let path = super::canonical_regular_file(path).map_err(|error| error.message)?;
    if action == FileAction::Open {
        return Ok(path);
    }
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() == 0 || metadata.len() > super::MAX_IMAGE_BYTES {
        return Err(format!(
            "image must contain 1–{} bytes",
            super::MAX_IMAGE_BYTES
        ));
    }
    let (rgba, _) = super::decode_rgba(&path).map_err(|error| error.message)?;
    let decoded = DynamicImage::ImageRgba8(rgba);
    let (output, format) = match action {
        FileAction::Open => unreachable!(),
        FileAction::Png => (decoded, ImageFormat::Png),
        FileAction::Webp => (decoded, ImageFormat::WebP),
        FileAction::Jpeg => {
            let mut white = image::RgbaImage::from_pixel(
                decoded.width(),
                decoded.height(),
                image::Rgba([255, 255, 255, 255]),
            );
            image::imageops::overlay(&mut white, &decoded.to_rgba8(), 0, 0);
            (
                DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(white).into_rgb8()),
                ImageFormat::Jpeg,
            )
        }
        FileAction::HalfSize => (
            decoded.resize_exact(
                (decoded.width() / 2).max(1),
                (decoded.height() / 2).max(1),
                image::imageops::FilterType::Lanczos3,
            ),
            ImageFormat::Png,
        ),
        FileAction::Rotate => (decoded.rotate90(), ImageFormat::Png),
        FileAction::Grayscale => (decoded.grayscale(), ImageFormat::Png),
    };
    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::WebP => "webp",
        _ => "png",
    };
    let stem = path.file_stem().unwrap_or_default();
    for ordinal in 1..=1000 {
        let mut name = stem.to_os_string();
        name.push(format!("-canvas-{}-{ordinal}.{extension}", action.suffix()));
        let target = path.with_file_name(name);
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create {}: {error}", target.display())),
        };
        let mut writer = BufWriter::new(file);
        let result = output
            .write_to(&mut writer, format)
            .map_err(|error| error.to_string())
            .and_then(|()| writer.flush().map_err(|error| error.to_string()));
        drop(writer);
        if let Err(error) = result {
            fs::remove_file(&target).map_err(|cleanup| {
                format!(
                    "export failed: {error}; could not remove {}: {cleanup}",
                    target.display()
                )
            })?;
            return Err(format!("export failed: {error}"));
        }
        return Ok(target);
    }
    Err("No unused export filename after 1000 attempts".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slices_have_a_dead_zone_and_bounded_targets() {
        let c = Point::new(200.0, 200.0);
        for (index, p) in [
            (200.0, 100.0),
            (300.0, 200.0),
            (200.0, 300.0),
            (100.0, 200.0),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(hit(c, Point::new(p.0, p.1), 4), Some(index));
        }
        assert_eq!(hit(c, c, 4), None);
        assert_eq!(hit(c, Point::ORIGIN, 4), None);
    }
    #[test]
    fn multi_file_drop_is_one_action_even_when_hover_leaves() {
        let mut menu = Menu::default();
        menu.show(iced::window::Id::unique(), None, Size::new(800.0, 600.0));
        menu.selected = Some(1);
        let now = Instant::now();
        menu.drop_file("one.png".into(), now).unwrap();
        menu.leave();
        menu.drop_file("two.png".into(), now + Duration::from_millis(40))
            .unwrap();
        assert!(menu.ready(now + Duration::from_millis(120)).is_none());
        assert_eq!(
            menu.ready(now + Duration::from_millis(150)),
            Some((FileAction::Png, vec!["one.png".into(), "two.png".into()]))
        );
        assert!(!menu.visible);
    }
    #[test]
    fn drop_without_hover_waits_for_an_explicit_choice() {
        let mut menu = Menu::default();
        let now = Instant::now();
        menu.drop_file("one.png".into(), now).unwrap();
        assert!(menu.ready(now + Duration::from_secs(1)).is_none());
        menu.selected = Some(2);
        menu.choose();
        assert_eq!(
            menu.ready(now + Duration::from_secs(1)).unwrap().0,
            FileAction::Jpeg
        );
        assert_eq!(clamp_axis(5.0, 200.0), 100.0);
    }
    #[test]
    fn batch_limit_and_cancel_are_explicit() {
        let mut menu = Menu::default();
        for i in 0..MAX_FILES {
            menu.add(format!("{i}.png").into()).unwrap();
        }
        assert!(menu.add("extra.png".into()).is_err());
        menu.cancel();
        assert!(menu.files.is_empty());
        assert!(menu.queued.is_none());
    }

    #[test]
    fn image_exports_preserve_originals_and_do_not_clobber_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.png");
        image::RgbaImage::from_pixel(8, 4, image::Rgba([100, 20, 0, 0]))
            .save(&path)
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        for (action, size) in [
            (FileAction::Png, (8, 4)),
            (FileAction::Jpeg, (8, 4)),
            (FileAction::Webp, (8, 4)),
            (FileAction::Rotate, (4, 8)),
            (FileAction::HalfSize, (4, 2)),
            (FileAction::Grayscale, (8, 4)),
        ] {
            let first = transform(&path, action).unwrap();
            let bytes = std::fs::read(&first).unwrap();
            let second = transform(&path, action).unwrap();
            assert_ne!(first, second);
            assert_eq!(std::fs::read(first).unwrap(), bytes);
            let decoded = image::open(second).unwrap();
            assert_eq!((decoded.width(), decoded.height()), size);
            if action == FileAction::Jpeg {
                assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [255, 255, 255]);
            }
        }
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn unsupported_files_do_not_create_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        std::fs::write(&path, b"not an image").unwrap();
        assert!(transform(&path, FileAction::Png).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
