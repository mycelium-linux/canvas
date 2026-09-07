use dock_mac::resources::{self, Entry, Index};
use iced::widget::{button, column, container, markdown, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Task, Theme};

#[derive(Debug, Clone)]
enum Message {
    Loaded(Result<Index, String>),
    Query(String),
    Choose(usize),
    Open,
    Opened(Result<(), String>),
    Link(String),
    Keyboard(iced::keyboard::Event),
    Tick,
    Capture(iced::window::Screenshot),
}

struct Palette {
    index: Index,
    query: String,
    company: Option<Option<String>>,
    project: Option<String>,
    include_inactive: bool,
    cursor: usize,
    loading: bool,
    opening: bool,
    error: Option<String>,
    markdown: Vec<markdown::Item>,
    catalog: dock_core::Catalog,
    hotkey: Option<super::summon::SummonHotkey>,
    shortcut_error: Option<String>,
    refreshed: std::time::Instant,
    captured: bool,
}

impl Palette {
    fn refresh(&mut self) -> Task<Message> {
        if self.loading {
            return Task::none();
        }
        self.loading = true;
        Task::perform(async { resources::load() }, Message::Loaded)
    }
    fn folders(&self) -> Vec<resources::Folder> {
        resources::folders(&self.index, &self.query, self.include_inactive)
    }
    fn selected_folder(&self) -> Option<resources::Folder> {
        if self.company.is_none() {
            self.folders().get(self.cursor).cloned()
        } else {
            None
        }
    }
    fn matches(&self) -> Vec<usize> {
        match &self.company {
            None => (0..self.folders().len()).collect(),
            Some(company) => resources::folder_matches(
                &self.index,
                &self.query,
                company.as_deref(),
                self.project.as_deref(),
                self.include_inactive,
            ),
        }
    }
    fn selected(&self) -> Option<&Entry> {
        self.company.as_ref()?;
        self.matches()
            .get(self.cursor)
            .and_then(|i| self.index.entries.get(*i))
    }
    fn preview(&mut self) {
        self.markdown = markdown::parse(
            self.selected()
                .and_then(|e| e.note.as_deref())
                .unwrap_or(""),
        )
        .collect();
    }
    fn breadcrumb(&self) -> String {
        let Some(company) = &self.company else {
            return "Companies".into();
        };
        let company = company
            .as_ref()
            .and_then(|id| {
                self.index
                    .companies
                    .iter()
                    .find(|c| c.id == *id)
                    .map(|c| c.name.as_str())
            })
            .unwrap_or("Unassigned");
        if let Some(project) = self
            .project
            .as_ref()
            .and_then(|id| self.index.entries.iter().find(|e| e.project == *id))
        {
            format!("Companies / {company} / {}", project.project_name)
        } else {
            format!("Companies / {company}")
        }
    }
    fn theme(&self) -> Theme {
        let project = self
            .selected()
            .and_then(|entry| self.catalog.projects.iter().find(|p| p.id == entry.project));
        let theme = project.map(|p| p.theme).unwrap_or(dock_core::ProjectTheme {
            mode: dock_core::ThemeMode::Dark,
            accent: dock_core::Accent::Brand,
        });
        let p = theme.palette();
        let color = |rgb: [u8; 3]| Color::from_rgb8(rgb[0], rgb[1], rgb[2]);
        Theme::custom(
            "Coherence project resources",
            iced::theme::Palette {
                background: color(p.background),
                text: color(p.text),
                primary: color(p.primary),
                success: color(p.success),
                danger: color(p.danger),
                warning: color(p.warning),
            },
        )
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                self.refreshed = std::time::Instant::now();
                match result {
                    Ok(index) => {
                        let selected = self
                            .selected()
                            .map(|entry| (entry.project.clone(), entry.id.clone()));
                        self.index = index;
                        if self
                            .company
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|id| !self.index.companies.iter().any(|c| c.id == *id))
                        {
                            self.company = None;
                            self.project = None;
                        } else if self.project.as_ref().is_some_and(|id| {
                            !self.index.entries.iter().any(|entry| {
                                entry.project == *id
                                    && Some(&entry.company_id) == self.company.as_ref()
                            })
                        }) {
                            self.project = None;
                        }
                        self.error = None;
                        self.cursor = self
                            .matches()
                            .iter()
                            .position(|i| {
                                selected.as_ref().is_some_and(|(project, id)| {
                                    self.company.is_some()
                                        && self.index.entries[*i].project == *project
                                        && self.index.entries[*i].id == *id
                                })
                            })
                            .unwrap_or(0);
                        self.preview();
                    }
                    Err(error) => {
                        if error == dock_mac::alfalfa::AUTHORIZATION_DENIED {
                            self.index = Index::default();
                            self.company = None;
                            self.project = None;
                            self.cursor = 0;
                            self.preview();
                        }
                        self.error = Some(error);
                    }
                }
            }
            Message::Query(query) => {
                self.query = query;
                self.cursor = 0;
                self.preview();
            }
            Message::Choose(cursor) => {
                self.cursor = cursor;
                self.preview();
            }
            Message::Open => {
                if self.opening {
                    return Task::none();
                }
                if let Some(folder) = self.selected_folder() {
                    self.company = Some(folder.company_id);
                    self.query.clear();
                    self.cursor = 0;
                    self.preview();
                    return iced::widget::operation::focus("resource-query");
                }
                if let Some(entry) = self.selected().cloned() {
                    if entry.kind == "project" && self.project.is_none() {
                        self.project = Some(entry.project);
                        self.query.clear();
                        self.cursor = 0;
                        self.preview();
                        return iced::widget::operation::focus("resource-query");
                    }
                    if entry.note.is_some() {
                        return iced::widget::operation::focus("resource-note");
                    }
                    self.opening = true;
                    return Task::perform(async move { resources::open(&entry) }, Message::Opened);
                }
            }
            Message::Link(uri) => {
                if self.opening {
                    return Task::none();
                }
                if let Some(mut entry) = self.selected().cloned() {
                    entry.uri = uri;
                    self.opening = true;
                    return Task::perform(async move { resources::open(&entry) }, Message::Opened);
                }
            }
            Message::Opened(result) => {
                self.opening = false;
                match result {
                    Ok(()) => {
                        self.error = None;
                        return iced::window::oldest()
                            .and_then(|id| iced::window::minimize(id, true));
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Message::Keyboard(iced::keyboard::Event::KeyPressed {
                key,
                modifiers,
                repeat: false,
                ..
            }) => {
                use iced::keyboard::{key::Named, Key};
                match key.as_ref() {
                    Key::Named(Named::ArrowDown) => {
                        self.cursor = (self.cursor + 1).min(self.matches().len().saturating_sub(1));
                        self.preview();
                    }
                    Key::Named(Named::ArrowUp) => {
                        self.cursor = self.cursor.saturating_sub(1);
                        self.preview();
                    }
                    Key::Named(Named::Enter) => return self.update(Message::Open),
                    Key::Named(Named::Escape) => {
                        if self.project.take().is_some() {
                            self.query.clear();
                            self.cursor = 0;
                            self.preview();
                            return iced::widget::operation::focus("resource-query");
                        }
                        if self.company.take().is_some() {
                            self.query.clear();
                            self.cursor = 0;
                            self.preview();
                            return iced::widget::operation::focus("resource-query");
                        }
                        return iced::window::oldest()
                            .and_then(|id| iced::window::minimize(id, true));
                    }
                    Key::Character("r") if modifiers.command() => return self.refresh(),
                    Key::Character("a" | "A") if modifiers.command() && modifiers.shift() => {
                        self.include_inactive = !self.include_inactive;
                        self.cursor = 0;
                        self.preview();
                    }
                    _ => {}
                }
            }
            Message::Tick => {
                if self.hotkey.as_ref().is_some_and(|key| key.poll_pressed()) {
                    self.company = None;
                    self.project = None;
                    self.query.clear();
                    self.cursor = 0;
                    self.preview();
                    let focus = iced::window::oldest().and_then(|id| {
                        Task::batch([
                            iced::window::minimize(id, false),
                            iced::window::gain_focus(id),
                            iced::widget::operation::focus("resource-query"),
                            iced::widget::operation::select_all("resource-query"),
                        ])
                    });
                    return if self.refreshed.elapsed().as_secs() > 30 {
                        Task::batch([focus, self.refresh()])
                    } else {
                        focus
                    };
                }
                if !self.loading && !self.captured && std::env::var_os("DOCK_SCREENSHOT").is_some()
                {
                    self.captured = true;
                    return iced::window::oldest()
                        .and_then(iced::window::screenshot)
                        .map(Message::Capture);
                }
            }
            Message::Capture(screenshot) => {
                if let Some(path) = std::env::var_os("DOCK_SCREENSHOT") {
                    if let Err(error) = image::save_buffer(
                        std::path::PathBuf::from(path),
                        &screenshot.rgba,
                        screenshot.size.width,
                        screenshot.size.height,
                        image::ColorType::Rgba8,
                    ) {
                        self.error = Some(error.to_string());
                    } else {
                        return iced::exit();
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }
    fn view(&self) -> Element<'_, Message> {
        let matches = self.matches();
        let start = self.cursor.saturating_sub(4);
        let mut items = column![].spacing(4);
        for (position, index) in matches.iter().enumerate().skip(start).take(5) {
            let (title, detail) = if self.company.is_none() {
                let folders = self.folders();
                let folder = &folders[*index];
                let duplicate = self
                    .index
                    .companies
                    .iter()
                    .filter(|c| c.name == folder.name)
                    .count()
                    > 1;
                (
                    folder.name.clone(),
                    if duplicate {
                        format!(
                            "{} projects · {}",
                            folder.projects,
                            folder.company_id.as_deref().unwrap_or("Unassigned")
                        )
                    } else {
                        format!(
                            "{} {}",
                            folder.projects,
                            if folder.projects == 1 {
                                "project"
                            } else {
                                "projects"
                            }
                        )
                    },
                )
            } else {
                let entry = &self.index.entries[*index];
                (
                    if self.project.is_none() {
                        entry.project_name.clone()
                    } else {
                        entry.title.clone()
                    },
                    if self.project.is_none() {
                        entry.status.clone()
                    } else {
                        format!("{} · {}", entry.project_name, entry.kind)
                    },
                )
            };
            items = items.push(
                button(column![text(title).size(15), text(detail).size(12)].spacing(4))
                    .width(Length::Fill)
                    .padding(12)
                    .style(if position == self.cursor {
                        button::primary
                    } else {
                        button::text
                    })
                    .on_press(Message::Choose(position)),
            );
        }
        if matches.is_empty() {
            items = items.push(
                text(if self.loading {
                    "Loading project resources…"
                } else if self.error.is_some() {
                    "Project resources are unavailable. Press Command-R to retry."
                } else {
                    "No matching resources. Add notes or links in your Alfalfa project."
                })
                .size(15),
            );
        }
        let preview: Element<'_, Message> = if let Some(folder) = self.selected_folder() {
            column![
                text(folder.name).size(22),
                text(if folder.company_id.is_none() {
                    "Projects without an assigned company."
                } else {
                    "Choose a project in this company."
                })
                .size(17),
                text("Enter to browse projects").size(13)
            ]
            .spacing(20)
            .into()
        } else if let Some(entry) = self.selected() {
            let body: Element<'_, Message> = if entry.note.is_some() {
                scrollable(
                    markdown::view(&self.markdown, self.theme())
                        .map(|uri| Message::Link(uri.to_string())),
                )
                .id("resource-note")
                .height(Length::Fill)
                .into()
            } else {
                column![
                    text(if entry.kind == "project" && !entry.purpose.is_empty() {
                        &entry.purpose
                    } else {
                        "Open this resource in its existing app."
                    })
                    .size(17),
                    text(resources::target(entry)).size(13),
                    text(if self.project.is_none() {
                        "Enter to switch project · notes and dashboards inside"
                    } else {
                        "Enter to open"
                    })
                    .size(13)
                ]
                .spacing(20)
                .into()
            };
            column![
                text(if self.project.is_none() {
                    &entry.project_name
                } else {
                    &entry.title
                })
                .size(22),
                body
            ]
            .spacing(20)
            .into()
        } else {
            text("Your project context, within reach.").size(20).into()
        };
        let footer = self
            .error
            .as_deref()
            .or(self.index.warnings.first().map(String::as_str))
            .or(self.shortcut_error.as_deref())
            .unwrap_or(if self.project.is_some() {
                "↑ ↓ choose · Enter open · Esc back to projects"
            } else if self.company.is_some() {
                "↑ ↓ choose · Enter browse · Esc back to companies · ⌘⇧A inactive"
            } else if self.include_inactive {
                "All projects · ⌘⇧A hide inactive · ↑ ↓ choose · Enter browse · Esc hide"
            } else {
                "Ctrl+Option+Space summon · ↑ ↓ choose · Enter browse · ⌘⇧A inactive"
            });
        container(
            column![
                text(self.breadcrumb()).size(12),
                text_input(
                    if self.project.is_some() {
                        "Notes and dashboards in this project…"
                    } else if self.company.is_some() {
                        "Switch project…"
                    } else {
                        "Companies and projects…"
                    },
                    &self.query
                )
                .id("resource-query")
                .on_input(Message::Query)
                .padding(14)
                .size(18),
                row![
                    container(scrollable(items).height(Length::Fill)).width(Length::FillPortion(2)),
                    container(preview)
                        .padding(16)
                        .width(Length::FillPortion(3))
                        .height(Length::Fill)
                ]
                .spacing(20)
                .height(Length::Fill),
                row![
                    text(footer).size(12).width(Length::Fill),
                    text(format!(
                        "{} / {}",
                        if matches.is_empty() {
                            0
                        } else {
                            self.cursor + 1
                        },
                        matches.len()
                    ))
                    .size(12)
                ]
            ]
            .spacing(20)
            .padding(24),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

pub fn run(catalog: dock_core::Catalog) -> iced::Result {
    iced::application(
        move || {
            let (hotkey, shortcut_error) = match super::summon::SummonHotkey::register() {
                Ok(key) => (Some(key), None),
                Err(_) => (
                    None,
                    Some("Shortcut is in use; Command-Tab to Dock.".into()),
                ),
            };
            let mut app = Palette {
                index: Index::default(),
                query: std::env::var("DOCK_RESOURCE_QUERY").unwrap_or_default(),
                company: None,
                project: None,
                include_inactive: false,
                cursor: 0,
                loading: false,
                opening: false,
                error: None,
                markdown: vec![],
                catalog: catalog.clone(),
                hotkey,
                shortcut_error,
                refreshed: std::time::Instant::now(),
                captured: false,
            };
            let task = Task::batch([
                app.refresh(),
                iced::widget::operation::focus("resource-query"),
            ]);
            (app, task)
        },
        Palette::update,
        Palette::view,
    )
    .title("Dock")
    .theme(Palette::theme)
    .window_size((880.0, 560.0))
    .centered()
    .subscription(|_| {
        iced::Subscription::batch([
            iced::time::every(std::time::Duration::from_millis(100)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Keyboard(event) => Some(Message::Keyboard(event)),
                _ => None,
            }),
        ])
    })
    .run()
}

#[cfg(test)]
mod company_navigation_tests {
    use super::*;
    fn app() -> Palette {
        Palette{index:Index{companies:vec![resources::Company{id:"canonical".into(),name:"Company".into(),role:"member".into()}],entries:vec![serde_json::from_value(serde_json::json!({"company_id":"canonical","project":"project","project_name":"Project","id":"@home","title":"Home","kind":"project","uri":"https://example.test","note":null,"home":"https://example.test"})).unwrap()],warnings:vec![]},query:String::new(),company:None,project:None,include_inactive:false,cursor:0,loading:false,opening:false,error:None,markdown:vec![],catalog:dock_core::Catalog::bundled().unwrap(),hotkey:None,shortcut_error:None,refreshed:std::time::Instant::now(),captured:false}
    }
    fn escape() -> Message {
        use iced::keyboard::*;
        Message::Keyboard(Event::KeyPressed {
            key: Key::Named(key::Named::Escape),
            modified_key: Key::Named(key::Named::Escape),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }
    #[test]
    fn enter_and_escape_walk_company_project_resource_levels() {
        let mut app = app();
        assert_eq!(app.matches().len(), 1);
        assert!(app.selected().is_none());
        let _ = app.update(Message::Open);
        assert_eq!(app.company, Some(Some("canonical".into())));
        assert!(app.project.is_none());
        let _ = app.update(Message::Open);
        assert_eq!(app.project.as_deref(), Some("project"));
        let _ = app.update(escape());
        assert!(app.project.is_none());
        assert!(app.company.is_some());
        let _ = app.update(escape());
        assert!(app.company.is_none());
    }
    #[test]
    fn authorization_denial_removes_cached_project_resources() {
        let mut app = app();
        let _ = app.update(Message::Open);
        let _ = app.update(Message::Open);
        let _ = app.update(Message::Loaded(Err(
            dock_mac::alfalfa::AUTHORIZATION_DENIED.into(),
        )));
        assert!(app.index.entries.is_empty());
        assert!(app.index.companies.is_empty());
        assert!(app.project.is_none());
        assert!(app.company.is_none());
        assert!(app.error.is_some());
    }
    #[test]
    fn lost_membership_resets_open_project_without_reclassifying_it() {
        let mut app = app();
        let _ = app.update(Message::Open);
        let _ = app.update(Message::Open);
        let mut refreshed = app.index.clone();
        refreshed.companies.clear();
        let _ = app.update(Message::Loaded(Ok(refreshed)));
        assert!(app.project.is_none());
        assert!(app.company.is_none());
        assert!(app.matches().is_empty());
        assert_eq!(
            app.index.entries[0].company_id.as_deref(),
            Some("canonical")
        );
    }
}
