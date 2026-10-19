//! Keyboard project switching, with a responsive window onto the full catalog.

use canvas_core::{Accent, Catalog, Project, ThemeMode, ThemeSelection};
use iced::widget::{column, container, responsive, row, space, text};
use iced::{Color, Element, Length};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Consumed,
    Apply(String),
    Cancel,
    Ignored,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectSwitcher {
    query: String,
    selected: usize,
}

impl ProjectSwitcher {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected(&self, catalog: &Catalog) -> Option<String> {
        self.matches(catalog)
            .get(self.selected)
            .map(|project| project.id.clone())
    }

    fn matches<'a>(&self, catalog: &'a Catalog) -> Vec<&'a Project> {
        let query = self.query.trim().to_lowercase();
        catalog
            .projects
            .iter()
            .filter(|project| {
                project.name.to_lowercase().contains(&query)
                    || project.id.to_lowercase().contains(&query)
            })
            .collect()
    }

    pub fn handle(&mut self, event: &iced::keyboard::Event, catalog: &Catalog) -> Action {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed {
            key,
            modifiers,
            text,
            ..
        } = event
        else {
            return Action::Ignored;
        };
        if modifiers.alt() || modifiers.logo() {
            return Action::Ignored;
        }
        let count = self.matches(catalog).len();
        self.selected = self.selected.min(count.saturating_sub(1));
        match key {
            Key::Named(Named::Escape) => return Action::Cancel,
            Key::Named(Named::Enter) => {
                return self
                    .selected(catalog)
                    .map_or(Action::Consumed, Action::Apply);
            }
            Key::Named(Named::ArrowLeft | Named::ArrowUp) => {
                self.selected = self.selected.saturating_sub(1);
            }
            Key::Named(Named::ArrowRight | Named::ArrowDown) => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1));
            }
            Key::Named(Named::Home) => self.selected = 0,
            Key::Named(Named::End) => self.selected = count.saturating_sub(1),
            Key::Named(Named::Backspace) if !modifiers.control() => {
                self.query.pop();
                self.selected = 0;
            }
            Key::Character(_) | Key::Named(Named::Space) if !modifiers.control() => {
                let Some(input) = text.as_ref().filter(|input| {
                    !input.is_empty() && input.chars().all(|character| !character.is_control())
                }) else {
                    return Action::Ignored;
                };
                self.query.push_str(input);
                self.selected = 0;
            }
            _ => return Action::Ignored,
        }
        Action::Consumed
    }

    pub fn view<'a, Message: 'a>(
        &'a self,
        catalog: &'a Catalog,
        current: &'a str,
    ) -> Element<'a, Message> {
        responsive(move |size| self.panel(catalog, current, size.width)).into()
    }

    fn panel<'a, Message: 'a>(
        &'a self,
        catalog: &'a Catalog,
        current: &str,
        available_width: f32,
    ) -> Element<'a, Message> {
        let visual = catalog
            .projects
            .iter()
            .find(|project| project.id == current)
            .map_or_else(
                || ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand),
                |project| project.theme,
            )
            .visual_theme();
        let p = visual.palette;
        let m = visual.metrics;
        let width = available_width.min(900.0);
        let capacity = (((width - 4.0 * m.space_4) / 180.0).floor() as usize).clamp(1, 4);
        let matches = self.matches(catalog);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let range = visible_range(matches.len(), selected, capacity);
        let mut cards = row![].spacing(m.space_2);
        for index in range {
            let project = matches[index];
            let chosen = index == selected;
            let active = project.id == current;
            let accent = rgb(project.theme.palette().primary);
            let card = column![
                container(space::horizontal())
                    .height(4)
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(accent.into()),
                        ..Default::default()
                    }),
                text(&project.name)
                    .size(m.body)
                    .color(rgb(p.text))
                    .wrapping(text::Wrapping::WordOrGlyph),
                text(&project.id)
                    .size(m.small)
                    .color(rgb(p.text_muted))
                    .wrapping(text::Wrapping::WordOrGlyph),
                text(if active { "Current workspace" } else { " " })
                    .size(m.small)
                    .color(rgb(p.text_muted)),
            ]
            .spacing(m.space_2)
            .width(Length::Fill);
            cards = cards.push(
                container(card)
                    .padding(m.space_3)
                    .width(Length::FillPortion(1))
                    .style(move |_| container::Style {
                        background: Some(
                            rgb(if chosen {
                                visual.surface_accent
                            } else {
                                visual.surface_raised
                            })
                            .into(),
                        ),
                        border: iced::Border {
                            color: rgb(if chosen { visual.focus } else { p.border }),
                            width: if chosen { 2.0 } else { 1.0 },
                            radius: m.radius_sm.into(),
                        },
                        ..Default::default()
                    }),
            );
        }
        let position = if matches.is_empty() {
            "0 projects".into()
        } else {
            format!("{} of {}", selected + 1, matches.len())
        };
        let mut content = column![
            row![
                text("Switch project").size(m.title).color(rgb(p.text)),
                space::horizontal(),
                text(position).size(m.small).color(rgb(p.text_muted))
            ]
            .align_y(iced::Alignment::Center),
            text(if self.query.is_empty() {
                "Type a project name…"
            } else {
                &self.query
            })
            .size(m.body)
            .color(rgb(if self.query.is_empty() {
                p.text_muted
            } else {
                p.text
            }))
            .width(Length::Fill)
            .wrapping(text::Wrapping::WordOrGlyph),
        ]
        .spacing(m.space_3);
        if matches.is_empty() {
            content = content.push(
                text(if catalog.projects.is_empty() {
                    "No projects are available in this catalog."
                } else {
                    "No matching projects. Use Backspace to change your search."
                })
                .size(m.body)
                .color(rgb(p.text_muted))
                .width(Length::Fill),
            );
        } else {
            content = content.push(cards);
        }
        content = content.push(
            text("← → or ↑ ↓ choose · Enter switch · Esc cancel")
                .size(m.small)
                .color(rgb(p.text_muted))
                .width(Length::Fill),
        );
        container(
            container(content)
                .padding(m.space_4)
                .width(Length::Fill)
                .max_width(900)
                .style(move |_| container::Style {
                    background: Some(rgb(p.surface).into()),
                    border: iced::Border {
                        color: rgb(p.border),
                        width: 1.0,
                        radius: m.radius_md.into(),
                    },
                    ..Default::default()
                }),
        )
        .padding(m.space_4)
        .center(Length::Fill)
        .into()
    }
}

