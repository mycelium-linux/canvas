//! `fpl-gfx` clock hosted directly in iced's existing compositor pass.

use canvas_core::VisualTheme;
use fpl_gfx::{AnalogClock, Rect, Renderer, RendererConfig, Scene, Theme, Viewport};
use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use iced_wgpu::wgpu;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct State {
    ticket: u64,
}

#[derive(Debug, Clone)]
struct Program {
    clock: AnalogClock,
    theme: Theme,
}

#[derive(Debug, Clone)]
struct Primitive {
    ticket: u64,
    clock: AnalogClock,
    theme: Theme,
}

struct RetainedRenderer {
    renderer: Renderer,
    generation: u64,
    prepared: bool,
}

struct Pipeline {
    format: wgpu::TextureFormat,
    renderers: BTreeMap<u64, RetainedRenderer>,
    generation: u64,
}

pub(super) fn view<'a, Message: 'a>(
    hour: u8,
    minute: u8,
    second: f32,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let theme = match theme_from_canvas(visual) {
        Ok(theme) => theme,
        Err(error) => return iced::widget::text(format!("Clock theme error: {error}")).into(),
    };
    Shader::new(Program {
        clock: AnalogClock {
            hour,
            minute,
            second,
        },
        theme,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Emergency parity switch while the new path is exercised on real displays.
pub(super) fn legacy_requested() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        std::env::var("CANVAS_CLOCK_RENDERER")
            .is_ok_and(|value| value.eq_ignore_ascii_case("legacy"))
    })
}

fn theme_from_canvas(visual: &VisualTheme) -> Result<Theme, coherence_native::InvalidMetrics> {
    let mut theme = Theme::studio_dark();
    theme.shapes.outline_width = 1.0;
    super::gfx_theme::apply(visual, &mut theme)?;
    // This is a HUD/terminal face: laddered, snappy readouts rather than glides.
    super::gfx_theme::MotionStyle::Terminal.apply(&mut theme);
    Ok(theme)
}

impl<Message> shader::Program<Message> for Program {
    type State = State;
    type Primitive = Primitive;

    fn draw(
        &self,
        state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        Primitive {
            ticket: state.ticket,
            clock: self.clock,
            theme: self.theme.clone(),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            ticket: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl Default for Primitive {
    fn default() -> Self {
        Self {
            ticket: 0,
            clock: AnalogClock {
                hour: 0,
                minute: 0,
                second: 0.0,
            },
            theme: Theme::studio_dark(),
        }
    }
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let _ = device;
        Self {
            format,
            renderers: BTreeMap::new(),
            generation: 0,
        }
    }

    fn trim(&mut self) {
        self.generation = self.generation.saturating_add(1);
        let oldest = self.generation.saturating_sub(2);
        self.renderers
            .retain(|_, retained| retained.generation >= oldest);
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let retained = pipeline
            .renderers
            .entry(self.ticket)
            .or_insert_with(|| RetainedRenderer {
                renderer: Renderer::new(device, RendererConfig::new(pipeline.format)),
                generation: pipeline.generation,
                prepared: false,
            });
        retained.generation = pipeline.generation;
        let mut scene = Scene::with_capacity(65);
        self.clock
            .append(&mut scene, Rect::new(0.0, 0.0, bounds.width, bounds.height));
        retained.prepared = retained
            .renderer
            .prepare(
                device,
                queue,
                Viewport {
                    width: bounds.width,
                    height: bounds.height,
                    scale_factor: viewport.scale_factor(),
                    time_seconds: 0.0,
                },
                &self.theme,
                &scene,
            )
            .is_ok();
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(retained) = pipeline
            .renderers
            .get(&self.ticket)
            .filter(|retained| retained.prepared)
        else {
            return true;
        };
        retained.renderer.draw(render_pass);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{ThemeMode, ThemeSelection};
    use fpl_gfx::{Color, ThemeRole};

    fn srgb(rgb: [u8; 3]) -> Color {
        Color::from_srgb8(rgb[0], rgb[1], rgb[2], 255)
    }

    #[test]
    fn canvas_theme_maps_every_gfx_role_and_validates() {
        let visual =
            ThemeSelection::coherence(ThemeMode::Dark, canvas_core::Accent::Brand).visual_theme();
        let theme = theme_from_canvas(&visual).unwrap();

        assert_eq!(
            theme.color(ThemeRole::Primary),
            srgb(visual.palette.primary)
        );
        assert_eq!(theme.color(ThemeRole::Text), srgb(visual.palette.text));
        assert_eq!(
            theme.color(ThemeRole::Surface),
            srgb(visual.palette.surface)
        );
        assert_eq!(
            theme.color(ThemeRole::TextMuted),
            srgb(visual.palette.text_muted)
        );
        assert_eq!(theme.shapes.radius_large, visual.metrics.radius_lg);
        assert!(theme.validate().is_ok());
        let mut scene = Scene::with_capacity(65);
        AnalogClock {
            hour: 10,
            minute: 9,
            second: 30.5,
        }
        .append(&mut scene, Rect::new(0.0, 0.0, 300.0, 300.0));
        assert_eq!(scene.len(), 65, "face, sixty ticks, three hands and hub");
    }
}
