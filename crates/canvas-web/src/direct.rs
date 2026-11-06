//! Browser presentation modes for Canvas.
//!
//! The default is an exact projection of native Canvas pixels supplied by the
//! owner-local bridge. The independent Canvas2D renderer is retained only as
//! an explicit low-detail mode for weak or disconnected clients.

use super::{App, AudioDirection, Message, ThemeId, ThemeProjection, update};
use serde::Deserialize;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, KeyboardEvent, MouseEvent,
    PointerEvent, WheelEvent,
};

const TV_WORKSPACE: &str = include_str!("../../../config/workspaces/tv/transmog.json");

#[derive(Deserialize)]
struct Workspace {
    project_id: String,
    surfaces: Vec<Surface>,
    layout: Layout,
}
#[derive(Deserialize)]
struct Surface {
    id: String,
    title: String,
    content: serde_json::Value,
}
#[derive(Deserialize)]
struct Layout {
    placements: Vec<Placement>,
}
#[derive(Deserialize)]
struct Placement {
    surface_id: String,
    bounds: Bounds,
}
#[derive(Deserialize)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub fn run() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("window unavailable")?;
    let document = window.document().ok_or("document unavailable")?;
    let canvas = document
        .get_element_by_id("canvas-web")
        .ok_or("#canvas-web missing")?
        .dyn_into::<HtmlCanvasElement>()?;
    let context = canvas
        .get_context("2d")?
        .ok_or("Canvas2D unavailable")?
        .dyn_into::<CanvasRenderingContext2d>()?;
    let app = Rc::new(RefCell::new(App::default()));
    let lod = Rc::new(Cell::new(false));
    let surface = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
        .and_then(|params| params.get("surface"))
        .filter(|surface| !surface.is_empty());
    resize(&canvas)?;
    if let Some(surface) = surface.clone() {
        start_surface_scene(&canvas, &context, surface)?;
        set_status("SURFACE · LIVE SCENE", "ready");
    } else {
        start_native_mirror(&canvas, &context, Rc::clone(&lod), None)?;
        set_status("EXACT NATIVE MIRROR · L TOGGLE LOD", "ready");
    }

    let click_canvas = canvas.clone();
    let click_context = context.clone();
    let click_app = Rc::clone(&app);
    let click_lod = Rc::clone(&lod);
    let click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let sx = f64::from(click_canvas.width()) / f64::from(click_canvas.client_width().max(1));
        let sy = f64::from(click_canvas.height()) / f64::from(click_canvas.client_height().max(1));
        let (x, y) = (
            f64::from(event.offset_x()) * sx,
            f64::from(event.offset_y()) * sy,
        );
        let width = f64::from(click_canvas.width());
        let message = if y < 48.0 {
            if x > width - 386.0 && x < width - 274.0 {
                Some(Message::SelectTheme(ThemeId::Jarvis))
            } else if x > width - 264.0 && x < width - 152.0 {
                Some(Message::SelectTheme(ThemeId::PipBoy))
            } else if x > width - 142.0 {
                Some(Message::SelectTheme(ThemeId::Imperial))
            } else {
                None
            }
        } else if y > f64::from(click_canvas.height()) - 50.0 {
            if x < 150.0 {
                Some(Message::ToggleDetails)
            } else if x < 280.0 {
                Some(Message::RequestAudio(AudioDirection::Input))
            } else if x < 460.0 {
                Some(Message::RequestAudio(AudioDirection::Output))
            } else {
                None
            }
        } else {
            None
        };
        if click_lod.get()
            && let Some(message) = message
        {
            update(&mut click_app.borrow_mut(), message);
            let _ = draw(&click_canvas, &click_context, &click_app.borrow());
        }
    });
    canvas.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())?;
    click.forget();

    let (resize_canvas, resize_context, resize_app, resize_lod) = (
        canvas.clone(),
        context.clone(),
        Rc::clone(&app),
        Rc::clone(&lod),
    );
    let resize = Closure::<dyn FnMut()>::new(move || {
        let _ = resize(&resize_canvas);
        if resize_lod.get() {
            let _ = draw(&resize_canvas, &resize_context, &resize_app.borrow());
        }
    });
    window.add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref())?;
    resize.forget();

    let (key_canvas, key_context, key_app, key_lod) =
        (canvas, context, Rc::clone(&app), Rc::clone(&lod));
    let key = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if event.key() == "F9" {
            event.prevent_default();
            let next = !key_lod.get();
            key_lod.set(next);
            if next {
                let _ = draw(&key_canvas, &key_context, &key_app.borrow());
                set_status("LOW-DETAIL WASM MODE · F9 FOR EXACT", "ready");
            } else {
                set_status("EXACT NATIVE MIRROR · F9 TOGGLE LOD", "ready");
            }
            return;
        }
        if key_lod.get() {
            return;
        }
        let key_event = KeyEvent {
            key: Key::character(event.key()),
            text: Some(event.key()),
            repeat: event.repeat(),
            modifiers: Modifiers {
                shift: event.shift_key(),
                control: event.ctrl_key(),
                alt: event.alt_key(),
                meta: event.meta_key(),
            },
            at_ms: None,
        };
        let encoded = js_sys::encode_uri_component(&key_event.key.as_str());
        let url = format!(
            "http://127.0.0.1:8090/v1/key?key={encoded}&shift={}&control={}&alt={}&meta={}",
            key_event.modifiers.shift,
            key_event.modifiers.control,
            key_event.modifiers.alt,
            key_event.modifiers.meta,
        );
        let _ = web_sys::window().map(|window| window.fetch_with_str(&url));
        event.prevent_default();
    });
    window.add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref())?;
    key.forget();
    Ok(())
}


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
        Self {
            translation: [0.0; 3],
            scale: [1.0; 3],
        }
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
fn one() -> f32 {
    1.0
}
#[derive(Debug, Clone, Deserialize)]
struct Scene {
    nodes: Vec<SceneNode>,
}
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
fn one_bool() -> bool {
    true
}
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
    surface_id: String,
    title: String,
    scene: Scene,
    buttons: Vec<SceneButton>,
    theme: SceneTheme,
    viewport: [f64; 2],
}

