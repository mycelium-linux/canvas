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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPicker {
    query: String,
    selected: usize,
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

    /// Best matches first, capped at `MAX_RESULTS`. The sort is stable and has
    /// no tie-break, so equally good matches keep catalog order: that order is
    /// authored in `config/projects.json`, and its first entry is the project
    /// the rest of Canvas treats as the default. Ranking by id instead would
    /// bury the default behind whatever sorts alphabetically first, and would
    /// look arbitrary next to the names the rows actually show.
    fn matches<'a>(&self, catalog: &'a Catalog) -> Vec<&'a Project> {
        let mut matches = catalog
            .projects
            .iter()
            .filter_map(|project| fuzzy_score(&self.query, project).map(|score| (score, project)))
            .collect::<Vec<_>>();
        matches.sort_by_key(|(score, _)| *score);
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
        // Arrow keys and Enter work even with Ctrl held: the picker opens via
        // Ctrl-P, and a user who hasn't released Ctrl yet should not find the
        // keys silently ignored. Alt and logo (Cmd) are still rejected to avoid
        // colliding with desktop-level hotkeys.
        if modifiers.alt() || modifiers.logo() {
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
    // Theme filtering: "dark", "light", "brand", "info", "success" match themes.
    // Only activates for single-word theme queries to avoid false positives.
    let theme_desc = match project.theme {
        canvas_core::ThemeSelection::Coherence { mode, accent } => {
            format!(
                "{} {}",
                match mode {
                    canvas_core::ThemeMode::Dark => "dark",
                    canvas_core::ThemeMode::Light => "light",
                },
                match accent {
                    canvas_core::Accent::Brand => "brand",
                    canvas_core::Accent::Info => "info",
                    canvas_core::Accent::Success => "success",
                }
            )
        }
        _ => "theme".into(),
    };
    // If query is a known theme keyword, match against theme description only.
    if matches!(
        query.as_str(),
        "dark" | "light" | "brand" | "info" | "success" | "theme"
    ) {
        if theme_desc.starts_with(&query) || theme_desc.contains(&query) {
            return Some(10);
        }
        return None;
    }
    // Otherwise, match against id and name only (not theme description).
    let id = project.id.to_ascii_lowercase();
    let name = project.name.to_ascii_lowercase();
    for candidate in [&id, &name] {
        if candidate.starts_with(&query) {
            return Some(10);
        }
        if candidate.contains(&query) {
            return Some(20);
        }
    }
    // Subsequence fallback on id/name only.
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
            return Some(30 + gaps as u32);
        }
    }
    None
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

    fn catalog_of(ids: &[&str]) -> Catalog {
        let projects = ids
            .iter()
            .map(|id| {
                format!(
                    r#"{{"id":"{id}","name":"{id}","theme":{{"source":"coherence","mode":"dark","accent":"brand"}},"resources":[]}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        serde_json::from_str(&format!(
            r#"{{"schema_version":1,"projects":[{projects}]}}"#
        ))
        .unwrap()
    }

    fn listed(picker: &ProjectPicker, catalog: &Catalog) -> Vec<String> {
        picker
            .matches(catalog)
            .iter()
            .map(|project| project.id.clone())
            .collect()
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

    #[test]
    fn an_empty_query_lists_the_catalog_in_its_own_order() {
        let catalog = catalog();
        let picker = ProjectPicker::default();
        let ids = catalog
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(listed(&picker, &catalog), ids);
        // Opening the picker must preselect the project Canvas defaults to.
        assert_eq!(picker.selected_project(&catalog).as_deref(), Some("dock"));
    }

    #[test]
    fn ranking_is_independent_of_catalog_order() {
        let mut picker = ProjectPicker::default();
        picker.query.push_str("notes");
        // A prefix match outranks a mere substring match from either position.
        for order in [["studio-notes", "notes"], ["notes", "studio-notes"]] {
            let catalog = catalog_of(&order);
            assert_eq!(listed(&picker, &catalog), ["notes", "studio-notes"]);
        }
    }

    #[test]
    fn arrow_keys_wrap_within_the_result_set() {
        let catalog = catalog();
        let mut picker = ProjectPicker::default();
        let count = picker.matches(&catalog).len();
        let first = picker.selected_project(&catalog);
        for _ in 0..count {
            picker.handle(&key(Key::Named(key::Named::ArrowDown), None), &catalog);
            assert!(picker.selected_project(&catalog).is_some());
        }
        assert_eq!(picker.selected_project(&catalog), first);
        picker.handle(&key(Key::Named(key::Named::ArrowUp), None), &catalog);
        assert_ne!(picker.selected_project(&catalog), first);
        picker.handle(&key(Key::Named(key::Named::ArrowDown), None), &catalog);
        assert_eq!(picker.selected_project(&catalog), first);
    }

    #[test]
    fn narrowing_the_query_keeps_the_selection_in_range() {
        let catalog = catalog();
        let mut picker = ProjectPicker::default();
        while picker.selected + 1 < picker.matches(&catalog).len() {
            picker.handle(&key(Key::Named(key::Named::ArrowDown), None), &catalog);
        }
        picker.handle(&key(Key::Character("s".into()), Some("s")), &catalog);
        assert!(picker.selected < picker.matches(&catalog).len());
        assert_eq!(picker.selected_project(&catalog).as_deref(), Some("studio"));
    }

    #[test]
    fn results_are_ranked_before_they_are_capped() {
        let mut ids = (0..MAX_RESULTS + 2)
            .map(|index| format!("z-ab-{index}"))
            .collect::<Vec<_>>();
        ids.push("abz".into());
        let catalog = catalog_of(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        let mut picker = ProjectPicker::default();
        picker.query.push_str("ab");
        let matches = listed(&picker, &catalog);
        assert_eq!(matches.len(), MAX_RESULTS);
        // The cap must not drop the best match just because it is listed last.
        assert_eq!(matches[0], "abz");
    }
}