fn visible_range(count: usize, selected: usize, capacity: usize) -> std::ops::Range<usize> {
    let capacity = capacity.max(1);
    let start = selected
        .saturating_sub(capacity / 2)
        .min(count.saturating_sub(capacity));
    start..(start + capacity).min(count)
}

fn rgb(value: [u8; 3]) -> Color {
    Color::from_rgb8(value[0], value[1], value[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Event, Key, Location, Modifiers, key};

    fn catalog() -> Catalog {
        let projects: Vec<_> = (0..12)
            .map(|index| {
                serde_json::json!({
                    "id":format!("project-{index}"), "name":format!("Project {index}"),
                    "theme":{"source":"coherence","mode":"dark","accent":"info"}, "resources":[]
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({"schema_version":1,"projects":projects})).unwrap()
    }
    fn press(key: Key, input: Option<&str>, modifiers: Modifiers) -> Event {
        Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: input.map(Into::into),
            repeat: false,
        }
    }
    #[test]
    fn all_projects_remain_reachable_and_the_window_follows_selection() {
        let catalog = catalog();
        let mut picker = ProjectSwitcher::default();
        for _ in 0..11 {
            picker.handle(
                &press(Key::Named(key::Named::ArrowRight), None, Modifiers::empty()),
                &catalog,
            );
        }
        assert_eq!(picker.selected(&catalog).as_deref(), Some("project-11"));
        assert_eq!(
            picker.handle(
                &press(Key::Named(key::Named::Enter), None, Modifiers::empty()),
                &catalog,
            ),
            Action::Apply("project-11".into()),
        );
        assert_eq!(visible_range(12, 11, 4), 8..12);
        for capacity in 1..=4 {
            for selected in 0..12 {
                let range = visible_range(12, selected, capacity);
                assert!(range.contains(&selected));
                assert!(range.len() <= capacity);
            }
        }
    }
    #[test]
    fn search_accepts_full_text_spaces_and_punctuation() {
        let catalog = catalog();
        let mut picker = ProjectSwitcher::default();
        picker.handle(
            &press(
                Key::Character("Project 11".into()),
                Some("Project 11"),
                Modifiers::empty(),
            ),
            &catalog,
        );
        assert_eq!(picker.query(), "Project 11");
        assert_eq!(picker.selected(&catalog).as_deref(), Some("project-11"));
        picker.query = "PROJECT-10".into();
        assert_eq!(picker.selected(&catalog).as_deref(), Some("project-10"));
    }
    #[test]
    fn shortcuts_do_not_type_and_an_empty_result_cannot_switch() {
        let catalog = catalog();
        let mut picker = ProjectSwitcher::default();
        assert_eq!(
            picker.handle(
                &press(Key::Character("p".into()), Some("p"), Modifiers::CTRL),
                &catalog
            ),
            Action::Ignored
        );
        assert!(picker.query().is_empty());
        picker.query = "missing".into();
        assert_eq!(
            picker.handle(
                &press(Key::Named(key::Named::Enter), None, Modifiers::empty()),
                &catalog
            ),
            Action::Consumed
        );
        assert_eq!(visible_range(0, 0, 1), 0..0);
    }
}