fn start_surface_scene(
    canvas: &HtmlCanvasElement,
    context: &CanvasRenderingContext2d,
    surface: String,
) -> Result<(), JsValue> {
    let state: Rc<RefCell<Option<SurfaceScene>>> = Rc::new(RefCell::new(None));
    let hits: Rc<RefCell<Vec<SceneButton>>> = Rc::new(RefCell::new(Vec::new()));
    let hover: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let fetch_canvas = canvas.clone();
    let fetch_context = context.clone();
    let fetch_state = Rc::clone(&state);
    let fetch_hits = Rc::clone(&hits);
    let fetch_hover = Rc::clone(&hover);
    let fetch_surface = surface.clone();
    let tick = Closure::<dyn FnMut()>::new(move || {
        let url = format!(
            "http://127.0.0.1:8090/v1/scene?surface={}&t={}",
            js_sys::encode_uri_component(&fetch_surface),
            js_sys::Date::now()
        );
        let Ok(request) = web_sys::XmlHttpRequest::new() else { return };
        if request.open_with_async("GET", &url, false).is_err() {
            return;
        }
        if request.send().is_err() {
            return;
        }
        let Ok(Some(body)) = request.response_text() else { return };
        let Ok(scene) = serde_json::from_str::<SurfaceScene>(&body) else { return };
        *fetch_hits.borrow_mut() = scene.buttons.clone();
        draw_surface_scene(
            &fetch_canvas,
            &fetch_context,
            &scene,
            fetch_hover.borrow().as_deref(),
        );
        *fetch_state.borrow_mut() = Some(scene);
    });
    let function: &js_sys::Function = tick.as_ref().unchecked_ref();
    function.call0(&JsValue::NULL)?;
    web_sys::window()
        .ok_or("window unavailable")?
        .set_interval_with_callback_and_timeout_and_arguments_0(
            tick.as_ref().unchecked_ref(),
            250,
        )?;
    tick.forget();

    let move_canvas = canvas.clone();
    let move_context = context.clone();
    let move_hits = Rc::clone(&hits);
    let move_hover = Rc::clone(&hover);
    let move_state = Rc::clone(&state);
    let moved = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let hit = button_at(&move_canvas, &move_hits.borrow(), &event);
        let next = hit.map(|button| button.id);
        if *move_hover.borrow() == next {
            return;
        }
        let _ = move_canvas.style().set_property(
            "cursor",
            if next.is_some() { "pointer" } else { "default" },
        );
        *move_hover.borrow_mut() = next;
        if let Some(scene) = move_state.borrow().as_ref() {
            draw_surface_scene(&move_canvas, &move_context, scene, move_hover.borrow().as_deref());
        }
    });
    canvas.add_event_listener_with_callback("mousemove", moved.as_ref().unchecked_ref())?;
    moved.forget();

    let leave_canvas = canvas.clone();
    let leave_hover = Rc::clone(&hover);
    let leave_state = Rc::clone(&state);
    let leave_context = context.clone();
    let leave = Closure::<dyn FnMut(MouseEvent)>::new(move |_event: MouseEvent| {
        if leave_hover.borrow_mut().take().is_some() {
            let _ = leave_canvas.style().set_property("cursor", "default");
            if let Some(scene) = leave_state.borrow().as_ref() {
                draw_surface_scene(&leave_canvas, &leave_context, scene, None);
            }
        }
    });
    canvas.add_event_listener_with_callback("mouseleave", leave.as_ref().unchecked_ref())?;
    leave.forget();

    let click_canvas = canvas.clone();
    let click_hits = Rc::clone(&hits);
    let click_surface = surface;
    let click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if let Some(button) = button_at(&click_canvas, &click_hits.borrow(), &event) {
            let url = format!(
                "http://127.0.0.1:8090/v1/activate?surface={}&target={}",
                js_sys::encode_uri_component(&click_surface),
                js_sys::encode_uri_component(&button.id)
            );
            let _ = web_sys::window().map(|window| window.fetch_with_str(&url));
            event.prevent_default();
        }
    });
    canvas.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())?;
    click.forget();
    tick_surface_scene(canvas, context, &state, &hover);
    Ok(())
}

