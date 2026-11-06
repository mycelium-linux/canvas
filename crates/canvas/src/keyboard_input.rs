use std::time::Duration;

pub(crate) const FOCUS_EMPHASIS_DURATION: Duration = Duration::from_millis(1_400);

pub(crate) fn key_to_bytes(event: &iced::keyboard::Event) -> Vec<u8> {
    use iced::keyboard::Event;
    let Event::KeyPressed {
        text,
        key,
        modifiers,
        ..
    } = event
    else {
        return Vec::new();
    };

    // Control chords first. They are not text: iced reports `text: None` for
    // Ctrl-L, so a mapper that only looks at text silently drops every one of
    // them — which is why clear-screen and Ctrl-C did nothing.
    if modifiers.control() {
        if let iced::keyboard::Key::Character(character) = key {
            if let Some(byte) = control_byte(character.as_str()) {
                return vec![byte];
            }
        }
    }

    let mut bytes = if let Some(text) = text
        && !text.is_empty()
    {
        text.as_bytes().to_vec()
    } else {
        use iced::keyboard::key::Named;
        match key {
            iced::keyboard::Key::Named(Named::Enter) => b"\r".to_vec(),
            iced::keyboard::Key::Named(Named::Backspace) => b"\x7f".to_vec(),
            iced::keyboard::Key::Named(Named::Tab) => b"\t".to_vec(),
            iced::keyboard::Key::Named(Named::ArrowUp) => b"\x1b[A".to_vec(),
            iced::keyboard::Key::Named(Named::ArrowDown) => b"\x1b[B".to_vec(),
            iced::keyboard::Key::Named(Named::ArrowRight) => b"\x1b[C".to_vec(),
            iced::keyboard::Key::Named(Named::ArrowLeft) => b"\x1b[D".to_vec(),
            iced::keyboard::Key::Named(Named::Home) => b"\x1b[H".to_vec(),
            iced::keyboard::Key::Named(Named::End) => b"\x1b[F".to_vec(),
            iced::keyboard::Key::Named(Named::PageUp) => b"\x1b[5~".to_vec(),
            iced::keyboard::Key::Named(Named::PageDown) => b"\x1b[6~".to_vec(),
            iced::keyboard::Key::Named(Named::Escape) => b"\x1b".to_vec(),
            iced::keyboard::Key::Named(Named::Delete) => b"\x1b[3~".to_vec(),
            _ => Vec::new(),
        }
    };
    // Alt is the escape prefix; without this every meta-binding is dead.
    if modifiers.alt() && !bytes.is_empty() && !bytes.starts_with(b"\x1b") {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Ctrl-A..Ctrl-Z are 1..26, and the punctuation chords follow the same table
/// the terminal has used since the 1970s.
fn control_byte(character: &str) -> Option<u8> {
    let character = character.chars().next()?;
    let byte = match character.to_ascii_lowercase() {
        'a'..='z' => (character.to_ascii_lowercase() as u8) - b'a' + 1,
        '@' => 0,
        '[' => 27,
        '\\' => 28,
        ']' => 29,
        '^' => 30,
        '_' | '?' => 31,
        _ => return None,
    };
    Some(byte)
}

pub(crate) fn browser_key_event(
    value: &str,
    shift: bool,
    control: bool,
    alt: bool,
    meta: bool,
) -> Option<iced::keyboard::Event> {
    use iced::keyboard::{Event, Key, Location, Modifiers, key};

    let named = match value {
        "Enter" => Some(key::Named::Enter),
        "Escape" => Some(key::Named::Escape),
        "Backspace" => Some(key::Named::Backspace),
        "Tab" => Some(key::Named::Tab),
        "Delete" => Some(key::Named::Delete),
        "ArrowUp" => Some(key::Named::ArrowUp),
        "ArrowDown" => Some(key::Named::ArrowDown),
        "ArrowLeft" => Some(key::Named::ArrowLeft),
        "ArrowRight" => Some(key::Named::ArrowRight),
        "F2" => Some(key::Named::F2),
        "F3" => Some(key::Named::F3),
        "F4" => Some(key::Named::F4),
        " " => Some(key::Named::Space),
        _ => None,
    };
    let key = if let Some(named) = named {
        Key::Named(named)
    } else if value.chars().count() == 1 {
        Key::Character(value.into())
    } else {
        return None;
    };
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, shift);
    modifiers.set(Modifiers::CTRL, control);
    modifiers.set(Modifiers::ALT, alt);
    modifiers.set(Modifiers::LOGO, meta);
    let text =
        matches!(key, Key::Character(_) | Key::Named(key::Named::Space)).then(|| value.into());
    Some(Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: Location::Standard,
        modifiers,
        text,
        repeat: false,
    })
}

