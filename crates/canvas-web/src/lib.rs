//! Browser presentation of a Canvas Spore surface.
//!
//! The browser is another Canvas backend, not a mirror of native pixels and
//! not a second UI toolkit. It receives the same scene document a native Spore
//! surface draws and renders it with the same iced primitives. The first scene
//! is the home Spore's fixture; the eventual Luau-in-wasm runtime replaces
//! `HOME_SCENE` with a generated document without changing this renderer.
//!
//! A scene is data. Pressing a Spore control emits a typed `canvas-intent`
//! event to the page and never performs a host action itself.

use canvas_core::{ThemeMetrics, ThemePalette};
use iced::widget::{container, mouse_area, responsive, space, stack, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Point, Size, Task};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use serde::Serialize;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::{JsFuture, spawn_local};

const HOME_SCENE: &str = include_str!("../scenes/home.scene.json");
const JARVIS: &str = include_str!("../../../config/themes/jarvis.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    scene: Scene,
    buttons: Vec<Control>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    kind: String,
    schema: u32,
    nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    /// Decoded for schema completeness; the renderer keys nodes by scene order.
    #[allow(dead_code)]
    id: String,
    transform: Transform,
    #[serde(rename = "type")]
    kind: NodeKind,
    #[serde(default)]
    ink: Option<Ink>,
    #[serde(default)]
    opacity: Option<f32>,
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NodeKind {
    Rect,
    Text,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transform {
    translation: [f32; 3],
    /// Euler XYZ rotation is part of the document format; flat launchers do
    /// not use it yet.
    #[allow(dead_code)]
    rotation: [f32; 3],
    scale: [f32; 3],
}

/// The same semantic roles the native renderer resolves through Coherence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ink {
    Text,
    Surface,
    Background,
    Muted,
    Primary,
    Success,
    Warning,
    Danger,
    Info,
}

/// A host-owned control. The scene carries its pixels; this carries its hit
/// region and the intent it emits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    /// Sent with the browser event so a page can route by control identity.
    #[allow(dead_code)]
    id: String,
    label: String,
    action: String,
    #[serde(default)]
    body: serde_json::Value,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    enabled: bool,
    /// Host presentation hint from the Spore; this renderer draws the scene.
    #[serde(default)]
    #[allow(dead_code)]
    appearance: Option<String>,
}

impl Control {
    fn target(&self) -> &str {
        self.body
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&self.label)
    }
}

#[derive(Debug, Clone)]
struct Intent {
    action: String,
    target: String,
}

#[derive(Debug)]
struct App {
    scene: Arc<Scene>,
    buttons: Arc<Vec<Control>>,
    palette: ThemePalette,
    intent: Option<Intent>,
    /// Last cursor position in scene units, maintained by the pointer layer.
    hover: Option<[f32; 2]>,
}