fn tick_surface_scene(
    canvas: &HtmlCanvasElement,
    context: &CanvasRenderingContext2d,
    state: &Rc<RefCell<Option<SurfaceScene>>>,
    hover: &Rc<RefCell<Option<String>>>,
) {
    if let Some(scene) = state.borrow().as_ref() {
        draw_surface_scene(canvas, context, scene, hover.borrow().as_deref());
    }
}

fn button_at(
    canvas: &HtmlCanvasElement,
    buttons: &[SceneButton],
    event: &MouseEvent,
) -> Option<SceneButton> {
    let rect = canvas.get_bounding_client_rect();
    let width = rect.width().max(1.0);
    let height = rect.height().max(1.0);
    let nx = ((f64::from(event.client_x()) - rect.left()) / width * 8.0 - 4.0) as f32;
    let ny = (4.0 - (f64::from(event.client_y()) - rect.top()) / height * 8.0) as f32;
    buttons
        .iter()
        .rev()
        .find(|button| {
            button.enabled
                && nx >= button.x
                && nx <= button.x + button.width
                && ny <= button.y
                && ny >= button.y - button.height
        })
        .cloned()
}

fn scene_ink<'a>(ink: Option<&str>, theme: &'a SceneTheme) -> [u8; 3] {
    match ink.unwrap_or("text") {
        "surface" => theme.surface,
        "background" => theme.background,
        "muted" => theme.muted,
        "primary" => theme.primary,
        "success" => theme.success,
        "warning" => theme.warning,
        "danger" => theme.danger,
        _ => theme.text,
    }
}

