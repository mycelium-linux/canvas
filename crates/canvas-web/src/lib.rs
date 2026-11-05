//! View-only browser renderer for Canvas.
//!
//! This crate owns no project truth and performs no host actions. It proves the
//! portable renderer boundary: one full-window GPU canvas consuming canonical
//! Canvas palette roles. A retained snapshot/patch transport can replace the
//! demo projection without changing the renderer.

use canvas_core::ThemePalette;
use iced::widget::{button, column, container, mouse_area, row, space, stack, text};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Subscription, Task, Theme};
use serde::Deserialize;
use wasm_bindgen::prelude::*;


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

#[derive(Debug, Clone)]
enum Message {
    SelectTheme(ThemeId),
    ToggleDetails,
    RequestAudio(AudioDirection),
    Tick,
    Scene(Result<SurfaceScene, String>),
    Hover(Option<String>),
    Activate(String),
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
    surface: Option<String>,
    scene: Option<SurfaceScene>,
    hovered: Option<String>,
    error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            active: ThemeId::Jarvis,
            details: true,
            audio_intent: None,
            surface: surface_from_url(),
            scene: None,
            hovered: None,
            error: None,
        }
    }
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self::default();
        let task = app
            .surface
            .clone()
            .map_or_else(Task::none, |surface| {
                Task::perform(fetch_scene(surface), Message::Scene)
            });
        (app, task)
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.surface.is_some() {
            iced::time::every(std::time::Duration::from_millis(500)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::SelectTheme(theme) => app.active = theme,
        Message::ToggleDetails => app.details = !app.details,
        Message::RequestAudio(direction) => {
            app.audio_intent = Some(AudioIntent {
                direction,
                enabled: true,
            });
        }
        Message::Tick => {
            if let Some(surface) = app.surface.clone() {
                return Task::perform(fetch_scene(surface), Message::Scene);
            }
        }
        Message::Scene(Ok(scene)) => {
            app.scene = Some(scene);
            app.error = None;
            mark_runtime_ready();
        }
        Message::Scene(Err(error)) => app.error = Some(error),
        Message::Hover(target) => app.hovered = target,
        Message::Activate(target) => {
            if let Some(surface) = app.surface.clone() {
                return Task::perform(activate(surface, target), |_| Message::Tick);
            }
        }
    }
    Task::none()
}

fn view(app: &App) -> Element<'_, Message> {
    if app.surface.is_some() {
        return surface_view(app);
    }
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
    console_error_panic_hook::set_once();
    iced::application(App::boot, update, view)
        .subscription(App::subscription)
        .title("Canvas")
        .theme(iced_theme)
        .antialiasing(true)
        .run()
        .map_err(|error| JsValue::from_str(&error.to_string()))
}



