//! Choosing a workspace background by looking at it.
//!
//! The shaders are a Rust enum rendered from WGSL, and nothing in that chain
//! tells a person what `dusk-ember` looks like. Picking one meant editing a
//! document by hand, restarting, and judging the result — so in practice nobody
//! changed it. This shows all of them at once, running, and commits the choice
//! to the workspace the human is standing in.
//!
//! The thumbnails are live rather than captured stills, because every one of
//! these shaders is motion: a still frame of `slow-ripple` and a still frame of
//! `tidepool` are two blue rectangles, and the thing being chosen between is
//! precisely what a still cannot show. They also share one clock, so what is on
//! screen is genuinely comparable rather than seven pieces caught at seven
//! different moments of their own cycles.

use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, text};
use iced::{Color, Element, Length};

use crate::screensaver::{self, SHADERS};
use crate::{Background, BackgroundShader};

/// Selectable speeds, slowest first.
///
/// A ladder rather than a free value: the useful range is not linear — the
/// difference between 0.15 and 0.25 is the difference between meditative and
/// busy, while everything past 2.0 is the same blur. Zero is kept as a real
/// choice because a frozen shader is a legitimate still background, and it is
/// what several existing workspaces already store.
pub const SPEEDS: [f32; 7] = [0.0, 0.15, 0.25, 0.5, 1.0, 2.0, 4.0];

const DEFAULT_SPEED_INDEX: usize = 3;
const COLUMNS: usize = 4;
const TILE_WIDTH: f32 = 208.0;
const TILE_HEIGHT: f32 = 117.0;

/// What the caller should do after handing the picker a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing the caller needs to act on; the picker consumed the key.
    Consumed,
    /// The key was not the picker's; the caller should handle it as usual.
    Ignored,
    /// Commit the current selection to the active workspace.
    Apply,
    /// Close without changing anything.
    Cancel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundPicker {
    index: usize,
    speed_index: usize,
}

impl BackgroundPicker {
    /// Open on whatever the workspace is already showing.
    ///
    /// Landing on the current background rather than the first one means the
    /// picker opens showing the human where they are, so "no, keep it" is
    /// pressing Escape rather than remembering which of seven it was.
    pub fn opened_from(background: &Background) -> Self {
        match background {
            Background::Shader { shader_id, speed } => Self {
                index: screensaver::index_for(*shader_id),
                speed_index: nearest_speed_index(*speed),
            },
            _ => Self {
                index: 0,
                speed_index: DEFAULT_SPEED_INDEX,
            },
        }
    }

    pub fn selected(&self) -> BackgroundShader {
        SHADERS[self.index % SHADERS.len()].id
    }

    pub fn speed(&self) -> f32 {
        SPEEDS[self.speed_index]
    }

    /// The background this picker currently describes.
    pub fn background(&self) -> Background {
        Background::Shader {
            shader_id: self.selected(),
            speed: self.speed(),
        }
    }