fn draw_surface_scene(
    canvas: &HtmlCanvasElement,
    context: &CanvasRenderingContext2d,
    surface: &SurfaceScene,
    hover: Option<&str>,
) {
    let width = f64::from(canvas.width());
    let height = f64::from(canvas.height());
    let theme = &surface.theme;
    context.set_fill_style_str(&hex(theme.background));
    context.fill_rect(0.0, 0.0, width, height);
    let mut nodes = surface.scene.nodes.iter().collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.transform.translation[2].total_cmp(&right.transform.translation[2])
    });
    for node in nodes {
        let t = node.transform;
        let x = (f64::from(t.translation[0]) + 4.0) / 8.0 * width;
        let y = (1.0 - (f64::from(t.translation[1]) + 4.0) / 8.0) * height;
        let z = 0.5 + ((t.translation[2].clamp(-4.0, 4.0) + 4.0) / 8.0) * 1.5;
        match &node.content {
            NodeContent::Rect { ink, opacity, .. } => {
                let rw = (f64::from(t.scale[0].abs()) * width / 8.0).max(1.0);
                let rh = (f64::from(t.scale[1].abs()) * height / 8.0).max(1.0);
                let color = scene_ink(ink.as_deref(), theme);
                context.set_fill_style_str(&format!(
                    "rgba({},{},{},{})",
                    color[0],
                    color[1],
                    color[2],
                    opacity.clamp(0.0, 1.0)
                ));
                context.fill_rect(x, y, rw, rh);
            }
            NodeContent::Text { text, ink } => {
                let size = (t.scale[1].abs() * theme.body * z).max(theme.micro);
                context.set_fill_style_str(&hex(scene_ink(ink.as_deref(), theme)));
                context.set_font(&format!("{size}px ui-monospace, monospace"));
                context.set_text_baseline("top");
                let _ = context.fill_text(text, x, y);
            }
            NodeContent::Group | NodeContent::Other => {}
        }
    }
    for button in &surface.buttons {
        let x = (f64::from(button.x) + 4.0) / 8.0 * width;
        let y = (1.0 - (f64::from(button.y) + 4.0) / 8.0) * height;
        let w = f64::from(button.width.abs()) * width / 8.0;
        let h = f64::from(button.height.abs()) * height / 8.0;
        let is_hovered = hover == Some(button.id.as_str());
        // `hit` controls are invisible local targets over retained content.
        // They earn a surface only while the pointer is on them.
        if button.appearance != "hit" && !button.label.is_empty() {
            context.set_fill_style_str(&rgba(theme.primary, 0.18));
            context.fill_rect(x, y, w, h);
        }
        if is_hovered {
            context.set_fill_style_str(&rgba(theme.primary, 0.20));
            context.fill_rect(x, y, w, h);
            context.set_stroke_style_str(&rgba(theme.primary, 0.75));
            context.set_line_width(1.0);
            context.stroke_rect(x + 0.5, y + 0.5, (w - 1.0).max(1.0), (h - 1.0).max(1.0));
        }
    }
    set_status(&format!("{} · {} NODES", surface.title.to_uppercase(), surface.scene.nodes.len()), "ready");
    let label = format!("{} · LIVE SURFACE", surface.title.to_uppercase());
    let _ = &surface.surface_id;
    label_text(context, &hex(theme.muted), 10.0, width - 8.0, height - 8.0, &label);
}

