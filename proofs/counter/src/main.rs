//! The smallest thing that proves the renderer boundary: one counter, two
//! buttons, the same code in a browser tab and in a native window.
//!
//! This is deliberately *not* a Canvas Spore yet. A Spore hands the host a scene
//! document; this app owns its own iced widget tree. The point is to establish
//! that iced — the toolkit Canvas already renders with — survives the trip to
//! wasm32 before we decide how a Spore's scene reaches that same path.

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length, Task, Theme};

fn main() -> iced::Result {
    native()
}

#[derive(Default)]
struct Counter {
    count: i64,
}

#[derive(Debug, Clone, Copy)]
enum Message {
    Up,
    Down,
}

fn update(state: &mut Counter, message: Message) -> Task<Message> {
    match message {
        Message::Up => state.count += 1,
        Message::Down => state.count -= 1,
    }
    Task::none()
}

fn view(state: &Counter) -> Element<'_, Message> {
    let value = text(state.count.to_string()).size(72);
    let controls = row![
        button(text("−").size(26))
            .on_press(Message::Down)
            .padding(14),
        button(text("+").size(26))
            .on_press(Message::Up)
            .padding(14),
    ]
    .spacing(16);

    container(
        column![value, controls]
            .spacing(28)
            .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

#[cfg(not(target_arch = "wasm32"))]
fn native() -> iced::Result {
    iced::application(Counter::default, update, view)
        .title("Counter · iced")
        .theme(|_: &Counter| Theme::TokyoNight)
        .antialiasing(true)
        .window_size((360.0, 260.0))
        .run()
}

#[cfg(target_arch = "wasm32")]
fn native() -> iced::Result {
    unreachable!("the browser enters through `start`")
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    console_error_panic_hook::set_once();
    iced::application(Counter::default, update, view)
        .title("Counter · iced")
        .theme(|_: &Counter| Theme::TokyoNight)
        .run()
        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))
}