// ---- retained Canvas surface scene --------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct SceneNode {
    #[serde(default)]
    transform: Transform,
    content: NodeContent,
}
#[derive(Debug, Clone, Copy, Deserialize)]
struct Transform {
    #[serde(default)]
    translation: [f32; 3],
    #[serde(default)]
    scale: [f32; 3],
}
impl Default for Transform {
    fn default() -> Self {
        Self { translation: [0.0; 3], scale: [1.0; 3] }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NodeContent {
    Group,
    Rect {
        #[serde(default)]
        ink: Option<String>,
        #[serde(default = "one")]
        opacity: f32,
        #[serde(default)]
        radius: f32,
    },
    Text {
        text: String,
        #[serde(default)]
        ink: Option<String>,
    },
    #[serde(other)]
    Other,
}
fn one() -> f32 { 1.0 }

#[derive(Debug, Clone, Deserialize)]
struct Scene { nodes: Vec<SceneNode> }

#[derive(Debug, Clone, Deserialize)]
struct SceneButton {
    id: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    label: String,
    #[serde(default)]
    appearance: String,
    #[serde(default = "one_bool")]
    enabled: bool,
}
fn one_bool() -> bool { true }

#[derive(Debug, Clone, Deserialize)]
struct SceneTheme {
    background: [u8; 3],
    surface: [u8; 3],
    text: [u8; 3],
    muted: [u8; 3],
    primary: [u8; 3],
    success: [u8; 3],
    warning: [u8; 3],
    danger: [u8; 3],
    body: f32,
    micro: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct SurfaceScene {
    #[allow(dead_code)]
    surface_id: String,
    title: String,
    scene: Scene,
    buttons: Vec<SceneButton>,
    theme: SceneTheme,
    #[allow(dead_code)]
    viewport: [f64; 2],
}

impl SceneTheme {
    fn ink(&self, ink: Option<&str>) -> Color {
        let rgb = match ink.unwrap_or("text") {
            "surface" => self.surface,
            "background" => self.background,
            "muted" => self.muted,
            "primary" => self.primary,
            "success" => self.success,
            "warning" => self.warning,
            "danger" => self.danger,
            _ => self.text,
        };
        Color::from_rgb8(rgb[0], rgb[1], rgb[2])
    }
}

fn surface_view(app: &App) -> Element<'_, Message> {
    let Some(scene) = app.scene.clone() else {
        let message = app.error.clone().unwrap_or_else(|| "Connecting to Canvas…".into());
        return container(text(message).size(15))
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    };
    let hovered = app.hovered.clone();
    iced::widget::responsive(move |viewport| scene_content(&scene, hovered.as_deref(), viewport))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn scene_content<'a>(scene: &SurfaceScene, hovered: Option<&str>, viewport: iced::Size) -> Element<'a, Message> {
    let theme = &scene.theme;
    let background = Color::from_rgb8(theme.background[0], theme.background[1], theme.background[2]);
    let mut nodes = scene.scene.nodes.iter().collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.transform.translation[2].total_cmp(&right.transform.translation[2])
    });

    let mut children: Vec<Element<'a, Message>> = Vec::new();
    for node in nodes {
        let t = node.transform;
        let x = (f64::from(t.translation[0]) + 4.0) / 8.0 * f64::from(viewport.width);
        let y = (1.0 - (f64::from(t.translation[1]) + 4.0) / 8.0) * f64::from(viewport.height);
        let z = 0.5 + ((t.translation[2].clamp(-4.0, 4.0) + 4.0) / 8.0) * 1.5;
        match &node.content {
            NodeContent::Rect { ink, opacity, radius } => {
                let color = theme.ink(ink.as_deref());
                let fill = Color { a: opacity.clamp(0.0, 1.0), ..color };
                let radius = *radius;
                let width = f64::from(t.scale[0].abs()) * f64::from(viewport.width) / 8.0;
                let height = f64::from(t.scale[1].abs()) * f64::from(viewport.height) / 8.0;
                let content = container(space())
                    .width(Length::Fixed(width.max(1.0) as f32))
                    .height(Length::Fixed(height.max(1.0) as f32))
                    .style(move |_| iced::widget::container::Style {
                        background: Some(Background::Color(fill)),
                        border: Border { radius: radius.into(), ..Default::default() },
                        ..Default::default()
                    });
                children.push(offset(content, x, y));
            }
            NodeContent::Text { text: label, ink } => {
                let size = (f64::from(t.scale[1].abs()) * f64::from(theme.body) * f64::from(z))
                    .max(f64::from(theme.micro));
                let content = text(label.clone()).size(size as f32).color(theme.ink(ink.as_deref()));
                children.push(offset(content, x, y));
            }
            NodeContent::Group | NodeContent::Other => {}
        }
    }

    for button in &scene.buttons {
        if !button.enabled {
            continue;
        }
        let x = (f64::from(button.x) + 4.0) / 8.0 * f64::from(viewport.width);
        let y = (1.0 - (f64::from(button.y) + 4.0) / 8.0) * f64::from(viewport.height);
        let width = f64::from(button.width.abs()) * f64::from(viewport.width) / 8.0;
        let height = f64::from(button.height.abs()) * f64::from(viewport.height) / 8.0;
        let is_hovered = hovered == Some(button.id.as_str());
        let primary = theme.ink(Some("primary"));
        let fill = if is_hovered {
            Some(Background::Color(Color { a: 0.20, ..primary }))
        } else if button.appearance == "hit" || button.label.is_empty() {
            None
        } else {
            Some(Background::Color(Color { a: 0.18, ..primary }))
        };
        let target = button.id.clone();
        let content = container(space())
            .width(Length::Fixed(width.max(1.0) as f32))
            .height(Length::Fixed(height.max(1.0) as f32))
            .style(move |_| iced::widget::container::Style {
                background: fill,
                border: Border {
                    color: primary,
                    width: if is_hovered { 1.0 } else { 0.0 },
                    radius: 6.0.into(),
                },
                ..Default::default()
            });
        let enter_target = target.clone();
        let area = mouse_area(content)
            .on_enter(Message::Hover(Some(enter_target)))
            .on_exit(Message::Hover(None))
            .on_press(Message::Activate(target));
        children.push(offset(area, x, y));
    }

    let content = container(stack(children))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(background)),
            ..Default::default()
        });
    let footer = container(text(format!("{} · NATIVE SCENE", scene.title.to_uppercase()))
        .size(10)
        .color(theme.ink(Some("muted"))))
        .padding(Padding { right: 8.0, bottom: 6.0, ..Default::default() })
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::Alignment::End)
        .align_y(iced::Alignment::End);
    container(stack![content, footer])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn offset<'a>(content: impl Into<Element<'a, Message>>, x: f64, y: f64) -> Element<'a, Message> {
    container(content)
        .width(Length::Shrink)
        .height(Length::Shrink)
        .padding(Padding { top: y.max(0.0) as f32, left: x.max(0.0) as f32, ..Default::default() })
        .into()
}