fn label_text(
    context: &CanvasRenderingContext2d,
    color: &str,
    size: f64,
    x: f64,
    y: f64,
    value: &str,
) {
    context.set_fill_style_str(color);
    context.set_font(&format!("{size}px ui-monospace, monospace"));
    context.set_text_baseline("bottom");
    context.set_text_align("right");
    let _ = context.fill_text(value, x, y);
    context.set_text_align("start");
}

fn resize(canvas: &HtmlCanvasElement) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("window unavailable")?;
    let scale = window.device_pixel_ratio().clamp(1.0, 2.0);
    canvas.set_width((window.inner_width()?.as_f64().unwrap_or(1280.0) * scale) as u32);
    canvas.set_height((window.inner_height()?.as_f64().unwrap_or(720.0) * scale) as u32);
    Ok(())
}

fn start_native_mirror(
    canvas: &HtmlCanvasElement,
    context: &CanvasRenderingContext2d,
    lod: Rc<Cell<bool>>,
    surface: Option<String>,
) -> Result<(), JsValue> {
    let image = HtmlImageElement::new()?;
    let draw_canvas = canvas.clone();
    let draw_context = context.clone();
    let draw_lod = Rc::clone(&lod);
    let draw_image = image.clone();
    let fit_surface = surface.clone();
    let loaded = Closure::<dyn FnMut()>::new(move || {
        if draw_lod.get() {
            return;
        }
        let width = f64::from(draw_canvas.width());
        let height = f64::from(draw_canvas.height());
        let source_width = f64::from(draw_image.natural_width().max(1));
        let source_height = f64::from(draw_image.natural_height().max(1));
        // A named surface is a window: preserve its aspect ratio and letterbox
        // it. The whole-canvas mirror keeps cover-filling the wall.
        let fit = if fit_surface.is_some() { f64::min } else { f64::max };
        let scale = fit(width / source_width, height / source_height);
        let drawn_width = source_width * scale;
        let drawn_height = source_height * scale;
        let x = (width - drawn_width) * 0.5;
        let y = (height - drawn_height) * 0.5;
        draw_context.clear_rect(0.0, 0.0, width, height);
        if draw_context
            .draw_image_with_html_image_element_and_dw_and_dh(
                &draw_image,
                x,
                y,
                drawn_width,
                drawn_height,
            )
            .is_ok()
        {
            set_status("EXACT NATIVE MIRROR · L TOGGLE LOD", "ready");
        }
    });
    image.set_onload(Some(loaded.as_ref().unchecked_ref()));
    loaded.forget();

    let failed_lod = Rc::clone(&lod);
    let failed = Closure::<dyn FnMut()>::new(move || {
        if !failed_lod.get() {
            set_status("WAITING FOR LOCAL CANVAS · START canvas-web-host", "error");
        }
    });
    image.set_onerror(Some(failed.as_ref().unchecked_ref()));
    failed.forget();

    let next_image = image.clone();
    let next_surface = surface.clone();
    let tick = Closure::<dyn FnMut()>::new(move || {
        let surface = next_surface
            .as_deref()
            .map(|surface| format!("&surface={}", js_sys::encode_uri_component(surface)))
            .unwrap_or_default();
        next_image.set_src(&format!(
            "http://127.0.0.1:8090/v1/frame?t={}{surface}",
            js_sys::Date::now()
        ));
    });
    web_sys::window()
        .ok_or("window unavailable")?
        .set_interval_with_callback_and_timeout_and_arguments_0(
            tick.as_ref().unchecked_ref(),
            125,
        )?;
    tick.forget();
    let initial = surface
        .as_deref()
        .map(|surface| format!("&surface={}", js_sys::encode_uri_component(surface)))
        .unwrap_or_default();
    image.set_src(&format!("http://127.0.0.1:8090/v1/frame?t=0{initial}"));
    install_native_pointer(canvas, &image, lod)?;
    Ok(())
}