    pub fn handle(&mut self, event: &iced::keyboard::Event) -> Action {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed { key, modifiers, .. } = event else {
            return Action::Ignored;
        };
        // Shift is allowed through: `+` and `_` are shifted keys, and rejecting
        // every modifier meant the picker advertised `−/+` while silently
        // ignoring the only way to type `+`. Command, control and option still
        // belong to the desktop underneath.
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return Action::Ignored;
        }
        let count = SHADERS.len();
        match key {
            Key::Named(Named::ArrowRight) => self.index = (self.index + 1) % count,
            Key::Named(Named::ArrowLeft) => self.index = (self.index + count - 1) % count,
            // Down and up move a whole row, which is what the grid layout
            // implies; wrapping through the modulus keeps a short last row from
            // being a dead end.
            Key::Named(Named::ArrowDown) => self.index = (self.index + COLUMNS) % count,
            Key::Named(Named::ArrowUp) => {
                self.index = (self.index + count - COLUMNS % count) % count
            }
            Key::Named(Named::Enter) => return Action::Apply,
            Key::Named(Named::Escape) => return Action::Cancel,
            Key::Character(c) if c.as_str() == "-" || c.as_str() == "_" => {
                self.speed_index = self.speed_index.saturating_sub(1);
            }
            Key::Character(c) if c.as_str() == "=" || c.as_str() == "+" => {
                self.speed_index = (self.speed_index + 1).min(SPEEDS.len() - 1);
            }
            _ => return Action::Ignored,
        }
        Action::Consumed
    }

    /// Move the highlight to a tile the pointer chose.
    pub fn select(&mut self, index: usize) {
        self.index = index % SHADERS.len();
    }

    /// Whether a click on `index` is a second click on the already-chosen tile.
    ///
    /// Click to highlight, click again to commit: the same two steps the
    /// keyboard takes with arrows and Enter, rather than a single click
    /// silently changing the desktop under the pointer.
    pub fn is_selected(&self, index: usize) -> bool {
        index % SHADERS.len() == self.index % SHADERS.len()
    }

    pub fn view<'a, Message: Clone + 'a>(
        &self,
        elapsed: Duration,
        palette: &canvas_core::ThemePalette,
        on_pick: &'a dyn Fn(usize) -> Message,
    ) -> Element<'a, Message> {
        let text_color = rgb(palette.text);
        let muted = rgb(palette.text_muted);
        let accent = rgb(palette.primary);

        let mut grid = column![].spacing(10);
        for chunk in (0..SHADERS.len()).collect::<Vec<_>>().chunks(COLUMNS) {
            let mut line = row![].spacing(10);
            for &index in chunk {
                line = line.push(self.tile(index, elapsed, text_color, muted, accent, on_pick));
            }
            grid = grid.push(line);
        }

        let speed = self.speed();
        let heading = text(format!(
            "Background — {}",
            SHADERS[self.index % SHADERS.len()].name
        ))
        .size(16)
        .color(text_color);
        let detail = text(format!(
            "{}   ·   speed {}   ·   ←→↑↓ choose   − / + speed   Enter apply   Esc cancel",
            screensaver::attribution(self.index),
            if speed == 0.0 {
                "frozen".to_string()
            } else {
                format!("{speed:.2}×")
            }
        ))
        .size(12)
        .color(muted);

        let surface = palette.surface;
        let border = palette.border;
        container(column![heading, grid, detail].spacing(12))
            .padding(18)
            .style(move |_| container::Style {
                background: Some(
                    Color::from_rgba8(surface[0], surface[1], surface[2], 0.92).into(),
                ),
                border: iced::Border {
                    color: Color::from_rgba8(border[0], border[1], border[2], 0.9),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            })
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn tile<'a, Message: Clone + 'a>(
        &self,
        index: usize,
        elapsed: Duration,
        text_color: Color,
        muted: Color,
        accent: Color,
        on_pick: &'a dyn Fn(usize) -> Message,
    ) -> Element<'a, Message> {
        let chosen = index == self.index % SHADERS.len();
        // The selected tile previews at the chosen speed and the rest run at a
        // steady reference rate. Showing every tile at the selected speed would
        // make a speed change look like all seven shaders changed.
        // A frozen background is a real choice, but freezing the tile under the
        // cursor makes the picker look broken — the one preview a person is
        // looking at is the one that stops. Frozen previews at the reference
        // rate and says "frozen" in the caption instead.
        let tile_elapsed = if chosen && self.speed() > 0.0 {
            elapsed.mul_f32(self.speed())
        } else {
            elapsed
        };
        let preview = container(screensaver::view::<Message>(index, tile_elapsed))
            .width(Length::Fixed(TILE_WIDTH))
            .height(Length::Fixed(TILE_HEIGHT))
            .style(move |_| container::Style {
                border: iced::Border {
                    color: if chosen { accent } else { Color::TRANSPARENT },
                    width: if chosen { 2.0 } else { 0.0 },
                    radius: 6.0.into(),
                },
                ..Default::default()
            });
        mouse_area(
            column![
                preview,
                text(SHADERS[index].name)
                    .size(12)
                    .color(if chosen { text_color } else { muted })
            ]
            .spacing(4),
        )
        .on_press(on_pick(index))
        .into()
    }
}