#[cfg(target_arch = "wasm32")]
fn surface_from_url() -> Option<String> {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
        .and_then(|params| params.get("surface"))
        .filter(|surface| !surface.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
fn surface_from_url() -> Option<String> {
    std::env::var("CANVAS_WEB_SURFACE").ok().filter(|value| !value.is_empty())
}

#[cfg(target_arch = "wasm32")]
async fn fetch_scene(surface: String) -> Result<SurfaceScene, String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("window unavailable")?;
    let url = format!(
        "http://127.0.0.1:8090/v1/scene?surface={}&t={}",
        js_sys::encode_uri_component(&surface),
        js_sys::Date::now()
    );
    let response = JsFuture::from(window.fetch_with_str(&url))
        .await
        .map_err(|_| "scene fetch failed".to_string())?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "scene response invalid".to_string())?;
    if !response.ok() {
        return Err(format!("Canvas answered {}", response.status()));
    }
    let body = JsFuture::from(response.text().map_err(|_| "scene body unavailable".to_string())?)
        .await
        .map_err(|_| "scene body failed".to_string())?
        .as_string()
        .ok_or("scene body was not text")?;
    serde_json::from_str(&body).map_err(|error| format!("scene JSON: {error}"))
}

#[cfg(not(target_arch = "wasm32"))]
async fn fetch_scene(_surface: String) -> Result<SurfaceScene, String> {
    Err("Canvas Web scenes require wasm32".into())
}

#[cfg(target_arch = "wasm32")]
async fn activate(surface: String, target: String) -> Result<(), String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("window unavailable")?;
    let url = format!(
        "http://127.0.0.1:8090/v1/activate?surface={}&target={}",
        js_sys::encode_uri_component(&surface),
        js_sys::encode_uri_component(&target)
    );
    let _ = JsFuture::from(window.fetch_with_str(&url)).await;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn activate(_surface: String, _target: String) -> Result<(), String> {
    Ok(())
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
