//! Retained theme images and protocol-owned client cursor surfaces. Pointer
//! coordinates and hotspots are logical; Smithay handles buffer scale/transform.
use super::*;
use smithay::{
    backend::renderer::element::{
        Kind,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        surface::render_elements_from_surface_tree,
    },
    desktop::utils::send_frames_surface_tree,
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    utils::IsAlive,
};
use std::io::Read;

smithay::backend::renderer::element::render_elements! {
    pub(super) NativeRenderElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
}

struct ThemeImage {
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
}

pub(super) struct CursorState {
    status: CursorImageStatus,
    theme: xcursor::CursorTheme,
    size: u32,
    images: std::collections::HashMap<CursorIcon, ThemeImage>,
}

impl CursorState {
    pub fn new() -> Result<Self, String> {
        let name = match std::env::var("XCURSOR_THEME") {
            Ok(name) => name,
            Err(std::env::VarError::NotPresent) => "default".into(),
            Err(error) => return Err(format!("XCURSOR_THEME: {error}")),
        };
        let size = match std::env::var("XCURSOR_SIZE") {
            Ok(value) => value
                .parse::<u32>()
                .ok()
                .filter(|value| (1..=256).contains(value))
                .ok_or("XCURSOR_SIZE must be an integer in 1..=256")?,
            Err(std::env::VarError::NotPresent) => 24,
            Err(error) => return Err(format!("XCURSOR_SIZE: {error}")),
        };
        let mut cursor = Self {
            status: CursorImageStatus::default_named(),
            theme: xcursor::CursorTheme::load(&name),
            size,
            images: Default::default(),
        };
        cursor.load_icon(CursorIcon::Default)?;
        Ok(cursor)
    }

    fn load_icon(&mut self, icon: CursorIcon) -> Result<(), String> {
        if self.images.contains_key(&icon) {
            return Ok(());
        }
        let path = std::iter::once(icon.name())
            .chain(icon.alt_names().iter().copied())
            .find_map(|name| self.theme.load_icon(name))
            .ok_or_else(|| format!("cursor theme is missing {}", icon.name()))?;
        let file = std::fs::File::open(&path)
            .map_err(|error| format!("cursor {}: {error}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cursor {}: {error}", path.display()))?;
        let image = bounded_theme_images(&bytes)?
            .into_iter()
            .min_by_key(|image| image.size.abs_diff(self.size))
            .ok_or("cursor contains no images")?;
        if image.width > 256 || image.height > 256 {
            return Err("cursor image exceeds 256x256".into());
        }
        // Xcursor pixels are little-endian ARGB words (BGRA bytes on our
        // little-endian Linux targets). The parser's pixels_rgba field holds
        // those original file bytes; pixels_argb is a channel-rotated copy.
        let pixels: Vec<u8> = image
            .pixels_rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| u32::from_le_bytes(*pixel).to_ne_bytes())
            .collect();
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (image.width as i32, image.height as i32),
            1,
            Transform::Normal,
            None,
        );
        self.images.insert(
            icon,
            ThemeImage {
                buffer,
                hotspot: (image.xhot as i32, image.yhot as i32).into(),
            },
        );
        Ok(())
    }

    pub fn set(&mut self, status: CursorImageStatus, output: Option<&Output>) {
        let previous = self.surface();
        let next = match &status {
            CursorImageStatus::Surface(surface) => Some(surface),
            _ => None,
        };
        let changed_surface = previous.as_ref() != next;
        if changed_surface
            && let Some(previous) = previous
            && let Some(output) = output
        {
            output.leave(&previous);
        }
        if let CursorImageStatus::Named(icon) = status {
            if let Err(error) = self.load_icon(icon) {
                eprintln!("native cursor: {error}; using default theme image");
                self.status = CursorImageStatus::default_named();
                return;
            }
        }
        self.status = status;
        if changed_surface
            && let Some(surface) = self.surface()
            && let Some(output) = output
        {
            output.enter(&surface);
        }
    }

    pub fn refresh(&mut self) {
        if let CursorImageStatus::Surface(surface) = &self.status
            && !surface.alive()
        {
            self.status = CursorImageStatus::default_named();
        }
    }

    pub fn surface(&self) -> Option<WlSurface> {
        match &self.status {
            CursorImageStatus::Surface(surface) if surface.alive() => Some(surface.clone()),
            _ => None,
        }
    }

    fn hotspot(&self) -> Point<i32, Logical> {
        match &self.status {
            CursorImageStatus::Surface(surface) if surface.alive() => {
                smithay::wayland::compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|data| data.lock().unwrap().hotspot)
                        .unwrap_or_default()
                })
            }
            CursorImageStatus::Named(icon) => self
                .images
                .get(icon)
                .map(|image| image.hotspot)
                .unwrap_or_default(),
            _ => Point::default(),
        }
    }

    pub fn snapshot(&self, location: Point<f64, Logical>, active: bool) -> Value {
        let hotspot = self.hotspot();
        let origin = location - hotspot.to_f64();
        json!({"mode": if !active { "hidden" } else { match &self.status {
            CursorImageStatus::Hidden => "hidden", CursorImageStatus::Named(_) => "named",
            CursorImageStatus::Surface(_) => "surface" } },
            "position": [location.x, location.y], "hotspot": [hotspot.x, hotspot.y],
            "name": match &self.status { CursorImageStatus::Named(icon) => Some(icon.name()), _ => None },
            "origin": [origin.x, origin.y]})
    }

    pub fn elements(
        &self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
    ) -> Result<Vec<NativeRenderElement>, String> {
        let origin = (location - self.hotspot().to_f64()).to_physical_precise_round(1.0);
        match &self.status {
            CursorImageStatus::Hidden => Ok(vec![]),
            CursorImageStatus::Surface(surface) if surface.alive() => {
                Ok(render_elements_from_surface_tree(
                    renderer,
                    surface,
                    origin,
                    1.0,
                    1.0,
                    Kind::Cursor,
                ))
            }
            CursorImageStatus::Named(icon) => {
                let image = self
                    .images
                    .get(icon)
                    .ok_or("missing retained cursor image")?;
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    origin.to_f64(),
                    &image.buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                )
                .map(|element| vec![element.into()])
                .map_err(|error| format!("cursor texture import: {error}"))
            }
            _ => Ok(vec![]),
        }
    }
}