fn install_native_pointer(
    canvas: &HtmlCanvasElement,
    image: &HtmlImageElement,
    lod: Rc<Cell<bool>>,
) -> Result<(), JsValue> {
    let dragging = Rc::new(Cell::new(false));

    let down_canvas = canvas.clone();
    let down_image = image.clone();
    let down_dragging = Rc::clone(&dragging);
    let down_lod = Rc::clone(&lod);
    let down = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        if down_lod.get() || event.button() != 0 {
            return;
        }
        if let Some((x, y)) = normalized_pointer(&down_canvas, &down_image, &event) {
            down_dragging.set(true);
            send_pointer(PointerPhase::Press, x, y, 0.0);
            event.prevent_default();
        }
    });
    canvas.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let move_canvas = canvas.clone();
    let move_image = image.clone();
    let move_dragging = Rc::clone(&dragging);
    let moved = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        if !move_dragging.get() {
            return;
        }
        if let Some((x, y)) = normalized_pointer(&move_canvas, &move_image, &event) {
            send_pointer(PointerPhase::Move, x, y, 0.0);
            event.prevent_default();
        }
    });
    web_sys::window()
        .ok_or("window unavailable")?
        .add_event_listener_with_callback("pointermove", moved.as_ref().unchecked_ref())?;
    moved.forget();

    let up_canvas = canvas.clone();
    let up_image = image.clone();
    let up_dragging = Rc::clone(&dragging);
    let up = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
        if !up_dragging.replace(false) {
            return;
        }
        if let Some((x, y)) = normalized_pointer(&up_canvas, &up_image, &event) {
            send_pointer(PointerPhase::Release, x, y, 0.0);
            event.prevent_default();
        }
    });
    web_sys::window()
        .ok_or("window unavailable")?
        .add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    up.forget();

    let wheel_canvas = canvas.clone();
    let wheel_image = image.clone();
    let wheel_lod = lod;
    let wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |event: WheelEvent| {
        if wheel_lod.get() {
            return;
        }
        if let Some((x, y)) = normalized_client(
            &wheel_canvas,
            &wheel_image,
            f64::from(event.client_x()),
            f64::from(event.client_y()),
        ) {
            send_scroll(x, y, (-event.delta_y() / 50.0) as f32);
            event.prevent_default();
        }
    });
    canvas.add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())?;
    wheel.forget();
    Ok(())
}

fn normalized_pointer(
    canvas: &HtmlCanvasElement,
    image: &HtmlImageElement,
    event: &PointerEvent,
) -> Option<(f32, f32)> {
    normalized_client(
        canvas,
        image,
        f64::from(event.client_x()),
        f64::from(event.client_y()),
    )
}

fn normalized_client(
    canvas: &HtmlCanvasElement,
    image: &HtmlImageElement,
    client_x: f64,
    client_y: f64,
) -> Option<(f32, f32)> {
    let width = f64::from(canvas.client_width().max(1));
    let height = f64::from(canvas.client_height().max(1));
    let source_width = f64::from(image.natural_width());
    let source_height = f64::from(image.natural_height());
    if source_width <= 0.0 || source_height <= 0.0 {
        return None;
    }
    let scale = (width / source_width).max(height / source_height);
    let x = (client_x - (width - source_width * scale) * 0.5) / scale / source_width;
    let y = (client_y - (height - source_height * scale) * 0.5) / scale / source_height;
    Some((x.clamp(0.0, 1.0) as f32, y.clamp(0.0, 1.0) as f32))
}

fn send_pointer(phase: PointerPhase, x: f32, y: f32, delta: f32) {
    let event = PointerEvent {
        phase,
        x,
        y,
        pointer_id: 1,
        button: Some(soil_input::PointerButton::Primary),
        modifiers: Modifiers::default(),
        at_ms: None,
    };
    let url = format!(
        "http://127.0.0.1:8090/v1/pointer?phase={}&x={}&y={}&delta={delta}",
        event.phase.as_str(),
        event.x,
        event.y,
    );
    let _ = web_sys::window().map(|window| window.fetch_with_str(&url));
}

