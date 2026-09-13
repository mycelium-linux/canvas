//! Cmd+Tab-style projector switcher with project labels and previews.
//!
//! Activated by Ctrl-Shift-P, shows a horizontal strip of project cards.
//! Each card has a colored preview rectangle (based on the project theme),
//! the project name, and a status indicator. Live previews may be faked
//! (periodic capture is sufficient per the Bead).

use canvas_core::{Catalog, Project};
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Length};
const MAX_CARDS: usize = 8;

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
            .map(|p| p.id.clone())
    }

    fn matches<'a>(&self, catalog: &'a Catalog) -> Vec<&'a Project> {
        let query = self.query.to_lowercase();
        if query.is_empty() {
            catalog.projects.iter().take(MAX_CARDS).collect()
        } else {
            catalog
                .projects
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&query) || p.id.contains(&query))
                .take(MAX_CARDS)
                .collect()
        }
    }

    pub fn handle(&mut self, event: &iced::keyboard::Event, catalog: &Catalog) -> Action {
        use iced::keyboard::{Event, Key, Modifiers};
        match event {
            Event::KeyPressed {
                key: Key::Character(c),
                ..
            } if c
                .chars()
                .next()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false) =>
            {
                self.query.push(c.chars().next().unwrap());
                self.selected = 0;
                Action::Consumed
            }
            Event::KeyPressed {
                key: Key::Named(iced::keyboard::key::Named::Backspace),
                ..
            } => {
                self.query.pop();
                self.selected = 0;
                Action::Consumed
            }
            Event::KeyPressed {
                key: Key::Named(iced::keyboard::key::Named::Escape),
                ..
            } => Action::Cancel,
            Event::KeyPressed {
                key: Key::Named(iced::keyboard::key::Named::Enter),
                ..
            } => self
                .selected(catalog)
                .map(Action::Apply)
                .unwrap_or(Action::Consumed),
            Event::KeyPressed {
                key: Key::Named(iced::keyboard::key::Named::ArrowLeft),
                ..
            } => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                Action::Consumed
            }
            Event::KeyPressed {
                key: Key::Named(iced::keyboard::key::Named::ArrowRight),
                ..
            } => {
                if self.selected + 1 < self.matches(catalog).len() {
                    self.selected += 1;
                }
                Action::Consumed
            }
            _ => {
                if !Modifiers::default().is_empty() {
                    Action::Consumed
                } else {
                    Action::Ignored
                }
            }
        }
    }

    pub fn view<'a, Message: 'a>(
        &self,
        catalog: &'a Catalog,
        current: &str,
    ) -> Element<'a, Message> {
        let matches = self.matches(catalog);
        let cards: Vec<Element<'_, Message>> = matches
            .iter()
            .enumerate()
            .map(|(index, project)| {
                let is_selected = index == self.selected;
                let is_active = project.id == current;
                let preview = container(space::horizontal())
                    .width(Length::Fixed(48.0))
                    .height(Length::Fixed(32.0))
                    .style(move |_| iced::widget::container::Style {
                        background: Some(if is_active {
                            Color::from_rgba(0.39, 0.78, 0.99, 0.78).into()
                        } else if is_selected {
                            Color::from_rgba(100.0, 100.0, 100.0, 0.6).into()
                        } else {
                            Color::from_rgba(60.0, 60.0, 60.0, 0.4).into()
                        }),
                        ..Default::default()
                    });
                let label_color = if is_selected {
                    Color::WHITE
                } else {
                    Color::from_rgba(200.0, 200.0, 200.0, 1.0)
                };
                let status_dot = container(space::horizontal())
                    .width(Length::Fixed(10.0))
                    .height(Length::Fixed(10.0))
                    .style(move |_| iced::widget::container::Style {
                        background: Some(if is_active {
                            Color::from_rgba(34.0, 197.0, 94.0, 1.0).into()
                        } else {
                            Color::from_rgba(100.0, 100.0, 100.0, 0.6).into()
                        }),
                        ..Default::default()
                    });
                let card = column![
                    preview,
                    row![text(&project.name).size(14).color(label_color), status_dot,],
                    text(&project.id)
                        .size(10)
                        .color(Color::from_rgba(150.0, 150.0, 150.0, 0.8)),
                ]
                .spacing(4);
                container(card)
                    .padding(8)
                    .width(Length::Shrink)
                    .style(move |_| iced::widget::container::Style {
                        background: if is_selected {
                            Some(Color::from_rgba(50.0, 50.0, 60.0, 0.8).into())
                        } else {
                            Some(Color::from_rgba(35.0, 35.0, 40.0, 0.7).into())
                        },
                        ..Default::default()
                    })
                    .into()
            })
            .collect();

        let query_str = if self.query.is_empty() {
            "Filter projects (type to filter)...".to_string()
        } else {
            self.query.clone()
        };
        let query_box = text(query_str)
            .size(14)
            .color(Color::from_rgba(200.0, 200.0, 200.0, 1.0));

        let bar = row![query_box].padding(8);
        let bar_bg =
            container(bar)
                .width(Length::Fill)
                .style(move |_| iced::widget::container::Style {
                    background: Some(Color::from_rgba(20.0, 20.0, 25.0, 0.9).into()),
                    ..Default::default()
                });

        let cards_row = row(cards).spacing(12).padding(16);
        let content = column![bar_bg, cards_row];
        container(content)
            .width(Length::Fill)
            .height(Length::Shrink)
            .style(move |_| iced::widget::container::Style {
                background: Some(Color::from_rgba(15.0, 15.0, 20.0, 0.95).into()),
                ..Default::default()
            })
            .into()
    }
}
