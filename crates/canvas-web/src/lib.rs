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

#[cfg(target_arch = "wasm32")]
use serde::Serialize;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

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
    console_error_panic_hook::set_once();

    iced::application(|| (App::default(), Task::none()), update, view)
        .title("Canvas Web · Home Spore")
        .theme(iced_theme)
        .font(include_bytes!("../assets/NotoSansSymbols2-Regular.ttf").as_slice())
        .antialiasing(true)
        .run()
        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))
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