fn send_scroll(x: f32, y: f32, delta: f32) {
    let event = ScrollEvent {
        x,
        y,
        dx: 0.0,
        dy: -delta,
        modifiers: Modifiers::default(),
        at_ms: None,
    };
    let url = format!(
        "http://127.0.0.1:8090/v1/pointer?phase=zoom&x={}&y={}&delta={}",
        event.x,
        event.y,
        -event.dy,
    );
    let _ = web_sys::window().map(|window| window.fetch_with_str(&url));
}

fn draw(
    canvas: &HtmlCanvasElement,
    context: &CanvasRenderingContext2d,
    app: &App,
) -> Result<(), JsValue> {
    let theme = app.theme();
    let workspace: Workspace = serde_json::from_str(TV_WORKSPACE).map_err(|e| e.to_string())?;
    let p = theme.palette;
    let (width, height) = (f64::from(canvas.width()), f64::from(canvas.height()));
    context.set_fill_style_str(&hex(p.background));
    context.fill_rect(0.0, 0.0, width, height);
    for i in 0..6 {
        context.begin_path();
        context.set_stroke_style_str(&rgba(p.primary, 0.035 + i as f64 * 0.008));
        context.set_line_width(16.0);
        context.ellipse(
            width * 0.5,
            height * 0.5,
            (width * 0.58 - i as f64 * 70.0).max(30.0),
            (height * 0.45 - i as f64 * 32.0).max(20.0),
            -0.2,
            0.0,
            std::f64::consts::TAU,
        )?;
        context.stroke();
    }
    label(context, &hex(p.text), 18.0, 20.0, 30.0, "CANVAS")?;
    label(
        context,
        &hex(p.text_muted),
        11.0,
        100.0,
        29.0,
        &format!(
            "{} / {} · LOOPBACK",
            workspace.project_id.to_uppercase(),
            theme.name
        ),
    )?;
    button(
        context,
        app.active == ThemeId::Jarvis,
        &theme,
        width - 386.0,
        7.0,
        112.0,
        "JARVIS",
    )?;
    button(
        context,
        app.active == ThemeId::PipBoy,
        &theme,
        width - 264.0,
        7.0,
        112.0,
        "PIP-BOY",
    )?;
    button(
        context,
        app.active == ThemeId::Imperial,
        &theme,
        width - 142.0,
        7.0,
        120.0,
        "IMPERIAL",
    )?;

    let top = 46.0;
    let available = (height - top - 48.0).max(240.0);
    for placement in &workspace.layout.placements {
        if let Some(surface) = workspace
            .surfaces
            .iter()
            .find(|s| s.id == placement.surface_id)
        {
            let b = &placement.bounds;
            surface_panel(
                context,
                &theme,
                surface,
                b.x * width,
                top + b.y * available,
                b.width * width,
                b.height * available,
                app.details,
            )?;
        }
    }
    let footer = height - 42.0;
    button(
        context,
        false,
        &theme,
        14.0,
        footer,
        125.0,
        if app.details {
            "COMPACT LOD"
        } else {
            "FULL LOD"
        },
    )?;
    button(context, false, &theme, 150.0, footer, 120.0, "JARVIS MIC")?;
    button(
        context,
        false,
        &theme,
        281.0,
        footer,
        170.0,
        "JARVIS AUDIO OUT",
    )?;
    label(
        context,
        &hex(p.text_muted),
        11.0,
        (width - 210.0).max(470.0),
        footer + 23.0,
        if app.audio_intent.is_some() {
            "AUDIO INTENT READY"
        } else {
            "VIEW ONLY · LOCAL"
        },
    )?;
    Ok(())
}

