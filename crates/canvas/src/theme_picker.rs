//! Visual selection of the bounded Coherence themes Canvas can persist.

use crate::theme_pack::ThemePack;
use canvas_core::{Accent, ThemeMode, ThemeSelection};
use iced::widget::{column, container, mouse_area, row, text};
use iced::{Color, Element, Length};

const COLUMNS: usize = 3;
const CHOICES: [ThemeSelection; 6] = [
    ThemeSelection::Coherence {
        mode: ThemeMode::Dark,
        accent: Accent::Brand,
    },
    ThemeSelection::Coherence {
        mode: ThemeMode::Dark,
        accent: Accent::Info,
    },
    ThemeSelection::Coherence {
        mode: ThemeMode::Dark,
        accent: Accent::Success,
    },
    ThemeSelection::Coherence {
        mode: ThemeMode::Light,
        accent: Accent::Brand,
    },
    ThemeSelection::Coherence {
        mode: ThemeMode::Light,
        accent: Accent::Info,
    },
    ThemeSelection::Coherence {
        mode: ThemeMode::Light,
        accent: Accent::Success,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Consumed,
    Ignored,
    Apply,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePicker {
    index: usize,
    choices: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    theme: ThemeSelection,
    mode: String,
    accent: String,
    pack_id: Option<String>,
}

impl ThemePicker {
    pub fn opened_from(theme: ThemeSelection, packs: &[ThemePack]) -> Self {
        let mut choices = packs
            .iter()
            .map(|pack| Choice {
                theme: ThemeSelection::Palette {
                    palette: pack.palette,
                },
                mode: "Personal".into(),
                accent: pack.name.clone(),
                pack_id: Some(pack.id.clone()),
            })
            .collect::<Vec<_>>();
        choices.extend(CHOICES.into_iter().map(|theme| {
            let (mode, accent) = labels(theme);
            Choice {
                theme,
                mode: mode.into(),
                accent: accent.into(),
                pack_id: None,
            }
        }));
        let index = choices
            .iter()
            .position(|choice| choice.theme == theme)
            .unwrap_or(0);
        Self { index, choices }
    }

    pub fn selection(&self) -> ThemeSelection {
        self.choices[self.index].theme
    }

    pub fn selected_pack_id(&self) -> Option<&str> {
        self.choices[self.index].pack_id.as_deref()
    }

    pub fn selection_index(&self) -> usize {
        self.index
    }

    pub fn select(&mut self, index: usize) {
        self.index = index % self.choices.len();
    }

    pub fn handle(&mut self, event: &iced::keyboard::Event) -> Action {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed { key, modifiers, .. } = event else {
            return Action::Ignored;
        };
        if !modifiers.is_empty() {
            return Action::Ignored;
        }
        let count = self.choices.len();
        match key {
            Key::Named(Named::ArrowRight) => self.index = (self.index + 1) % count,
            Key::Named(Named::ArrowLeft) => self.index = (self.index + count - 1) % count,
            Key::Named(Named::ArrowDown) => self.index = (self.index + COLUMNS) % count,
            Key::Named(Named::ArrowUp) => self.index = (self.index + count - COLUMNS) % count,
            Key::Named(Named::Enter) => return Action::Apply,
            Key::Named(Named::Escape) => return Action::Cancel,
            _ => return Action::Ignored,
        }
        Action::Consumed
    }

    pub fn view<'a, Message: Clone + 'a>(
        &self,
        on_pick: &'a dyn Fn(usize) -> Message,
    ) -> Element<'a, Message> {
        let selected = self.selection();
        let shell = selected.palette();
        let mut grid = column![].spacing(10);
        for chunk in self.choices.chunks(COLUMNS) {
            let mut line = row![].spacing(10);
            for (index, choice) in chunk.iter().enumerate() {
                let abs = self.choices.iter().position(|c| c == choice).unwrap_or(0);
                let _ = index; // chunk-local index; abs is the global index
                let tile = theme_tile(choice, choice.theme == selected);
                line = line.push(
                    mouse_area(tile)
                        .on_press(on_pick(abs))
                        .interaction(iced::mouse::Interaction::Pointer),
                );
            }
            grid = grid.push(line);
        }
        let choice = &self.choices[self.index];
        let (mode, accent) = (choice.mode.as_str(), choice.accent.as_str());
        let heading = text(format!("Theme — {mode} / {accent}"))
            .size(16)
            .color(rgb(shell.text));
        let detail = text("←→↑↓ choose   Enter apply   Esc cancel")
            .size(12)
            .color(rgb(shell.text_muted));
        container(column![heading, grid, detail].spacing(12))
            .padding(18)
            .style(move |_| container::Style {
                background: Some(with_alpha(shell.surface, 0.96).into()),
                border: iced::Border {
                    color: with_alpha(shell.border, 0.9),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            })
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

fn theme_tile<'a, Message: 'a>(choice: &Choice, selected: bool) -> Element<'a, Message> {
    let palette = choice.theme.palette();
    let (mode, accent) = (&choice.mode, &choice.accent);
    let swatches = row![
        swatch(palette.background),
        swatch(palette.surface),
        swatch(palette.primary),
        swatch(palette.success),
        swatch(palette.warning),
    ]
    .spacing(5);
    container(column![text(format!("{mode} · {accent}")).size(13), swatches].spacing(12))
        .width(Length::Fixed(208.0))
        .height(Length::Fixed(92.0))
        .padding(14)
        .style(move |_| container::Style {
            text_color: Some(rgb(palette.text)),
            background: Some(rgb(palette.background_strong).into()),
            border: iced::Border {
                color: if selected {
                    rgb(palette.primary)
                } else {
                    rgb(palette.border)
                },
                width: if selected { 2.0 } else { 1.0 },
                radius: 7.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn swatch<'a, Message: 'a>(value: [u8; 3]) -> Element<'a, Message> {
    container("")
        .width(Length::Fixed(24.0))
        .height(Length::Fixed(24.0))
        .style(move |_| container::Style {
            background: Some(rgb(value).into()),
            border: iced::Border {
                color: Color::from_rgba8(255, 255, 255, 0.18),
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn labels(theme: ThemeSelection) -> (&'static str, &'static str) {
    match theme {
        ThemeSelection::Coherence { mode, accent } => (
            match mode {
                ThemeMode::Light => "Light",
                ThemeMode::Dark => "Dark",
            },
            match accent {
                Accent::Brand => "Brand",
                Accent::Info => "Info",
                Accent::Success => "Success",
            },
        ),
        ThemeSelection::Palette { .. } => ("Imported", "Custom"),
    }
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

fn with_alpha(value: [u8; 3], alpha: f32) -> Color {
    Color::from_rgba8(value[0], value[1], value[2], alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Event, Key, Location, Modifiers, key};

    fn pressed(key: Key) -> Event {
        Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        }
    }

    #[test]
    fn opens_on_the_active_native_theme_and_wraps() {
        let active = ThemeSelection::coherence(ThemeMode::Light, Accent::Info);
        let mut picker = ThemePicker::opened_from(active, &[]);
        assert_eq!(picker.selection(), active);
        picker.handle(&pressed(Key::Named(key::Named::ArrowRight)));
        assert_eq!(
            picker.selection(),
            ThemeSelection::coherence(ThemeMode::Light, Accent::Success)
        );
        picker.handle(&pressed(Key::Named(key::Named::ArrowRight)));
        assert_eq!(
            picker.selection(),
            ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand)
        );
    }

    #[test]
    fn enter_applies_and_escape_cancels() {
        let mut picker = ThemePicker::opened_from(CHOICES[0], &[]);
        assert_eq!(
            picker.handle(&pressed(Key::Named(key::Named::Enter))),
            Action::Apply
        );
        assert_eq!(
            picker.handle(&pressed(Key::Named(key::Named::Escape))),
            Action::Cancel
        );
    }
}
