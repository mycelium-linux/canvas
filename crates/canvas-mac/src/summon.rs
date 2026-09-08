//! Global Ctrl+Option+Space project summon.

use canvas_core::global_hotkey::{GlobalHotkey, HotkeySpec};

pub struct SummonHotkey(GlobalHotkey);

impl SummonHotkey {
    pub fn register() -> Result<Self, String> {
        GlobalHotkey::register(HotkeySpec {
            signature: *b"Dock",
            id: 1,
            key_code: 0x31,                   // kVK_Space
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: "Ctrl+Option+Space",
        })
        .map(Self)
    }

    pub fn poll_pressed(&self) -> bool {
        self.0.poll_pressed()
    }
}
