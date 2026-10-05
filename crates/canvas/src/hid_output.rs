//! Focus-scoped desktop HID output.
//!
//! Canvas owns focus and geometry. This module only converts physical input to
//! the bounded local report accepted by `unibus-hid-socket`; it never connects
//! to a remote guest or knows QMP.

use iced::keyboard::{
    Event as KeyEvent,
    key::{Code, Physical},
};
use serde::Serialize;
use std::{collections::BTreeSet, os::unix::net::UnixDatagram, path::PathBuf};

#[derive(Debug, Default)]
pub(crate) struct HidOutput {
    keys: BTreeSet<u16>,
    pointer_x: u16,
    pointer_y: u16,
    pointer_buttons: u8,
    sequence: u64,
    target: Option<String>,
}

#[derive(Serialize)]
struct LocalReport<'a> {
    target_id: &'a str,
    sequence: u64,
    keys: Vec<u16>,
    pointer_x: u16,
    pointer_y: u16,
    pointer_buttons: u8,
    scroll_x: i16,
    scroll_y: i16,
}

impl HidOutput {
    pub(crate) fn key(&mut self, target_id: &str, event: &KeyEvent) -> Result<bool, String> {
        let (physical_key, down) = match event {
            KeyEvent::KeyPressed { physical_key, .. } => (physical_key, true),
            KeyEvent::KeyReleased { physical_key, .. } => (physical_key, false),
            KeyEvent::ModifiersChanged(_) => return Ok(false),
        };
        let Physical::Code(code) = physical_key else {
            return Ok(false);
        };
        let Some(usage) = code_to_hid(*code) else {
            return Ok(false);
        };
        self.select_target(target_id)?;
        if down {
            self.keys.insert(usage);
        } else {
            self.keys.remove(&usage);
        }
        self.send(target_id, 0, 0)?;
        Ok(true)
    }