impl Default for App {
    fn default() -> Self {
        let fixture: Fixture =
            serde_json::from_str(HOME_SCENE).expect("the checked-in home scene is generated JSON");
        assert_eq!(fixture.scene.kind, "scene");
        assert_eq!(fixture.scene.schema, 1);
        let theme: ThemeProjection =
            toml::from_str(JARVIS).expect("the bundled JARVIS theme is validated");
        Self {
            scene: Arc::new(fixture.scene),
            buttons: Arc::new(fixture.buttons),
            palette: theme.palette,
            intent: None,
            hover: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ThemeProjection {
    palette: ThemePalette,
}

#[derive(Debug, Clone)]
enum Message {
    Hover([f32; 2]),
    Press,
}

fn update(app: &mut App, message: Message) {
    match message {
        Message::Hover(position) => app.hover = Some(position),
        Message::Press => {
            let Some(position) = app.hover else {
                return;
            };
            let Some(index) = hit_test(&app.buttons, position) else {
                return;
            };
            let control = &app.buttons[index];
            app.intent = Some(Intent {
                action: control.action.clone(),
                target: control.target().to_owned(),
            });
            emit_intent(control);
        }
    }
}

/// Hit-test the retained control rectangles. The renderer draws a scene; the
/// host owns pointer geometry, and a button's hit region is its rectangle in
/// the same 8-unit space as its pixels.
fn hit_test(controls: &[Control], [x, y]: [f32; 2]) -> Option<usize> {
    controls.iter().position(|control| {
        control.enabled
            && x >= control.x
            && x <= control.x + control.width
            && y <= control.y
            && y >= control.y - control.height
    })
}

fn view(app: &App) -> Element<'_, Message> {
    mark_runtime_ready();
    let scene = Arc::clone(&app.scene);
    let intent = app.intent.clone();
    let palette = app.palette;
    let metrics = ThemeMetrics::shared();

    responsive(move |viewport| {
        let mut order: Vec<usize> = (0..scene.nodes.len()).collect();
        // Farther nodes draw first so closer ones appear on top.
        order.sort_by(|left, right| {
            scene.nodes[*left].transform.translation[2]
                .total_cmp(&scene.nodes[*right].transform.translation[2])
        });
        let nodes: Vec<Element<'static, Message>> = order
            .iter()
            .map(|&index| scene_node(&scene.nodes[index], viewport, &palette, &metrics))
            .collect();

        let mut layers: Vec<Element<'static, Message>> = vec![
            container(stack(nodes))
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        ];

        if let Some(intent) = intent.clone() {
            layers.push(intent_pill(&intent, &palette));
        }

        // The pointer layer is last so it stays above the transient pill; the
        // pill is presentation only and must not intercept cursor motion.
        let pointer = mouse_area(
            container(space::horizontal())
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_move(move |point: Point| Message::Hover(scene_point(point, viewport)))
        .on_press(Message::Press);
        layers.push(pointer.into());

        stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    })
    .into()
}

fn scene_node(
    node: &Node,
    viewport: Size,
    palette: &ThemePalette,
    metrics: &ThemeMetrics,
) -> Element<'static, Message> {
    let [x, y] = scene_position(node.transform.translation, viewport);
    let content: Element<'static, Message> = match node.kind {
        NodeKind::Text => {
            let depth = scene_depth(node.transform.translation[2]);
            let size = (node.transform.scale[1].abs() * metrics.body * depth).max(metrics.micro);
            text(node.text.clone().unwrap_or_default())
                .size(size)
                .color(rgb(scene_ink(node.ink, palette)))
                .into()
        }
        NodeKind::Rect => {
            let width = (node.transform.scale[0].abs() * viewport.width / 8.0).max(1.0);
            let height = (node.transform.scale[1].abs() * viewport.height / 8.0).max(1.0);
            let mut fill = rgb(scene_ink(node.ink, palette));
            fill.a = node.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
            let radius = node.radius.unwrap_or(0.0);
            container(space::horizontal())
                .width(width)
                .height(height)
                .style(move |_| iced::widget::container::Style {
                    background: Some(Background::Color(fill)),
                    border: Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        }
    };

    container(content)
        .width(Length::Shrink)
        .height(Length::Shrink)
        .padding(Padding {
            top: y,
            left: x,
            ..Default::default()
        })
        .into()
}

fn intent_pill<'a>(intent: &Intent, palette: &ThemePalette) -> Element<'a, Message> {
    let palette = *palette;
    let pill = container(
        text(format!("{} → {}", intent.action, intent.target))
            .size(12)
            .color(rgb(palette.text)),
    )
    .padding([8, 12])
    .style(move |_| iced::widget::container::Style {
        background: Some(Background::Color(rgba(palette.surface_muted, 0.96))),
        border: Border {
            color: rgb(palette.primary),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    });

    container(pill)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(18)
        .into()
}

/// Scene space is an 8-unit square with the origin at its centre. Canvas maps
/// it to logical pixels the same way on native and web.
fn scene_point(point: Point, viewport: Size) -> [f32; 2] {
    [
        (point.x / viewport.width.max(1.0)) * 8.0 - 4.0,
        4.0 - (point.y / viewport.height.max(1.0)) * 8.0,
    ]
}

fn scene_position(translation: [f32; 3], viewport: Size) -> [f32; 2] {
    let x_fraction = (translation[0] + 4.0) / 8.0;
    let y_fraction = 1.0 - (translation[1] + 4.0) / 8.0;
    [
        (x_fraction * viewport.width.max(1.0)).max(0.0),
        (y_fraction * viewport.height.max(1.0)).max(0.0),
    ]
}

/// Depth is a gentle perspective scale, not a camera.
fn scene_depth(z: f32) -> f32 {
    0.5 + ((z.clamp(-4.0, 4.0) + 4.0) / 8.0) * 1.5
}

fn scene_ink(ink: Option<Ink>, palette: &ThemePalette) -> [u8; 3] {
    match ink.unwrap_or(Ink::Text) {
        Ink::Text => palette.text,
        Ink::Surface => palette.surface,
        Ink::Background => palette.background,
        Ink::Muted => palette.text_muted,
        Ink::Primary => palette.primary,
        Ink::Success => palette.success,
        Ink::Warning => palette.warning,
        Ink::Danger => palette.danger,
        Ink::Info => palette.info,
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

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct IntentEvent<'a> {
    control: &'a str,
    action: &'a str,
    body: &'a serde_json::Value,
}

#[cfg(target_arch = "wasm32")]
fn emit_intent(control: &Control) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let payload = serde_json::to_string(&IntentEvent {
        control: &control.id,
        action: &control.action,
        body: &control.body,
    });
    let Ok(payload) = payload else {
        return;
    };
    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&detail, &"action".into(), &control.action.clone().into());
    let body = js_sys::JSON::parse(&payload)
        .ok()
        .and_then(|value| js_sys::Reflect::get(&value, &"body".into()).ok())
        .unwrap_or(wasm_bindgen::JsValue::NULL);
    let _ = js_sys::Reflect::set(&detail, &"body".into(), &body);
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&detail);
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("canvas-intent", &init) {
        let _ = window.dispatch_event(&event);
    }
    if let Some(element) = window
        .document()
        .and_then(|document| document.get_element_by_id("intent"))
    {
        let _ =
            element.set_text_content(Some(&format!("{} → {}", control.action, control.target())));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn emit_intent(_control: &Control) {}

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

fn iced_theme(_: &App) -> iced::Theme {
    iced::Theme::Dark
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
#[allow(clippy::missing_errors_doc)]
pub fn run() -> Result<(), wasm_bindgen::JsValue> {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        spawn_local(async {
            let desktop = load_desktop().await;
            if let Err(error) = iced::application(
                move || (desktop.clone(), Task::none()),
                shell_update,
                shell_view,
            )
            .title("Mycelium")
            .theme(shell_theme)
            .font(include_bytes!("../assets/NotoSansSymbols2-Regular.ttf").as_slice())
            .subscription(shell_subscription)
            .antialiasing(true)
            .run()
            {
                web_sys::console::error_1(&format!("canvas shell: {error}").into());
            }
        });
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        iced::application(|| (Shell::demo(), Task::none()), shell_update, shell_view)
            .title("Mycelium")
            .theme(shell_theme)
            .font(include_bytes!("../assets/NotoSansSymbols2-Regular.ttf").as_slice())
            .antialiasing(true)
            .run()
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Hosted Mycelium shell
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct DesktopPayload {
    account: DesktopAccount,
    workspace: DesktopWorkspace,
    #[serde(default)]
    welcome: Option<DesktopWelcome>,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopAccount {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    plan: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopWorkspace {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopWelcome {
    #[serde(default)]
    eyebrow: String,
    headline: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Clone)]
struct Shell {
    user_name: String,
    email: String,
    workspace: String,
    plan: String,
    welcome: DesktopWelcome,
    welcome_frame: u32,
    prompt_open: bool,
    prompt: String,
    prompt_note: String,
}

#[derive(Debug, Clone)]
enum ShellMessage {
    Tick,
    TogglePrompt,
    PromptChanged(String),
    SubmitPrompt,
}

impl Shell {
    fn from_payload(payload: DesktopPayload) -> Self {
        let name = payload.account.name.unwrap_or_else(|| {
            payload
                .account
                .email
                .split('@')
                .next()
                .unwrap_or("there")
                .split(['.', '_', '-'])
                .next()
                .unwrap_or("there")
                .to_owned()
        });
        let mut user_name = name;
        if let Some(first) = user_name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        let welcome = payload.welcome.unwrap_or_else(|| DesktopWelcome {
            eyebrow: "MYCELIUM".to_owned(),
            headline: format!("Welcome, {user_name}."),
            body: "Your desktop is ready.".to_owned(),
        });
        Self {
            user_name,
            email: payload.account.email,
            workspace: payload.workspace.name,
            plan: payload.account.plan,
            welcome,
            welcome_frame: 0,
            prompt_open: true,
            prompt: String::new(),
            prompt_note: "Try “make me a renewal desk”".to_owned(),
        }
    }

    fn demo() -> Self {
        Self::from_payload(DesktopPayload {
            account: DesktopAccount {
                email: "avery@fpl.dev".to_owned(),
                name: Some("Avery".to_owned()),
                plan: "starter".to_owned(),
            },
            workspace: DesktopWorkspace {
                name: "Mycelium".to_owned(),
            },
            welcome: None,
        })
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::demo()
    }
}

fn welcome_alpha(frame: u32) -> f32 {
    let frame = frame as f32;
    if frame < 42.0 {
        1.0
    } else if frame < 84.0 {
        (1.0 - (frame - 42.0) / 42.0).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn shell_update(state: &mut Shell, message: ShellMessage) {
    match message {
        ShellMessage::Tick => {
            if state.welcome_frame < 90 {
                state.welcome_frame += 1;
            }
        }
        ShellMessage::TogglePrompt => state.prompt_open = !state.prompt_open,
        ShellMessage::PromptChanged(value) => state.prompt = value,
        ShellMessage::SubmitPrompt => {
            if !state.prompt.trim().is_empty() {
                state.prompt_note = "Sent. The desktop is listening.".to_owned();
                state.prompt.clear();
            }
        }
    }
}

fn shell_subscription(state: &Shell) -> iced::Subscription<ShellMessage> {
    #[cfg(target_arch = "wasm32")]
    {
        if state.welcome_frame < 90 {
            return iced::time::every(Duration::from_millis(16)).map(|_| ShellMessage::Tick);
        }
    }
    let _ = state;
    iced::Subscription::none()
}

fn shell_theme(_: &Shell) -> iced::Theme {
    iced::Theme::Dark
}

fn shell_button_style(
    palette: ThemePalette,
) -> impl Fn(&iced::Theme, iced::widget::button::Status) -> iced::widget::button::Style + 'static {
    move |_, status| {
        let (background, border) = match status {
            iced::widget::button::Status::Hovered => (
                rgba(palette.surface_muted, 1.0),
                rgba(palette.primary, 0.75),
            ),
            iced::widget::button::Status::Pressed => {
                (rgba(palette.primary, 0.88), rgba(palette.primary, 1.0))
            }
            _ => (rgba(palette.surface, 0.92), rgba(palette.primary, 0.24)),
        };
        iced::widget::button::Style {
            background: Some(Background::Color(background)),
            text_color: rgb(palette.text),
            border: Border {
                color: border,
                width: 1.0,
                radius: 7.0.into(),
            },
            ..Default::default()
        }
    }
}

fn shell_view(state: &Shell) -> Element<'_, ShellMessage> {
    mark_runtime_ready();
    let palette = ThemePalette {
        background: [2, 10, 17],
        background_strong: [1, 6, 10],
        surface: [7, 22, 32],
        surface_muted: [11, 36, 48],
        text: [220, 249, 255],
        text_muted: [117, 172, 186],
        primary: [49, 230, 255],
        success: [49, 230, 255],
        warning: [242, 193, 78],
        danger: [255, 89, 112],
        info: [117, 172, 186],
        border: [19, 72, 91],
        border_strong: [49, 230, 255],
    };
    let width = Length::Fill;

    let top_bar = iced::widget::Row::with_children(vec![
        text("MYCELIUM").size(11).color(rgb(palette.primary)).into(),
        text(state.workspace.clone())
            .size(11)
            .color(rgb(palette.text_muted))
            .into(),
        iced::widget::Space::new().width(Length::Fill).into(),
        text(format!("{} · {}", state.user_name, state.plan))
            .size(11)
            .color(rgb(palette.text_muted))
            .into(),
        iced::widget::button(text("Command").size(11).color(rgb(palette.text)))
            .padding([7, 10])
            .style(shell_button_style(palette))
            .on_press(ShellMessage::TogglePrompt)
            .into(),
    ])
    .spacing(14)
    .align_y(Alignment::Center)
    .padding([12, 18]);

    let top_bar = iced::widget::container(top_bar)
        .width(width)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(rgba(palette.surface, 0.92))),
            border: Border {
                color: rgba(palette.primary, 0.16),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });

    let shortcut = |glyph: &'static str, title: &'static str, detail: &'static str| {
        iced::widget::button(
            iced::widget::Column::with_children(vec![
                text(glyph).size(24).color(rgb(palette.primary)).into(),
                text(title).size(14).color(rgb(palette.text)).into(),
                text(detail).size(11).color(rgb(palette.text_muted)).into(),
            ])
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .width(150)
        .padding([18, 16])
        .style(shell_button_style(palette))
        .on_press(ShellMessage::TogglePrompt)
    };

    let desk = iced::widget::Column::with_children(vec![
        text("YOUR DESK")
            .size(10)
            .color(rgb(palette.primary))
            .into(),
        text("A room for the work.")
            .size(34)
            .color(rgb(palette.text))
            .into(),
        text("The Canvas compositor is running. Prompts, windows, and Spores land here.")
            .size(13)
            .color(rgb(palette.text_muted))
            .into(),
        iced::widget::Space::new().height(18).into(),
        iced::widget::Row::with_children(vec![
            shortcut("✦", "Ask", "Prompt popup").into(),
            shortcut("▤", "Notes", "Coming next").into(),
            shortcut("◫", "Files", "Coming next").into(),
            shortcut("⌘", "Command", "Anything").into(),
        ])
        .spacing(10)
        .align_y(Alignment::Start)
        .into(),
        iced::widget::Space::new().height(Length::Fill).into(),
    ])
    .spacing(8)
    .align_x(Alignment::Center)
    .width(width)
    .height(Length::Fill)
    .padding([62, 28]);

    let desktop = iced::widget::Column::with_children(vec![top_bar.into(), desk.into()])
        .width(width)
        .height(Length::Fill);

    let mut layers: Vec<Element<'_, ShellMessage>> = vec![
        iced::widget::container(desktop)
            .width(width)
            .height(Length::Fill)
            .style(move |_| iced::widget::container::Style {
                background: Some(Background::Color(rgb(palette.background))),
                ..Default::default()
            })
            .into(),
    ];

    if state.prompt_open {
        let input = iced::widget::text_input("Ask Mycelium, or type a command…", &state.prompt)
            .on_input(ShellMessage::PromptChanged)
            .on_submit(ShellMessage::SubmitPrompt)
            .padding([12, 14])
            .size(14)
            .style(move |_, _| iced::widget::text_input::Style {
                background: Background::Color(rgba(palette.surface_muted, 1.0)),
                border: Border {
                    color: rgba(palette.primary, 0.24),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                icon: rgb(palette.primary),
                placeholder: rgba(palette.text_muted, 0.9),
                value: rgb(palette.text),
                selection: rgba(palette.primary, 0.28),
            });
        let prompt = iced::widget::Column::with_children(vec![
            iced::widget::Row::with_children(vec![
                text("MYCELIUM / PROMPT")
                    .size(10)
                    .color(rgb(palette.primary))
                    .into(),
                iced::widget::Space::new().width(Length::Fill).into(),
                iced::widget::button(text("Close").size(10).color(rgb(palette.text_muted)))
                    .padding([5, 8])
                    .style(shell_button_style(palette))
                    .on_press(ShellMessage::TogglePrompt)
                    .into(),
            ])
            .align_y(Alignment::Center)
            .into(),
            input.into(),
            text(state.prompt_note.clone())
                .size(11)
                .color(rgb(palette.text_muted))
                .into(),
        ])
        .spacing(10);
        let prompt = iced::widget::container(prompt)
            .width(560)
            .padding(18)
            .style(move |_| iced::widget::container::Style {
                background: Some(Background::Color(rgba(palette.surface_muted, 0.98))),
                border: Border {
                    color: rgba(palette.primary, 0.55),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba8(0, 0, 0, 0.45),
                    offset: iced::Vector::new(0.0, 14.0),
                    blur_radius: 36.0,
                },
                ..Default::default()
            });
        let prompt_layer = iced::widget::container(prompt)
            .width(width)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(Padding {
                top: 0.0,
                right: 18.0,
                bottom: 52.0,
                left: 18.0,
            });
        layers.push(prompt_layer.into());
    }

    let alpha = welcome_alpha(state.welcome_frame);
    if alpha > 0.0 {
        let welcome = iced::widget::Column::with_children(vec![
            text(if state.welcome.eyebrow.is_empty() {
                "MYCELIUM".to_owned()
            } else {
                state.welcome.eyebrow.clone()
            })
            .size(10)
            .color(rgba(palette.primary, alpha))
            .into(),
            text(state.welcome.headline.clone())
                .size(56)
                .color(rgba(palette.text, alpha))
                .into(),
            text(state.welcome.body.clone())
                .size(14)
                .color(rgba(palette.text_muted, alpha))
                .into(),
        ])
        .spacing(12)
        .align_x(Alignment::Center);
        let welcome_layer = iced::widget::container(welcome)
            .width(width)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| iced::widget::container::Style {
                background: Some(Background::Color(rgba(palette.background, alpha * 0.98))),
                ..Default::default()
            });
        layers.push(welcome_layer.into());
    }

    stack(layers).width(width).height(Length::Fill).into()
}

async fn load_desktop() -> Shell {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Shell::demo();
        };
        let Ok(response) = JsFuture::from(window.fetch_with_str("/v1/desktop")).await else {
            return Shell::demo();
        };
        let Ok(response) = response.dyn_into::<web_sys::Response>() else {
            return Shell::demo();
        };
        if response.status() == 401 {
            let _ = window.location().set_href("/v1/auth/login");
            return Shell::demo();
        }
        let Ok(text) = JsFuture::from(response.text().expect("response text")).await else {
            return Shell::demo();
        };
        let Some(text) = text.as_string() else {
            return Shell::demo();
        };
        match serde_json::from_str::<DesktopPayload>(&text) {
            Ok(payload) => Shell::from_payload(payload),
            Err(error) => {
                console_error_panic_hook::set_once();
                web_sys::console::error_1(&format!("desktop payload: {error}").into());
                Shell::demo()
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Shell::demo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Fixture {
        serde_json::from_str(HOME_SCENE).expect("home scene parses")
    }

    #[test]
    fn the_home_fixture_is_a_scene_document() {
        let fixture = fixture();
        assert_eq!(fixture.scene.kind, "scene");
        assert_eq!(fixture.scene.schema, 1);
        assert!(fixture.scene.nodes.len() > 80, "a launcher is dense");
        assert!(
            fixture.buttons.len() >= 19,
            "every tile and dock item is openable"
        );
    }

    #[test]
    fn the_home_fixture_keeps_the_launcher_landmarks() {
        let fixture = fixture();
        let text_nodes: Vec<&str> = fixture
            .scene
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Text)
            .filter_map(|node| node.text.as_deref())
            .collect();
        for landmark in [
            "TODAY",
            "THIS MACHINE",
            "STORE",
            "SYSTEM",
            "SERVICES",
            "16 Spores",
            "averys-mac-studio",
        ] {
            assert!(
                text_nodes.contains(&landmark),
                "home scene lost {landmark:?}"
            );
        }
    }

    #[test]
    fn every_home_control_opens_one_spore() {
        let fixture = fixture();
        assert_eq!(fixture.buttons.len(), 19);
        for control in &fixture.buttons {
            assert_eq!(control.action, "home.open");
            assert!(!control.target().is_empty());
            assert!(control.body.get("host").is_some(), "a tile names its host");
        }
    }

    #[test]
    fn pointing_at_a_tile_selects_its_control() {
        let fixture = fixture();
        let control = fixture
            .buttons
            .iter()
            .find(|control| control.id == "home-open-dev-fungos-store")
            .expect("the store tile is a control");
        let center = [
            control.x + control.width / 2.0,
            control.y - control.height / 2.0,
        ];
        let hit = hit_test(&fixture.buttons, center).expect("the store tile is hit");
        assert_eq!(fixture.buttons[hit].id, control.id);
        assert_eq!(hit_test(&fixture.buttons, [0.0, 0.0]), None);
    }

    #[test]
    fn scene_point_inverts_scene_position() {
        let viewport = Size::new(800.0, 600.0);
        let pixel = scene_position([1.0, -2.0, 0.0], viewport);
        let scene = scene_point(Point::new(pixel[0], pixel[1]), viewport);
        assert!((scene[0] - 1.0).abs() < 0.001, "x round-trips");
        assert!((scene[1] + 2.0).abs() < 0.001, "y round-trips");
    }

    #[test]
    fn scene_space_maps_its_corners_to_the_viewport() {
        let viewport = Size::new(800.0, 600.0);
        assert_eq!(scene_position([-4.0, 4.0, 0.0], viewport), [0.0, 0.0]);
        assert_eq!(scene_position([4.0, -4.0, 0.0], viewport), [800.0, 600.0]);
        assert_eq!(scene_position([0.0, 0.0, 0.0], viewport), [400.0, 300.0]);
    }
}
