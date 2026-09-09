//! Keyboard-only fuzzy navigation across the catalog's project identities.

use canvas_core::{Catalog, Project};
use iced::widget::{column, container, row, text};
use iced::{Color, Element, Length};

const MAX_RESULTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Consumed,
    Ignored,
    Apply(String),
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPicker {
    query: String,
    selected: usize,
}

impl Default for ProjectPicker {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: 0,
        }
    }
}

impl ProjectPicker {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected_project(&self, catalog: &Catalog) -> Option<String> {
        self.matches(catalog)
            .get(self.selected)
            .map(|project| project.id.clone())
    }

    fn matches<'a>(&self, catalog: &'a Catalog) -> Vec<&'a Project> {
        let mut matches = catalog
            .projects
            .iter()
            .filter_map(|project| fuzzy_score(&self.query, project).map(|score| (score, project)))
            .collect::<Vec<_>>();
        matches.sort_by(|(a_score, a), (b_score, b)| {
            a_score.cmp(b_score).then_with(|| a.id.cmp(&b.id))
        });
        matches
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, project)| project)
            .collect()
    }

    fn clamp_selection(&mut self, catalog: &Catalog) {
        let len = self.matches(catalog).len();
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn handle(&mut self, event: &iced::keyboard::Event, catalog: &Catalog) -> Action {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed {
            key,
            modifiers,
            repeat: false,
            text,
            ..
        } = event
        else {
            return Action::Ignored;
        };
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return Action::Ignored;
        }
        let count = self.matches(catalog).len();
        match key {
            Key::Named(Named::ArrowDown) if count > 0 => {
                self.selected = (self.selected + 1) % count
            }
            Key::Named(Named::ArrowUp) if count > 0 => {
                self.selected = (self.selected + count - 1) % count
            }
            Key::Named(Named::Enter) => {
                return self
                    .selected_project(catalog)
                    .map_or(Action::Consumed, Action::Apply);
            }
            Key::Named(Named::Escape) => return Action::Cancel,
            Key::Named(Named::Backspace) => {
                self.query.pop();
                self.clamp_selection(catalog);
            }
            Key::Character(_) => {
                if let Some(text) = text {
                    if text.chars().all(|character| !character.is_control()) {
                        self.query.push_str(text);
                        self.clamp_selection(catalog);
                    } else {
                        return Action::Ignored;
                    }
                } else {
                    return Action::Ignored;
                }
            }
            _ => return Action::Ignored,
        }
        Action::Consumed
    }

    pub fn view<'a, Message: 'a>(
        &self,
        catalog: &'a Catalog,
        current: &str,
    ) -> Element<'a, Message> {
        let matches = self.matches(catalog);
        let palette = catalog
            .projects
            .iter()
            .find(|project| project.id == current)
            .map(|project| project.theme.palette())
            .unwrap_or_else(|| {
                canvas_core::ThemeSelection::coherence(
                    canvas_core::ThemeMode::Dark,
                    canvas_core::Accent::Brand,
                )
                .palette()
            });
        let mut results = column![].spacing(4);
        for (index, project) in matches.iter().enumerate() {
            let selected = index == self.selected;
            results = results.push(
                row![
                    text(if selected { "›" } else { " " }).color(rgb(palette.primary)),
                    text(&project.name).color(rgb(palette.text)),
                    text(format!("  {}", project.id))
                        .size(12)
                        .color(rgb(palette.text_muted)),
                ]
                .spacing(8),
            );
        }
        if matches.is_empty() {
            results = results.push(text("No matching projects").color(rgb(palette.text_muted)));
        }
        container(
            column![
                text(format!(
                    "Go to project  {}",
                    if self.query.is_empty() {
                        "·"
                    } else {
                        &self.query
                    }
                ))
                .size(16)
                .color(rgb(palette.text)),
                results,
                text("Type to filter   ↑↓ choose   Enter switch   Esc cancel")
                    .size(12)
                    .color(rgb(palette.text_muted)),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(560.0))
        .padding(20)
        .style(move |_| container::Style {
            background: Some(
                Color {
                    a: 0.97,
                    ..rgb(palette.surface)
                }
                .into(),
            ),
            border: iced::Border {
                color: rgb(palette.border),
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        })
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }
}

fn fuzzy_score(query: &str, project: &Project) -> Option<u32> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let id = project.id.to_ascii_lowercase();
    let name = project.name.to_ascii_lowercase();
    if id.starts_with(&query) || name.starts_with(&query) {
        return Some(10);
    }
    if id.contains(&query) || name.contains(&query) {
        return Some(20);
    }
    let mut subsequence_score: Option<u32> = None;
    for candidate in [&id, &name] {
        let mut position = 0;
        let mut gaps = 0;
        let mut found = true;
        for character in query.chars() {
            let Some(offset) = candidate[position..].find(character) else {
                found = false;
                break;
            };
            gaps += offset;
            position += offset + character.len_utf8();
        }
        if found {
            let score = 30 + gaps as u32;
            subsequence_score = Some(subsequence_score.map_or(score, |best| best.min(score)));
        }
    }
    subsequence_score
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Event, Key, Location, Modifiers, key};

    fn catalog() -> Catalog {
        serde_json::from_str(r#"{"schema_version":1,"projects":[
          {"id":"dock","name":"Dock","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]},
          {"id":"studio","name":"Studio","theme":{"source":"coherence","mode":"light","accent":"brand"},"resources":[]},
          {"id":"pedalkernel","name":"Pedal Kernel","theme":{"source":"coherence","mode":"dark","accent":"success"},"resources":[]}
        ]}"#).unwrap()
    }

    fn key(key: Key, text: Option<&'static str>) -> Event {
        Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: text.map(Into::into),
            repeat: false,
        }
    }

    #[test]
    fn ranks_prefix_contains_and_subsequence_matches() {
        let catalog = catalog();
        let mut picker = ProjectPicker::default();
        picker.handle(&key(Key::Character("pk".into()), Some("pk")), &catalog);
        assert_eq!(
            picker.selected_project(&catalog).as_deref(),
            Some("pedalkernel")
        );
        picker.query.clear();
        picker.handle(&key(Key::Character("stu".into()), Some("stu")), &catalog);
        assert_eq!(picker.selected_project(&catalog).as_deref(), Some("studio"));
    }

    #[test]
    fn keyboard_actions_are_bounded_and_explicit() {
        let catalog = catalog();
        let mut picker = ProjectPicker::default();
        assert_eq!(
            picker.handle(&key(Key::Named(key::Named::ArrowDown), None), &catalog),
            Action::Consumed
        );
        assert_eq!(
            picker.handle(&key(Key::Named(key::Named::Enter), None), &catalog),
            Action::Apply("studio".into())
        );
        assert_eq!(
            picker.handle(&key(Key::Named(key::Named::Escape), None), &catalog),
            Action::Cancel
        );
    }
}