fn surface_panel(
    context: &CanvasRenderingContext2d,
    theme: &ThemeProjection,
    surface: &Surface,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    details: bool,
) -> Result<(), JsValue> {
    let p = theme.palette;
    context.set_fill_style_str(&rgba(p.surface, 0.94));
    context.fill_rect(x, y, width, height);
    context.set_stroke_style_str(&hex(p.border));
    context.stroke_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0);
    context.set_fill_style_str(&rgba(p.background_strong, 0.94));
    context.fill_rect(x + 1.0, y + 1.0, width - 2.0, 27.0);
    context.set_fill_style_str(&hex(p.primary));
    context.fill_rect(x + 1.0, y + 1.0, 3.0, 27.0);
    label(
        context,
        &hex(p.text),
        11.0,
        x + 11.0,
        y + 18.0,
        &surface.title.to_uppercase(),
    )?;
    let kind = surface
        .content
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unsupported");
    if kind == "live_video" {
        let source = surface
            .content
            .get("source_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("stream");
        context.set_fill_style_str(&rgba(p.background, 0.78));
        context.fill_rect(
            x + 9.0,
            y + 36.0,
            (width - 18.0).max(1.0),
            (height - 45.0).max(1.0),
        );
        label(
            context,
            &hex(p.primary),
            12.0,
            x + 18.0,
            y + 61.0,
            "LIVE SURFACE",
        )?;
        label(
            context,
            &hex(p.text_muted),
            10.0,
            x + 18.0,
            y + 80.0,
            &format!("{source} · PRIVATE BYTES NOT CONNECTED"),
        )?;
        if surface
            .content
            .get("orbitable")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            label(
                context,
                &hex(p.text_muted),
                9.0,
                x + 18.0,
                y + height - 17.0,
                "ORBIT ◉   VIEWS 1—8   TIMELINE",
            )?;
        }
    } else {
        let widget = surface
            .content
            .pointer("/spec/kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("widget");
        label(
            context,
            &hex(p.primary),
            11.0,
            x + 14.0,
            y + 51.0,
            &widget.replace('_', " ").to_uppercase(),
        )?;
        if details {
            for row in 0..3 {
                context.set_fill_style_str(&rgba(p.border_strong, 0.36));
                context.fill_rect(
                    x + 14.0,
                    y + 67.0 + row as f64 * 15.0,
                    (width - 28.0) * (0.86 - row as f64 * 0.14),
                    3.0,
                );
            }
        }
    }
    Ok(())
}

fn button(
    context: &CanvasRenderingContext2d,
    selected: bool,
    theme: &ThemeProjection,
    x: f64,
    y: f64,
    width: f64,
    text: &str,
) -> Result<(), JsValue> {
    let p = theme.palette;
    context.set_fill_style_str(&hex(if selected { p.primary } else { p.surface_muted }));
    context.fill_rect(x, y, width, 34.0);
    context.set_stroke_style_str(&hex(if selected { p.primary } else { p.border }));
    context.stroke_rect(x + 0.5, y + 0.5, width - 1.0, 33.0);
    label(
        context,
        &hex(if selected { p.background } else { p.text }),
        11.0,
        x + 11.0,
        y + 22.0,
        text,
    )
}

fn label(
    context: &CanvasRenderingContext2d,
    color: &str,
    size: f64,
    x: f64,
    y: f64,
    value: &str,
) -> Result<(), JsValue> {
    context.set_fill_style_str(color);
    context.set_font(&format!("{size}px ui-monospace, monospace"));
    context.fill_text(value, x, y).map(|_| ())
}
fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}
fn rgba(rgb: [u8; 3], alpha: f64) -> String {
    format!("rgba({},{},{},{alpha})", rgb[0], rgb[1], rgb[2])
}
fn set_status(message: &str, state: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(status) = document.get_element_by_id("boot-status") else {
        return;
    };
    let _ = status.set_attribute("data-state", state);
    if let Ok(Some(label)) = status.query_selector("span") {
        label.set_text_content(Some(message));
    }
}
