//! View-only browser renderer for Canvas.
//!
//! This crate owns no project truth and performs no host actions. It proves the
//! portable renderer boundary: one full-window GPU canvas consuming canonical
//! Canvas palette roles. A retained snapshot/patch transport can replace the
//! demo projection without changing the renderer.

use canvas_core::ThemePalette;
use iced::widget::{button, column, container, row, space, text};
use iced::{Background, Border, Color, Element, Length, Shadow, Theme};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
mod direct;

const JARVIS: &str = include_str!("../../../config/themes/jarvis.toml");
const PIP_BOY: &str = include_str!("../../../config/themes/pip-boy.toml");
const IMPERIAL: &str = include_str!("../../../config/themes/imperial.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeId {
    Jarvis,
    PipBoy,
    Imperial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDirection {
    Input,
    Output,
}

/// Narrow future boundary for JARVIS WebAudio/WebRTC integration. The renderer
/// emits intent; only an authenticated browser host may grant devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioIntent {
    pub direction: AudioDirection,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy)]
enum Message {
    SelectTheme(ThemeId),
    ToggleDetails,
    RequestAudio(AudioDirection),
}

#[derive(Deserialize)]
struct ThemeProjection {
    name: String,
    subtitle: String,
    palette: ThemePalette,
}

struct App {
    active: ThemeId,
    details: bool,
    audio_intent: Option<AudioIntent>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            active: ThemeId::Jarvis,
            details: true,
            audio_intent: None,
        }
    }
}

fn update(app: &mut App, message: Message) {
    match message {
        Message::SelectTheme(theme) => app.active = theme,
        Message::ToggleDetails => app.details = !app.details,
        Message::RequestAudio(direction) => {
            app.audio_intent = Some(AudioIntent {
                direction,
                enabled: true,
            });
        }
    }
}

fn view(app: &App) -> Element<'_, Message> {
    mark_runtime_ready();
    let theme = app.theme();
    let palette = theme.palette;
    let chip = |label, id| {
        button(text(label).size(12).color(rgb(palette.text)))
            .padding([7, 12])
            .on_press(Message::SelectTheme(id))
            .style(move |_, status| {
                let selected = app.active == id;
                iced::widget::button::Style {
                    background: Some(Background::Color(rgb(if selected {
                        palette.primary
                    } else {
                        palette.surface_muted
                    }))),
                    text_color: rgb(if selected {
                        palette.background
                    } else {
                        palette.text
                    }),
                    border: Border {
                        color: rgb(if selected {
                            palette.primary
                        } else {
                            palette.border
                        }),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    shadow: if matches!(status, iced::widget::button::Status::Hovered) {
                        Shadow {
                            color: rgba(palette.primary, 0.28),
                            blur_radius: 12.0,
                            ..Shadow::default()
                        }
                    } else {
                        Shadow::default()
                    },
                    ..Default::default()
                }
            })
    };

    let header = row![
        column![
            text("CANVAS / WEBGPU").size(24).color(rgb(palette.text)),
            text(format!("{} · {}", theme.name, theme.subtitle))
                .size(12)
                .color(rgb(palette.text_muted)),
        ]
        .spacing(2),
        space::horizontal(),
        chip("JARVIS", ThemeId::Jarvis),
        chip("PIP-BOY", ThemeId::PipBoy),
        chip("IMPERIAL", ThemeId::Imperial),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let hero = panel(
        &palette,
        column![
            text("LOCAL GPU VIEW").size(12).color(rgb(palette.primary)),
            text("One document. One canvas. Zero video latency.").size(30).color(rgb(palette.text)),
            text("The browser renders Canvas geometry directly through WASM + WebGPU. Remote pixels remain reserved for video and native applications.")
                .size(15).color(rgb(palette.text_muted)),
        ].spacing(12),
        true,
    );

    let metric = |label: &'static str, value: &'static str, accent: [u8; 3]| {
        panel(
            &palette,
            column![
                text(label).size(11).color(rgb(palette.text_muted)),
                text(value).size(28).color(rgb(accent)),
            ]
            .spacing(6),
            false,
        )
    };
    let metrics = row![
        metric("RENDER PATH", "GPU AUTO", palette.primary),
        metric(
            "VIEW MODE",
            if app.details { "FULL" } else { "COMPACT" },
            palette.success
        ),
        metric("STATE FEED", "DEMO", palette.warning),
    ]
    .spacing(12);

    let audio_status = app.audio_intent.map_or("NOT REQUESTED", |_| "INTENT READY");
    let footer = row![
        button(text(if app.details {
            "COMPACT LOD"
        } else {
            "FULL LOD"
        }))
        .on_press(Message::ToggleDetails),
        button(text("JARVIS MIC")).on_press(Message::RequestAudio(AudioDirection::Input)),
        button(text("JARVIS AUDIO OUT")).on_press(Message::RequestAudio(AudioDirection::Output)),
        space::horizontal(),
        text(format!("AUDIO · {audio_status}"))
            .size(11)
            .color(rgb(palette.text_muted)),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let content = if app.details {
        column![header, hero, metrics, footer].spacing(14)
    } else {
        column![header, hero, footer].spacing(14)
    };

    container(content)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(rgb(palette.background))),
            text_color: Some(rgb(palette.text)),
            ..Default::default()
        })
        .into()
}