fn nearest_speed_index(speed: f32) -> usize {
    SPEEDS
        .iter()
        .enumerate()
        .min_by(|(a_index, a), (b_index, b)| {
            let a_distance = (*a - speed).abs();
            let b_distance = (*b - speed).abs();
            if (a_distance - b_distance).abs() <= f32::EPSILON * 4.0 {
                b_index.cmp(a_index)
            } else {
                a_distance.total_cmp(&b_distance)
            }
        })
        .map(|(index, _)| index)
        .unwrap_or(DEFAULT_SPEED_INDEX)
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypress(key: iced::keyboard::Key) -> iced::keyboard::Event {
        iced::keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key.clone(),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        }
    }

    fn named(key: iced::keyboard::key::Named) -> iced::keyboard::Event {
        keypress(iced::keyboard::Key::Named(key))
    }

    fn character(c: &str) -> iced::keyboard::Event {
        keypress(iced::keyboard::Key::Character(c.into()))
    }

    #[test]
    fn it_opens_on_the_background_the_workspace_already_shows() {
        // Opening on the first shader would make "keep what I have" require
        // remembering which of seven it was.
        let picker = BackgroundPicker::opened_from(&Background::Shader {
            shader_id: BackgroundShader::SmokeRings,
            speed: 0.25,
        });
        assert_eq!(picker.selected(), BackgroundShader::SmokeRings);
        assert_eq!(picker.speed(), 0.25);
    }

    #[test]
    fn a_workspace_with_no_shader_opens_somewhere_sensible() {
        let picker = BackgroundPicker::opened_from(&Background::Theme);
        assert!(BackgroundShader::ALL.contains(&picker.selected()));
        assert!(picker.speed() > 0.0, "a first-time pick should be animated");
    }

    #[test]
    fn an_unlisted_speed_snaps_to_the_nearest_rung_rather_than_resetting() {
        // Documents written before the ladder existed hold arbitrary values;
        // dropping them to the default would silently restyle the workspace.
        let picker = BackgroundPicker::opened_from(&Background::Shader {
            shader_id: BackgroundShader::Tidepool,
            speed: 0.2,
        });
        assert!(
            (picker.speed() - 0.25).abs() < f32::EPSILON,
            "got {}",
            picker.speed()
        );
    }

    #[test]
    fn arrows_walk_every_shader_and_wrap() {
        let mut picker = BackgroundPicker::opened_from(&Background::Theme);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..SHADERS.len() {
            seen.insert(picker.selected());
            assert_eq!(
                picker.handle(&named(iced::keyboard::key::Named::ArrowRight)),
                Action::Consumed
            );
        }
        assert_eq!(seen.len(), SHADERS.len(), "every shader must be reachable");

        let start = picker.selected();
        picker.handle(&named(iced::keyboard::key::Named::ArrowLeft));
        assert_ne!(picker.selected(), start);
        picker.handle(&named(iced::keyboard::key::Named::ArrowRight));
        assert_eq!(picker.selected(), start, "left then right returns");
    }

    #[test]
    fn vertical_movement_never_strands_the_selection() {
        // The last row is short, so an unguarded +COLUMNS would index past the
        // end and a naive clamp would make Down a dead end.
        for shader in SHADERS {
            let mut picker = BackgroundPicker::opened_from(&Background::Shader {
                shader_id: shader.id,
                speed: 0.5,
            });
            picker.handle(&named(iced::keyboard::key::Named::ArrowDown));
            let after_down = picker.selected();
            picker.handle(&named(iced::keyboard::key::Named::ArrowUp));
            assert_eq!(picker.selected(), shader.id, "down then up returns");
            assert!(BackgroundShader::ALL.contains(&after_down));
        }
    }

    #[test]
    fn the_shifted_speed_key_the_caption_promises_actually_works() {
        // `+` is shift-`=`. Rejecting every modifier made the advertised key
        // silently do nothing, which reads as the picker being stuck.
        let mut picker = BackgroundPicker::opened_from(&Background::Theme);
        let before = picker.speed();
        let mut shifted = keypress(iced::keyboard::Key::Character("+".into()));
        if let iced::keyboard::Event::KeyPressed { modifiers, .. } = &mut shifted {
            *modifiers = iced::keyboard::Modifiers::SHIFT;
        }
        assert_eq!(picker.handle(&shifted), Action::Consumed);
        assert!(picker.speed() > before, "shift-= must raise the speed");
    }

    #[test]
    fn speed_is_bounded_at_both_ends_and_reaches_frozen() {
        let mut picker = BackgroundPicker::opened_from(&Background::Theme);
        for _ in 0..20 {
            picker.handle(&character("-"));
        }
        assert_eq!(picker.speed(), 0.0, "the slow end is a still background");
        for _ in 0..20 {
            picker.handle(&character("="));
        }
        assert_eq!(picker.speed(), *SPEEDS.last().unwrap());
        // The document validator rejects a speed outside 0.0..=4.0, so no rung
        // of the ladder may be able to produce an unsaveable background.
        assert!(
            SPEEDS
                .iter()
                .all(|s| s.is_finite() && (0.0..=4.0).contains(s)),
            "a rung outside the validated range would make the picker unsaveable"
        );
    }

    #[test]
    fn enter_applies_escape_cancels_and_anything_else_is_left_alone() {
        let mut picker = BackgroundPicker::opened_from(&Background::Theme);
        assert_eq!(
            picker.handle(&named(iced::keyboard::key::Named::Enter)),
            Action::Apply
        );
        assert_eq!(
            picker.handle(&named(iced::keyboard::key::Named::Escape)),
            Action::Cancel
        );
        // Unhandled keys must fall through so the rest of the app still works
        // while the picker happens to be open.
        assert_eq!(picker.handle(&character("q")), Action::Ignored);
        assert_eq!(
            picker.handle(&named(iced::keyboard::key::Named::F3)),
            Action::Ignored
        );
    }
}