pub(super) fn send_cursor_frame(surface: &WlSurface, output: &Output, elapsed: Duration) {
    send_frames_surface_tree(
        surface,
        output,
        elapsed,
        Some(Duration::from_millis(16)),
        |_, _| Some(output.clone()),
    );
}

/// Admission bounds only; xcursor still owns decoding. Its decoder allocates
/// from declared dimensions before detecting a truncated pixel payload.
fn bounded_theme_images(bytes: &[u8]) -> Result<Vec<xcursor::parser::Image>, String> {
    if bytes.len() > 1024 * 1024 {
        return Err("cursor file exceeds 1 MiB".into());
    }
    let word = |offset: usize| -> Result<u32, String> {
        let end = offset.checked_add(4).ok_or("cursor offset overflow")?;
        let value = bytes.get(offset..end).ok_or("truncated cursor header")?;
        Ok(u32::from_le_bytes(value.try_into().unwrap()))
    };
    let header = word(4)? as usize;
    let count = word(12)? as usize;
    if header < 16 || header > bytes.len() || count > 64 {
        return Err("cursor header/table exceeds admission bounds".into());
    }
    for index in 0..count {
        let toc = header + index * 12;
        if word(toc)? != 0xfffd_0002 {
            continue;
        }
        let position = word(toc + 8)? as usize;
        if position > bytes.len() {
            return Err("cursor image offset exceeds file".into());
        }
        let width = word(position + 16)?;
        let height = word(position + 20)?;
        if !(1..=256).contains(&width) || !(1..=256).contains(&height) {
            return Err("cursor image must be 1..=256 pixels per axis".into());
        }
        if position + 36 + width as usize * height as usize * 4 > bytes.len() {
            return Err("cursor pixel data exceeds file".into());
        }
    }
    xcursor::parser::parse_xcursor(bytes).ok_or_else(|| "invalid Xcursor file".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"Xcur".to_vec();
        for word in [
            16u32,
            0x10000,
            1,
            0xfffd_0002,
            24,
            28,
            36,
            0xfffd_0002,
            24,
            1,
            width,
            height,
            0,
            0,
            0,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn theme_admission_rejects_allocation_bombs_and_truncated_payloads() {
        let mut bytes = file(1, 1);
        assert!(bounded_theme_images(&bytes).is_err());
        bytes.extend_from_slice(&[0, 0, 0, 255]);
        assert_eq!(
            bounded_theme_images(&bytes).unwrap()[0].pixels_rgba,
            [0, 0, 0, 255]
        );
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(bounded_theme_images(&bytes).is_err());
        assert!(bounded_theme_images(&file(32767, 32767)).is_err());
        assert!(bounded_theme_images(&vec![0; 1024 * 1024 + 1]).is_err());
    }
    #[test]
    fn retained_theme_image_and_hotspot_are_stable_across_movement() {
        let mut cursor = CursorState::new().unwrap();
        let count = cursor.images.len();
        cursor.load_icon(CursorIcon::Default).unwrap();
        assert_eq!(cursor.images.len(), count);
        cursor.images.get_mut(&CursorIcon::Default).unwrap().hotspot = (2, 3).into();
        let before = cursor.snapshot((10.0, 20.0).into(), true);
        assert_eq!(before["origin"], json!([8.0, 17.0]));
        let after = cursor.snapshot((20.0, 40.0).into(), true);
        assert_eq!(after["origin"], json!([18.0, 37.0]));
        assert_eq!(cursor.images.len(), count);
        assert_eq!(
            cursor.snapshot((20.0, 40.0).into(), false)["mode"],
            "hidden"
        );
        cursor.set(CursorImageStatus::Hidden, None);
        assert!(cursor.surface().is_none());
        assert_eq!(cursor.snapshot((20.0, 40.0).into(), true)["mode"], "hidden");
    }

    #[test]
    #[ignore = "requires an EGL hardware device; private offscreen buffer only, never a display seat"]
    fn offscreen_cursor_pixels_preserve_alpha_color_and_hotspot() {
        use smithay::backend::{
            egl::EGLDevice,
            renderer::{
                Bind, ExportMem, Frame, Offscreen, Renderer,
                element::{Element, RenderElement},
                gles::GlesTexture,
            },
        };
        let device = EGLDevice::enumerate()
            .unwrap()
            .find(|device| !device.is_software())
            .expect("EGL hardware device");
        eprintln!(
            "cursor offscreen EGL device: {:?}",
            device.render_device_path()
        );
        // SAFETY: this process enumerated the device; its display/context
        // remain owned on this thread, without acquiring a display seat.
        let display = unsafe { EGLDisplay::new(device) }.unwrap();
        let context = EGLContext::new(&display).unwrap();
        // SAFETY: exclusive owned context, confined to this test thread.
        let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
        let mut texture: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (64, 64).into())
            .unwrap();
        let mut cursor = CursorState::new().unwrap();
        // Premultiplied 50%-alpha red in native ARGB words, independent of
        // theme art. Detect swapped channels or straight-alpha import.
        let pixel = 0x8080_0000u32.to_ne_bytes();
        cursor.images.insert(
            CursorIcon::Default,
            ThemeImage {
                buffer: MemoryRenderBuffer::from_slice(
                    &pixel.repeat(4),
                    Fourcc::Argb8888,
                    (2, 2),
                    1,
                    Transform::Normal,
                    None,
                ),
                hotspot: (2, 3).into(),
            },
        );
        let elements = cursor.elements(&mut renderer, (20.0, 20.0).into()).unwrap();
        let geometry = elements[0].geometry(1.0.into());
        assert_eq!(geometry.loc, (18, 17).into());
        assert_eq!(geometry.size, (2, 2).into());
        let mut target = renderer.bind(&mut texture).unwrap();
        let mut frame = renderer
            .render(&mut target, (64, 64).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(
                Color32F::new(0.0, 0.0, 1.0, 1.0),
                &[smithay::utils::Rectangle::from_size((64, 64).into())],
            )
            .unwrap();
        for element in &elements {
            element
                .draw(
                    &mut frame,
                    element.src(),
                    element.geometry(1.0.into()),
                    &[smithay::utils::Rectangle::from_size((2, 2).into())],
                    &[],
                )
                .unwrap();
        }
        let sync = frame.finish().unwrap();
        renderer.wait(&sync).unwrap();
        let mapping = renderer
            .copy_framebuffer(
                &target,
                smithay::utils::Rectangle::from_size((64, 64).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        // Readback can be flipped; verify blended pixels without guessing
        // mapping origin. Element geometry separately verifies the hotspot.
        let blended = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                pixel[0].abs_diff(128) <= 1
                    && pixel[1] == 0
                    && pixel[2].abs_diff(127) <= 1
                    && pixel[3] == 255
            })
            .count();
        assert_eq!(blended, 4, "cursor alpha/channel mismatch");
        assert_eq!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| **pixel == [0, 0, 255, 255])
                .count(),
            4092
        );
        cursor.set(CursorImageStatus::Hidden, None);
        assert!(
            cursor
                .elements(&mut renderer, (20.0, 20.0).into())
                .unwrap()
                .is_empty()
        );
    }
}