pub(crate) fn focus_emphasis_curve(elapsed: Duration) -> f32 {
    let progress = (elapsed.as_secs_f32() / FOCUS_EMPHASIS_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    if progress >= 1.0 {
        return 0.0;
    }
    let pulse = (progress * core::f32::consts::TAU * 2.0).sin() * 0.5 + 0.5;
    (1.0 - progress) * (0.45 + pulse * 0.55)
}

/// Ctrl+Left / Ctrl+Right steps pages within the current workspace.
///
/// Horizontal to the workspace cycle's vertical (Ctrl+Shift+Up/Down): moving
/// sideways stays inside the board, moving up and down leaves it. Shift is
/// excluded so the two cannot be pressed as the same gesture.
pub(crate) fn page_cycle_direction(event: &iced::keyboard::Event) -> Option<i32> {
    use iced::keyboard::{Event, Key, key::Named};
    let Event::KeyPressed {
        key,
        modifiers,
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    if !modifiers.control() || modifiers.shift() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(Named::ArrowLeft) => Some(-1),
        Key::Named(Named::ArrowRight) => Some(1),
        _ => None,
    }
}

pub(crate) fn workspace_cycle_direction(event: &iced::keyboard::Event) -> Option<i32> {
    use iced::keyboard::{Event, Key, key::Named};
    let Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    if !modifiers.control() || !modifiers.shift() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(Named::ArrowUp) => Some(-1),
        Key::Named(Named::ArrowDown) => Some(1),
        _ => None,
    }
}

pub(crate) fn display_shortcut(event: &iced::keyboard::Event) -> Option<usize> {
    use iced::keyboard::{Event, Key};
    let Event::KeyPressed {
        key,
        modifiers,
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    if !modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        Key::Character("0") => Some(0),
        Key::Character("1") => Some(1),
        Key::Character("2") => Some(2),
        Key::Character("3") => Some(3),
        _ => None,
    }
}

/// Ctrl+Alt+Space opens the application launcher.
///
/// The dock has a button for this, and the top bar opens the workspace switcher,
/// but both are chrome: they can be closed, reloaded into a broken state, or fail
/// to draw at all. A shell needs one way back that does not go through a surface --
/// otherwise losing the launcher means losing the ability to start anything, which
/// is not a recoverable state for a compositor to leave a person in.
pub(crate) fn launcher_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(iced::keyboard::key::Named::Space),
            modifiers,
            repeat: false,
            ..
        } if modifiers.control() && modifiers.alt() && !modifiers.logo()
    )
}

/// Ctrl+Alt+C opens the control center.
pub(crate) fn control_center_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Character(character),
            modifiers,
            repeat: false,
            ..
        } if character.as_str().eq_ignore_ascii_case("c")
            && modifiers.control()
            && modifiers.alt()
            && !modifiers.logo()
    )
}

pub(crate) fn snap_layout_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::Event;
    matches!(
        event,
        Event::KeyPressed {
            key,
            modified_key,
            modifiers,
            repeat: false,
            ..
        } if (matches_character(key, "l") || matches_character(modified_key, "l"))
            && modifiers.control()
            && modifiers.alt()
            && !modifiers.logo()
    )
}

pub(crate) fn matches_character(key: &iced::keyboard::Key, expected: &str) -> bool {
    matches!(key, iced::keyboard::Key::Character(value) if value.eq_ignore_ascii_case(expected))
}

pub(crate) fn widget_catalog_page_direction(event: &iced::keyboard::Event) -> Option<i32> {
    use iced::keyboard::Event;
    let Event::KeyPressed {
        key,
        modified_key,
        modifiers,
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    if matches_character(key, "[") || matches_character(modified_key, "[") {
        Some(-1)
    } else if matches_character(key, "]") || matches_character(modified_key, "]") {
        Some(1)
    } else {
        None
    }
}

pub(crate) fn shortcut_help_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::Event;
    matches!(
        event,
        Event::KeyPressed {
            text: Some(text),
            modifiers,
            repeat: false,
            ..
        } if text.as_str() == "?"
            && !modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo()
    )
}

pub(crate) fn background_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F2),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

pub(crate) fn diagnostics_toggle_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F3),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

pub(crate) fn theme_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F4),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

pub(crate) fn project_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    // Key::Character carries a SmolStr, so the literal is compared rather than
    // pattern-matched.
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Character(character),
            modifiers,
            repeat: false,
            ..
        } if character == "p" && modifiers.control() && !modifiers.alt() && !modifiers.logo()
    )
}

pub(crate) fn new_task_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::Event;
    matches!(
        event,
        Event::KeyPressed {
            key,
            modified_key,
            modifiers,
            repeat: false,
            ..
        } if (matches_character(key, "n") || matches_character(modified_key, "n"))
            && modifiers.control()
            && !modifiers.shift()
            && !modifiers.alt()
            && !modifiers.logo()
    )
}

pub(crate) fn switcher_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    if matches!(event, Event::KeyPressed { key:Key::Named(iced::keyboard::key::Named::Space),modifiers,repeat:false,.. }
        if modifiers.control() && !modifiers.alt() && !modifiers.logo() && !modifiers.shift())
    {
        return true;
    }
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Character(character),
            modifiers,
            repeat: false,
            ..
        } if character == "p"
            && modifiers.control()
            && modifiers.shift()
            && !modifiers.alt()
            && !modifiers.logo()
    )
}