#[cfg(target_arch = "wasm32")]
fn mark_runtime_ready() {
    if let Some(status) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("boot-status"))
    {
        let _ = status.set_attribute("data-state", "ready");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn mark_runtime_ready() {}

fn panel<'a>(
    palette: &ThemePalette,
    content: impl Into<Element<'a, Message>>,
    strong: bool,
) -> Element<'a, Message> {
    let palette = *palette;
    container(content)
        .padding(if strong { 24 } else { 16 })
        .width(Length::Fill)
        .height(if strong {
            Length::FillPortion(3)
        } else {
            Length::FillPortion(1)
        })
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(rgb(if strong {
                palette.surface
            } else {
                palette.surface_muted
            }))),
            border: Border {
                color: rgb(if strong {
                    palette.border_strong
                } else {
                    palette.border
                }),
                width: 1.0,
                radius: 5.0.into(),
            },
            shadow: Shadow {
                color: rgba(palette.primary, if strong { 0.16 } else { 0.06 }),
                blur_radius: if strong { 22.0 } else { 8.0 },
                ..Shadow::default()
            },
            ..Default::default()
        })
        .into()
}

impl App {
    fn theme(&self) -> ThemeProjection {
        let source = match self.active {
            ThemeId::Jarvis => JARVIS,
            ThemeId::PipBoy => PIP_BOY,
            ThemeId::Imperial => IMPERIAL,
        };
        toml::from_str(source).expect("bundled Canvas theme packs are validated by native tests")
    }
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

fn rgba(value: [u8; 3], alpha: f32) -> Color {
    Color {
        a: alpha,
        ..rgb(value)
    }
}

fn iced_theme(_: &App) -> Theme {
    Theme::Dark
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() -> Result<(), JsValue> {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        return direct::run();
    }

    #[cfg(not(target_arch = "wasm32"))]
    iced::application(App::default, update, view)
        .title("Canvas Web · LoD Demo")
        .theme(iced_theme)
        .antialiasing(true)
        .run()
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_theme_projects_the_canonical_palette() {
        for id in [ThemeId::Jarvis, ThemeId::PipBoy, ThemeId::Imperial] {
            let app = App {
                active: id,
                ..App::default()
            };
            let theme = app.theme();
            assert!(!theme.name.is_empty());
            assert_ne!(theme.palette.background, theme.palette.text);
        }
    }

    #[test]
    fn audio_controls_emit_intent_without_owning_a_device() {
        let mut app = App::default();
        update(&mut app, Message::RequestAudio(AudioDirection::Input));
        assert_eq!(
            app.audio_intent,
            Some(AudioIntent {
                direction: AudioDirection::Input,
                enabled: true,
            })
        );
    }
}
