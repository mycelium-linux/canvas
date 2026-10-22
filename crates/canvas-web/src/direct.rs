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
    resize(&canvas)?;
    start_native_mirror(&canvas, &context, Rc::clone(&lod))?;
    set_status("EXACT NATIVE MIRROR · L TOGGLE LOD", "ready");

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
        let encoded = js_sys::encode_uri_component(&event.key());
        let url = format!(
            "http://127.0.0.1:8090/v1/key?key={encoded}&shift={}&control={}&alt={}&meta={}",
            event.shift_key(),
            event.ctrl_key(),
            event.alt_key(),
            event.meta_key(),
        );
        let _ = web_sys::window().map(|window| window.fetch_with_str(&url));
        event.prevent_default();
    });
    window.add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref())?;
    key.forget();
    Ok(())
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
) -> Result<(), JsValue> {
    let image = HtmlImageElement::new()?;
    let draw_canvas = canvas.clone();
    let draw_context = context.clone();
    let draw_lod = Rc::clone(&lod);
    let draw_image = image.clone();
    let loaded = Closure::<dyn FnMut()>::new(move || {
        if draw_lod.get() {
            return;
        }
        let width = f64::from(draw_canvas.width());
        let height = f64::from(draw_canvas.height());
        let source_width = f64::from(draw_image.natural_width().max(1));
        let source_height = f64::from(draw_image.natural_height().max(1));
        // Fill the viewport like the native wall. A differently-shaped browser
        // trims symmetric outer edges instead of surrounding Canvas with bars.
        let scale = (width / source_width).max(height / source_height);
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
    let tick = Closure::<dyn FnMut()>::new(move || {
        next_image.set_src(&format!(
            "http://127.0.0.1:8090/v1/frame?t={}",
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
    image.set_src("http://127.0.0.1:8090/v1/frame?t=0");
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
            send_pointer("press", x, y, 0.0);
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
            send_pointer("drag", x, y, 0.0);
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
            send_pointer("release", x, y, 0.0);
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
            send_pointer("zoom", x, y, (-event.delta_y() / 50.0) as f32);
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

fn send_pointer(phase: &str, x: f32, y: f32, delta: f32) {
    let url = format!("http://127.0.0.1:8090/v1/pointer?phase={phase}&x={x}&y={y}&delta={delta}");
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