    pub(crate) fn pointer(
        &mut self,
        target_id: &str,
        x: f64,
        y: f64,
        event: &iced::mouse::Event,
    ) -> Result<(), String> {
        self.select_target(target_id)?;
        let next_x = (x.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        let next_y = (y.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        if matches!(event, iced::mouse::Event::CursorMoved { .. })
            && next_x == self.pointer_x
            && next_y == self.pointer_y
        {
            return Ok(());
        }
        self.pointer_x = next_x;
        self.pointer_y = next_y;
        let mut scroll_x = 0;
        let mut scroll_y = 0;
        match event {
            iced::mouse::Event::ButtonPressed(button) => self.set_button(*button, true),
            iced::mouse::Event::ButtonReleased(button) => self.set_button(*button, false),
            iced::mouse::Event::WheelScrolled { delta } => {
                let (x, y) = match delta {
                    iced::mouse::ScrollDelta::Lines { x, y } => (*x * 120.0, *y * 120.0),
                    iced::mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                scroll_x = x.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
                scroll_y = y.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            }
            _ => {}
        }
        self.send(target_id, scroll_x, scroll_y)
    }

    fn set_button(&mut self, button: iced::mouse::Button, down: bool) {
        let mask = match button {
            iced::mouse::Button::Left => 1,
            iced::mouse::Button::Right => 2,
            iced::mouse::Button::Middle => 4,
            iced::mouse::Button::Back => 8,
            iced::mouse::Button::Forward => 16,
            iced::mouse::Button::Other(_) => return,
        };
        if down {
            self.pointer_buttons |= mask;
        } else {
            self.pointer_buttons &= !mask;
        }
    }

    pub(crate) fn release(&mut self, target_id: &str) -> Result<(), String> {
        self.keys.clear();
        self.pointer_buttons = 0;
        self.send(target_id, 0, 0)
    }

    pub(crate) fn release_if_active(&mut self, target_id: &str) -> Result<(), String> {
        if self.target.as_deref() == Some(target_id) {
            self.release(target_id)?;
        }
        Ok(())
    }

    fn select_target(&mut self, target_id: &str) -> Result<(), String> {
        if self.target.as_deref() != Some(target_id) {
            if let Some(previous) = self.target.clone() {
                self.release(&previous)?;
            }
            self.target = Some(target_id.to_owned());
        }
        Ok(())
    }

    fn send(&mut self, target_id: &str, scroll_x: i16, scroll_y: i16) -> Result<(), String> {
        let path = socket_path().ok_or_else(|| {
            "CANVAS_HID_SOCKET is not configured for this input surface".to_owned()
        })?;
        self.sequence = self.sequence.wrapping_add(1);
        let report = LocalReport {
            target_id,
            sequence: self.sequence,
            keys: self.keys.iter().copied().collect(),
            pointer_x: self.pointer_x,
            pointer_y: self.pointer_y,
            pointer_buttons: self.pointer_buttons,
            scroll_x,
            scroll_y,
        };
        let bytes = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
        UnixDatagram::unbound()
            .map_err(|error| error.to_string())?
            .send_to(&bytes, path)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn socket_path() -> Option<PathBuf> {
    std::env::var_os("CANVAS_HID_SOCKET").map(PathBuf::from)
}

fn code_to_hid(code: Code) -> Option<u16> {
    Some(match code {
        Code::KeyA => 0x04,
        Code::KeyB => 0x05,
        Code::KeyC => 0x06,
        Code::KeyD => 0x07,
        Code::KeyE => 0x08,
        Code::KeyF => 0x09,
        Code::KeyG => 0x0a,
        Code::KeyH => 0x0b,
        Code::KeyI => 0x0c,
        Code::KeyJ => 0x0d,
        Code::KeyK => 0x0e,
        Code::KeyL => 0x0f,
        Code::KeyM => 0x10,
        Code::KeyN => 0x11,
        Code::KeyO => 0x12,
        Code::KeyP => 0x13,
        Code::KeyQ => 0x14,
        Code::KeyR => 0x15,
        Code::KeyS => 0x16,
        Code::KeyT => 0x17,
        Code::KeyU => 0x18,
        Code::KeyV => 0x19,
        Code::KeyW => 0x1a,
        Code::KeyX => 0x1b,
        Code::KeyY => 0x1c,
        Code::KeyZ => 0x1d,
        Code::Digit1 => 0x1e,
        Code::Digit2 => 0x1f,
        Code::Digit3 => 0x20,
        Code::Digit4 => 0x21,
        Code::Digit5 => 0x22,
        Code::Digit6 => 0x23,
        Code::Digit7 => 0x24,
        Code::Digit8 => 0x25,
        Code::Digit9 => 0x26,
        Code::Digit0 => 0x27,
        Code::Enter => 0x28,
        Code::Escape => 0x29,
        Code::Backspace => 0x2a,
        Code::Tab => 0x2b,
        Code::Space => 0x2c,
        Code::Minus => 0x2d,
        Code::Equal => 0x2e,
        Code::BracketLeft => 0x2f,
        Code::BracketRight => 0x30,
        Code::Backslash => 0x31,
        Code::Semicolon => 0x33,
        Code::Quote => 0x34,
        Code::Backquote => 0x35,
        Code::Comma => 0x36,
        Code::Period => 0x37,
        Code::Slash => 0x38,
        Code::CapsLock => 0x39,
        Code::F1 => 0x3a,
        Code::F2 => 0x3b,
        Code::F3 => 0x3c,
        Code::F4 => 0x3d,
        Code::F5 => 0x3e,
        Code::F6 => 0x3f,
        Code::F7 => 0x40,
        Code::F8 => 0x41,
        Code::F9 => 0x42,
        Code::F10 => 0x43,
        Code::F11 => 0x44,
        Code::F12 => 0x45,
        Code::Insert => 0x49,
        Code::Home => 0x4a,
        Code::PageUp => 0x4b,
        Code::Delete => 0x4c,
        Code::End => 0x4d,
        Code::PageDown => 0x4e,
        Code::ArrowRight => 0x4f,
        Code::ArrowLeft => 0x50,
        Code::ArrowDown => 0x51,
        Code::ArrowUp => 0x52,
        Code::ControlLeft => 0xe0,
        Code::ShiftLeft => 0xe1,
        Code::AltLeft => 0xe2,
        Code::SuperLeft => 0xe3,
        Code::ControlRight => 0xe4,
        Code::ShiftRight => 0xe5,
        Code::AltRight => 0xe6,
        Code::SuperRight => 0xe7,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_previous_target_preserves_new_targets_held_state() {
        let mut output = HidOutput {
            target: Some("new-guest".into()),
            keys: BTreeSet::from([0x04]),
            pointer_buttons: 1,
            ..Default::default()
        };
        output.release_if_active("old-guest").unwrap();
        assert_eq!(output.keys, BTreeSet::from([0x04]));
        assert_eq!(output.pointer_buttons, 1);
    }

    #[test]
    fn physical_keys_use_usb_hid_usage_ids() {
        assert_eq!(code_to_hid(Code::KeyA), Some(0x04));
        assert_eq!(code_to_hid(Code::Enter), Some(0x28));
        assert_eq!(code_to_hid(Code::ControlLeft), Some(0xe0));
    }
}
