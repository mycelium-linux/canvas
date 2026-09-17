mod project_panel;
mod resource_palette;
mod summon;
use canvas_core::{
    Action, Actor, Catalog, Control, Host, ResourceKind, StateStore, WorkspaceState,
};
use canvas_mac::reservation::{self, Desktop};
use canvas_mac::workspace::Bindings;
use canvas_mac::{Rect, RemoteAction, Window, WindowInventory};
use iced::widget::{Column, button, column, container, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Task, Theme};
use std::sync::{Arc, Mutex};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct App {
    diagnostics: bool,
    picker: usize,
    summon: Option<summon::SummonHotkey>,
    summon_error: Option<String>,
    catalog: Catalog,
    panels: BTreeMap<String, project_panel::Panel>,
    state: WorkspaceState,
    selected: usize,
    bindings: Bindings,
    windows: Vec<Window>,
    status: String,
    area: String,
    custom_area: bool,
    busy: bool,
    captured: bool,
    display_snapshot: Option<reservation::Snapshot>,
    reserved_displays: Vec<String>,
    destination_displays: Vec<String>,
    stream_plan: Option<reservation::Plan>,
    stream_host: String,
    stream: Arc<Mutex<Option<reservation::Stream>>>,
    stream_active: bool,
    stream_polling: bool,
    recovery_needed: bool,
}
#[derive(Debug, Clone)]
enum Message {
    Keyboard(iced::keyboard::Event),
    SummonTick,
    Line(String),
    Panel(String, project_panel::Message),
    RecallLinux,
    Displays(Result<reservation::Snapshot, String>),
    ReserveDisplay(String),
    DestinationDisplay(String),
    ReviewStream,
    StartStream,
    StopStream,
    RecoverStream,
    StreamStarted(Result<(), String>),
    StreamStopped(Result<(), String>),
    StreamPolled(Result<bool, String>),
    Capture(iced::window::Screenshot),
    Select(usize),
    Tick,
    TakeControl,
    ReleaseControl,
    Refresh,
    Windows(Result<WindowInventory, String>),
    Bind(Window),
    Unbind(usize),
    Focus(Window),
    Recall,
    Tile,
    WorkspaceActivated(String, Result<Vec<Window>, String>),
    Area(String),
    ToggleArea,
    Launch(usize),
    Remote(usize, RemoteAction),
    RemoteFinished(String, RemoteAction, Result<String, String>),
    Finished(Result<String, String>),
}
fn state_store() -> StateStore {
    StateStore::new(
        std::env::var_os("DOCK_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".dock/state.json")),
    )
}
fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}
impl App {
    fn new(catalog: Catalog, state: WorkspaceState) -> Self {
        let (bindings, status) = match canvas_mac::workspace::load() {
            Ok(value) => (
                value,
                "Ready. Refresh windows to attach existing work.".into(),
            ),
            Err(error) => (
                Bindings::default(),
                format!("Cannot read saved window bindings: {error}"),
            ),
        };
        let selected = catalog
            .projects
            .iter()
            .position(|p| p.id == state.selected_project)
            .unwrap_or(0);
        let panels = catalog
            .projects
            .iter()
            .map(|p| (p.id.clone(), project_panel::Panel::default()))
            .collect();
        Self {
            diagnostics: false,
            picker: selected,
            summon: None,
            summon_error: None,
            catalog,
            panels,
            state,
            selected,
            bindings,
            windows: vec![],
            status,
            area: String::new(),
            custom_area: false,
            busy: false,
            captured: false,
            display_snapshot: None,
            reserved_displays: vec![],
            destination_displays: vec![],
            stream_plan: None,
            stream_host: String::new(),
            stream: Arc::new(Mutex::new(None)),
            stream_active: false,
            stream_polling: false,
            recovery_needed: reservation::journal_path().is_ok_and(|p| p.exists()),
        }
    }
    fn project_windows(&self) -> Vec<Window> {
        self.bindings
            .windows(&self.catalog.projects[self.selected].id)
            .to_vec()
    }
    fn save(&mut self) {
        let result = canvas_mac::workspace::save(&self.bindings);
        self.status = result
            .map(|_| "Window bindings saved.".into())
            .unwrap_or_else(|e| format!("Bindings were not saved: {e}"));
    }
    fn theme(&self) -> Theme {
        let p = self.catalog.projects[self.selected].theme.palette();
        Theme::custom(
            self.catalog.projects[self.selected].name.clone(),
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
            Message::Line(line) => {
                let id = &self.catalog.projects[self.selected].id;
                self.panels.get_mut(id).unwrap().line = line;
            }
            Message::SummonTick => {
                if self.summon.as_ref().is_some_and(|h| h.poll_pressed()) {
                    return iced::window::oldest().and_then(|id| {
                        Task::batch([
                            iced::window::minimize(id, false),
                            iced::window::gain_focus(id),
                            iced::widget::operation::focus("prompt"),
                        ])
                    });
                }
            }
            Message::Keyboard(iced::keyboard::Event::KeyPressed {
                key,
                modifiers,
                repeat: false,
                ..
            }) => {
                use iced::keyboard::{Key, key::Named};
                match key {
                    Key::Named(Named::Escape) => return self.update(Message::TakeControl),
                    Key::Named(Named::ArrowUp) if !self.diagnostics => {
                        self.picker = self.picker.saturating_sub(1)
                    }
                    Key::Named(Named::ArrowDown) if !self.diagnostics => {
                        self.picker = (self.picker + 1).min(self.catalog.projects.len() - 1)
                    }
                    Key::Character(value) if modifiers.command() => {
                        if let Ok(index) = value.parse::<usize>() {
                            if index > 0 && index <= self.catalog.projects.len() {
                                return self.update(Message::Select(index - 1));
                            }
                        }
                    }
                    Key::Named(Named::Enter) if !self.diagnostics => {
                        let id = self.catalog.projects[self.selected].id.clone();
                        if self.panels[&id].line.trim().is_empty() {
                            return self.update(Message::Select(self.picker));
                        }
                        return self.update(Message::Panel(
                            id,
                            project_panel::Message::Run(project_panel::Action::SubmitTask),
                        ));
                    }
                    _ => {}
                }
            }
            Message::Keyboard(_) => {}

            Message::Panel(project, message) => {
                if matches!(
                    &message,
                    project_panel::Message::Run(project_panel::Action::Recall)
                ) && self.panels.values().any(|panel| panel.recalling)
                {
                    self.status = "Finish recalling the current project before switching.".into();
                    return Task::none();
                }
                let hide = matches!(
                    &message,
                    project_panel::Message::Done(_, project_panel::Action::Recall, Ok(_))
                ) && !self.diagnostics
                    && !self.panels[&project].queued_submit
                    && std::env::var_os("DOCK_SCREENSHOT").is_none();
                let panel = self.panels.get_mut(&project).unwrap();
                let previous_owner = panel.controller.clone();
                let task = panel.update(&project, message);
                if let Some(owner) = &panel.controller {
                    if previous_owner.as_ref() != Some(owner) {
                        let action = if owner == "human" {
                            Action::TakeControl(project.clone())
                        } else {
                            Action::ReleaseControl(project.clone())
                        };
                        match state_store().update(&self.catalog, |s| s.apply(Actor::Human, action))
                        {
                            Ok(state) => self.state = state,
                            Err(error) => self.status = error.to_string(),
                        }
                    }
                }
                let task = task.map(move |message| Message::Panel(project.clone(), message));
                return if hide {
                    Task::batch([
                        task,
                        iced::window::oldest().and_then(|id| iced::window::minimize(id, true)),
                    ])
                } else {
                    task
                };
            }
            Message::TakeControl | Message::ReleaseControl => {
                let id = self.catalog.projects[self.selected].id.clone();
                let action = if matches!(message, Message::TakeControl) {
                    match state_store().update(&self.catalog, |s| {
                        s.apply(Actor::Human, Action::TakeControl(id.clone()))
                    }) {
                        Ok(state) => self.state = state,
                        Err(error) => self.status = error.to_string(),
                    }
                    project_panel::Action::Human
                } else {
                    project_panel::Action::Agent
                };
                return self.update(Message::Panel(id, project_panel::Message::Run(action)));
            }
            Message::RecallLinux => {
                let id = self.catalog.projects[self.selected].id.clone();
                return self.update(Message::Panel(
                    id,
                    project_panel::Message::Run(project_panel::Action::Recall),
                ));
            }

            Message::Displays(result) => {
                self.busy = false;
                match result {
                    Ok(snapshot) => {
                        self.reserved_displays = snapshot
                            .displays
                            .first()
                            .map(|display| vec![display.id.clone()])
                            .unwrap_or_default();
                        self.destination_displays = snapshot
                            .displays
                            .iter()
                            .skip(1)
                            .map(|d| d.id.clone())
                            .collect();
                        self.display_snapshot = Some(snapshot);
                        self.stream_plan = None;
                        self.status="Choose the streaming display and where local windows should go, then review.".into();
                    }
                    Err(e) => self.status = e,
                }
            }
            Message::ReserveDisplay(id) => {
                if self.reserved_displays.contains(&id) {
                    self.reserved_displays.retain(|display| *display != id);
                } else {
                    self.reserved_displays.push(id.clone());
                    self.destination_displays.retain(|display| *display != id);
                }
                self.stream_plan = None;
            }
            Message::DestinationDisplay(id) => {
                if self.destination_displays.contains(&id) {
                    self.destination_displays.retain(|d| *d != id)
                } else {
                    self.destination_displays.push(id)
                }
                self.stream_plan = None;
            }
            Message::ReviewStream => {
                if let Some(snapshot) = &self.display_snapshot {
                    match reservation::plan_outputs(
                        snapshot,
                        &self.reserved_displays,
                        &self.destination_displays,
                    ) {
                        Ok(plan) => {
                            self.stream_plan = Some(plan);
                            self.status="Review every affected window below. Start stream applies this snapshot.".into()
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::StartStream => {
                if let Some(plan) = self.stream_plan.clone() {
                    self.busy = true;
                    let project = self.catalog.projects[self.selected].id.clone();
                    let shared = self.stream.clone();
                    return Task::perform(
                        async move {
                            let stream = reservation::start_project(&plan, &project)?;
                            *shared.lock().map_err(|_| "Stream state lock failed")? = Some(stream);
                            Ok(())
                        },
                        Message::StreamStarted,
                    );
                }
            }
            Message::StopStream => {
                self.busy = true;
                let shared = self.stream.clone();
                return Task::perform(
                    async move {
                        let stream = shared
                            .lock()
                            .map_err(|_| "Stream state lock failed")?
                            .take()
                            .ok_or("No managed stream is running")?;
                        stream.stop()
                    },
                    Message::StreamStopped,
                );
            }
            Message::RecoverStream => {
                self.busy = true;
                return Task::perform(
                    async {
                        reservation::restore(
                            &mut reservation::Native,
                            &reservation::journal_path()?,
                        )
                    },
                    Message::StreamStopped,
                );
            }
            Message::StreamStarted(result) => {
                self.busy = false;
                self.stream_active = result.is_ok();
                self.recovery_needed = reservation::journal_path().is_ok_and(|p| p.exists());
                self.status=match result{Ok(())=>"Agora One has the selected displays. Stop stream restores your local windows.".into(),Err(e)=>e};
            }
            Message::StreamStopped(result) => {
                self.busy = false;
                self.stream_active = false;
                self.recovery_needed = reservation::journal_path().is_ok_and(|p| p.exists());
                self.stream_plan = None;
                self.status = match result {
                    Ok(()) => "Local windows restored.".into(),
                    Err(e) => e,
                };
            }
            Message::StreamPolled(result) => {
                self.stream_polling = false;
                match result {
                    Ok(true) => {
                        self.stream_active = false;
                        self.status = "Stream ended; local windows restored.".into();
                        self.recovery_needed =
                            reservation::journal_path().is_ok_and(|p| p.exists());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        self.stream_active = false;
                        self.recovery_needed = true;
                        self.status = e;
                    }
                }
                let selected = self.catalog.projects[self.selected].id.clone();
                let tasks: Vec<_> = self
                    .panels
                    .iter_mut()
                    .filter(|(id, p)| **id == selected || p.owns_task())
                    .map(|(id, p)| {
                        let id = id.clone();
                        p.update(&id, project_panel::Message::Poll)
                            .map(move |m| Message::Panel(id.clone(), m))
                    })
                    .collect();
                return Task::batch(tasks);
            }

            Message::Capture(screenshot) => {
                if let Some(path) = std::env::var_os("DOCK_SCREENSHOT") {
                    let mut bytes = format!(
                        "P6\n{} {}\n255\n",
                        screenshot.size.width, screenshot.size.height
                    )
                    .into_bytes();
                    for pixel in screenshot.rgba.chunks_exact(4) {
                        bytes.extend_from_slice(&pixel[..3]);
                    }
                    if let Err(e) = std::fs::write(path, bytes) {
                        self.status = format!("Cannot save screenshot: {e}");
                    }
                }
            }
            Message::Tick => {
                match state_store().load(&self.catalog) {
                    Ok(state) => {
                        let selected = self
                            .catalog
                            .projects
                            .iter()
                            .position(|p| p.id == state.selected_project)
                            .unwrap_or(0);
                        self.state = state;
                        if selected != self.selected
                            && !self.panels.values().any(|panel| panel.recalling)
                        {
                            self.selected = selected;
                            let id = self.catalog.projects[selected].id.clone();
                            return self.update(Message::Panel(
                                id,
                                project_panel::Message::Run(project_panel::Action::Recall),
                            ));
                        }
                    }
                    Err(e) => self.status = e.to_string(),
                }
                if self.stream_active && !self.busy && !self.stream_polling {
                    self.stream_polling = true;
                    let shared = self.stream.clone();
                    return Task::perform(
                        async move {
                            let mut guard =
                                shared.lock().map_err(|_| "Stream state lock failed")?;
                            if let Some(stream) = guard.as_mut() {
                                if stream.ended()? {
                                    guard.take().unwrap().stop()?;
                                    return Ok(true);
                                }
                            }
                            Ok(false)
                        },
                        Message::StreamPolled,
                    );
                }
                if !self.captured
                    && std::env::var_os("DOCK_SCREENSHOT").is_some()
                    && !self.panels[&self.catalog.projects[self.selected].id].busy
                {
                    self.captured = true;
                    return iced::window::oldest()
                        .and_then(iced::window::screenshot)
                        .map(Message::Capture);
                }
                let selected = self.catalog.projects[self.selected].id.clone();
                let tasks: Vec<_> = self
                    .panels
                    .iter_mut()
                    .filter(|(id, p)| **id == selected || p.owns_task())
                    .map(|(id, p)| {
                        let id = id.clone();
                        p.update(&id, project_panel::Message::Poll)
                            .map(move |m| Message::Panel(id.clone(), m))
                    })
                    .collect();
                return Task::batch(tasks);
            }
            Message::Select(_) => {
                if self.panels.values().any(|panel| panel.recalling) {
                    self.status = "Finish recalling the current project before switching.".into();
                    return Task::none();
                }
                let action = match message {
                    Message::Select(index) => {
                        Action::SelectProject(self.catalog.projects[index].id.clone())
                    }
                    _ => unreachable!(),
                };
                match state_store().update(&self.catalog, |s| s.apply(Actor::Human, action)) {
                    Ok(state) => {
                        self.selected = self
                            .catalog
                            .projects
                            .iter()
                            .position(|p| p.id == state.selected_project)
                            .unwrap_or(0);
                        self.state = state;
                        self.picker = self.selected;
                        self.status = "Recalling this project's windows and conversation.".into();
                        let id = self.catalog.projects[self.selected].id.clone();
                        let windows = self.project_windows();
                        let native = if windows.is_empty() {
                            Task::none()
                        } else {
                            let native_project = id.clone();
                            Task::perform(
                                async move {
                                    let area = canvas_mac::work_area()?;
                                    canvas_mac::activate_project_windows(area, &windows)
                                        .map(|(resolved, _)| resolved)
                                },
                                move |result| {
                                    Message::WorkspaceActivated(native_project.clone(), result)
                                },
                            )
                        };
                        return Task::batch([
                            native,
                            self.update(Message::Panel(
                                id,
                                project_panel::Message::Run(project_panel::Action::Recall),
                            )),
                        ]);
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::Area(value) => self.area = value,
            Message::ToggleArea => self.custom_area = !self.custom_area,
            Message::Refresh => {
                self.busy = true;
                return Task::perform(async { canvas_mac::window_inventory() }, Message::Windows);
            }
            Message::Windows(result) => {
                self.busy = false;
                match result {
                    Ok(inventory) => {
                        self.windows = inventory.windows;
                        self.status = if inventory.warnings.is_empty() {
                            "Choose windows to attach to this project.".into()
                        } else {
                            format!(
                                "Choose responsive windows to attach. {} application warning(s): {}",
                                inventory.warnings.len(),
                                inventory.warnings.join("; ")
                            )
                        };
                    }
                    Err(e) => self.status = e,
                }
            }
            Message::Bind(window) => {
                self.bindings
                    .bind(&self.catalog.projects[self.selected].id, window);
                self.save();
            }
            Message::Unbind(index) => {
                let project = &self.catalog.projects[self.selected].id;
                if self.bindings.unbind_index(project, index) {
                    self.save();
                } else {
                    self.status = "Window binding is no longer present.".into();
                }
            }
            Message::Focus(window) => {
                self.busy = true;
                return Task::perform(
                    async move {
                        canvas_mac::focus(&window)
                            .map(|_| "Window focused. Keyboard and mouse remain yours.".into())
                    },
                    Message::Finished,
                );
            }
            Message::Recall | Message::Tile => {
                let ws = self.project_windows();
                if ws.is_empty() {
                    self.status = "Attach a window before recalling this project.".into();
                    return Task::none();
                }
                let project = self.catalog.projects[self.selected].id.clone();
                let custom_area = self.custom_area.then(|| self.area.clone());
                self.busy = true;
                return Task::perform(
                    async move {
                        let area = match custom_area {
                            Some(value) => parse_area(&value)?,
                            None => canvas_mac::work_area()?,
                        };
                        canvas_mac::activate_project_windows(area, &ws)
                            .map(|(resolved, _)| resolved)
                    },
                    move |result| Message::WorkspaceActivated(project.clone(), result),
                );
            }
            Message::WorkspaceActivated(project, result) => {
                self.busy = false;
                match result {
                    Ok(windows) => {
                        self.bindings.0.insert(project, windows);
                        self.save();
                        self.status =
                            "Project workspace arranged. Other windows remain available.".into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::Remote(index, action) => {
                let target = self.catalog.projects[self.selected].resources[index]
                    .target
                    .clone();
                let project_id = self.catalog.projects[self.selected].id.clone();
                self.busy = true;
                return Task::perform(
                    async move { canvas_mac::remote_session(&target, action) },
                    move |result| Message::RemoteFinished(project_id.clone(), action, result),
                );
            }
            Message::Launch(index) => {
                let resource = self.catalog.projects[self.selected].resources[index].clone();
                self.busy = true;
                if resource.kind == ResourceKind::RemoteDesktop {
                    self.stream_host = resource.target;
                    return Task::perform(
                        async { reservation::Native.snapshot() },
                        Message::Displays,
                    );
                }
                return Task::perform(
                    async move {
                        let mut command = Command::new("/usr/bin/open");
                        match (&resource.host, &resource.kind) {
                            (Host::Mac, ResourceKind::MacApp) => {
                                command.args(["-a", &resource.target]);
                            }
                            (_, ResourceKind::RemoteDesktop) => {
                                command.args(["-a", "Moonlight"]);
                            }
                            _ => {
                                return Err(
                                    "Session control is not connected for this resource yet."
                                        .into(),
                                );
                            }
                        }
                        let out = command.output().map_err(|e| e.to_string())?;
                        if !out.status.success() {
                            return Err(String::from_utf8_lossy(&out.stderr).trim().into());
                        }
                        Ok(if matches!(resource.kind, ResourceKind::RemoteDesktop) {
                            "Moonlight opened. Select the paired Agora One host.".into()
                        } else {
                            "Application opened. Refresh windows to attach it.".into()
                        })
                    },
                    Message::Finished,
                );
            }
            Message::RemoteFinished(project_id, action, result) => {
                self.busy = false;
                self.status = match result {
                    Ok(message) => format!("{project_id} / {action:?}: {message}"),
                    Err(e) => format!("Remote action failed: {e}"),
                };
            }
            Message::Finished(result) => {
                self.busy = false;
                self.status = result.unwrap_or_else(|e| format!("Action failed: {e}"));
            }
        }
        Task::none()
    }
    fn view(&self) -> Element<'_, Message> {
        if self.diagnostics {
            return self.diagnostics_view();
        }
        let project = &self.catalog.projects[self.selected];
        let palette = project.theme.palette();
        let panel = &self.panels[&project.id];
        let mut strip = row![].spacing(8);
        for (index, project) in self.catalog.projects.iter().enumerate() {
            strip = strip.push(
                button(text(&project.name).size(14))
                    .padding([8, 12])
                    .style(if index == self.picker {
                        button::primary
                    } else {
                        button::text
                    })
                    .on_press_maybe(
                        (!self.panels.values().any(|p| p.recalling))
                            .then_some(Message::Select(index)),
                    ),
            );
        }
        let response = panel.latest_response();
        let hint = self
            .summon_error
            .as_deref()
            .unwrap_or("Ctrl+Option+Space · Enter to send · Esc to stop");
        let content = column![
            strip,
            text(panel.progress())
                .size(14)
                .color(color(palette.text_muted)),
            scrollable(container(text(response).size(18)).padding(iced::Padding {
                right: 16.0,
                ..iced::Padding::default()
            }))
            .height(Length::Fill),
            text_input("Ask your project…", &panel.line)
                .id("prompt")
                .on_input(Message::Line)
                .size(14)
                .padding(12),
            text(hint).size(14).color(color(palette.text_muted))
        ]
        .spacing(16)
        .padding(24);
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    fn diagnostics_view(&self) -> Element<'_, Message> {
        let p = &self.catalog.projects[self.selected];
        let palette = p.theme.palette();
        let metrics = p.theme.metrics();
        let mut rail = Column::new()
            .spacing(metrics.space_2)
            .push(text("Dock").size(22));
        for (index, project) in self.catalog.projects.iter().enumerate() {
            let label = if index == self.selected {
                format!("●  {}", project.name)
            } else {
                project.name.clone()
            };
            rail = rail.push(
                button(text(label).size(14))
                    .style(if index == self.selected {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .width(Length::Fill)
                    .on_press_maybe(
                        (!self.panels.values().any(|panel| panel.recalling))
                            .then_some(Message::Select(index)),
                    ),
            );
        }
        let human = self.state.projects[&p.id].control == Control::Human;
        rail = rail
            .push(
                text(if human {
                    "You have control"
                } else {
                    "Agent control enabled"
                })
                .size(metrics.small),
            )
            .push(
                button(if human {
                    "Allow agent control"
                } else {
                    "Take control"
                })
                .on_press(if human {
                    Message::ReleaseControl
                } else {
                    Message::TakeControl
                }),
            )
            .push(text("Switching projects leaves ongoing work running.").size(12));
        let mut work = column![]
            .spacing(12)
            .width(Length::Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 20.0,
                bottom: 0.0,
                left: 0.0,
            });
        let project_id = p.id.clone();
        work = work.push(
            self.panels[&p.id]
                .view()
                .map(move |message| Message::Panel(project_id.clone(), message)),
        );
        work = work.push(
            row![
                text("On this Mac").size(22),
                button("Refresh windows").on_press_maybe((!self.busy).then_some(Message::Refresh))
            ]
            .spacing(20),
        );
        let attached = self.project_windows();
        if attached.is_empty() {
            work = work.push(text("Attach an existing window below. Your workspace starts with the work already open."));
        }
        for (index, window) in attached.into_iter().enumerate() {
            work = work.push(
                row![
                    button(text(format!("{}   {}", window.app, window.title)))
                        .on_press_maybe((!self.busy).then_some(Message::Focus(window))),
                    button("Detach").on_press(Message::Unbind(index))
                ]
                .spacing(8),
            );
        }
        work = work
            .push(
                row![
                    button("Recall project")
                        .on_press_maybe((!self.busy).then_some(Message::Recall)),
                    button("Tile attached windows")
                        .on_press_maybe((!self.busy).then_some(Message::Tile))
                ]
                .spacing(8),
            )
            .push(
                button(if self.custom_area {
                    "Use display bounds"
                } else {
                    "Custom work area"
                })
                .style(button::text)
                .on_press(Message::ToggleArea),
            );
        if self.custom_area {
            work = work.push(
                text_input("x,y,width,height in screen points", &self.area).on_input(Message::Area),
            );
        }
        for window in &self.windows {
            work = work.push(
                button(text(format!("Attach: {}   {}", window.app, window.title)).size(12))
                    .on_press(Message::Bind(window.clone())),
            );
        }
        work = work.push(text("Across your machines").size(22));
        work = work.push(
            button("Recall Linux workspace")
                .on_press_maybe((!self.busy).then_some(Message::RecallLinux)),
        );
        for (index, resource) in p.resources.iter().enumerate() {
            let can_launch = matches!(
                (&resource.host, &resource.kind),
                (Host::Mac, ResourceKind::MacApp) | (_, ResourceKind::RemoteDesktop)
            );
            work = work.push(
                row![
                    column![
                        text(resource.title.clone()),
                        text(match resource.host {
                            Host::Mac => "Mac",
                            Host::AgoraOne => "Agora One",
                            Host::EdgeRunner => "Edge Runner",
                        })
                        .size(metrics.small)
                    ]
                    .width(Length::Fill),
                    button(if matches!(resource.kind, ResourceKind::RemoteDesktop) {
                        "Reserve display"
                    } else {
                        "Open"
                    })
                    .on_press_maybe((can_launch && !self.busy).then_some(Message::Launch(index)))
                ]
                .spacing(12),
            );
            if !can_launch {
                if resource.host == Host::AgoraOne
                    && matches!(
                        resource.kind,
                        ResourceKind::TerminalSession | ResourceKind::AgentSession
                    )
                {
                    work = work.push(
                        row![
                            button("Inspect").on_press_maybe(
                                (!self.busy)
                                    .then_some(Message::Remote(index, RemoteAction::Status))
                            ),
                            button("Pause process group").on_press_maybe(
                                (!self.busy).then_some(Message::Remote(index, RemoteAction::Pause))
                            ),
                            button("Resume process group").on_press_maybe(
                                (!self.busy)
                                    .then_some(Message::Remote(index, RemoteAction::Resume))
                            )
                        ]
                        .spacing(metrics.space_2),
                    );
                    work = work.push(
                        text("Managed processes only. Interactive prompting is not connected.")
                            .size(metrics.small),
                    );
                } else {
                    work = work.push(text("Session adapter not connected").size(metrics.small));
                }
            }
        }
        if self.stream_active {
            work = work.push(
                button("Stop stream and restore windows")
                    .on_press_maybe((!self.busy).then_some(Message::StopStream)),
            );
        } else if self.recovery_needed {
            work = work
                .push(text("A previous stream left a window recovery journal."))
                .push(
                    button("Restore local windows")
                        .on_press_maybe((!self.busy).then_some(Message::RecoverStream)),
                );
        } else if let Some(snapshot) = &self.display_snapshot {
            work = work.push(text("Streaming displays").size(22));
            for display in &snapshot.displays {
                let selected = self.reserved_displays.contains(&display.id);
                work = work.push(
                    button(text(format!(
                        "{}{} ({} × {})",
                        if selected { "● " } else { "" },
                        display.name,
                        display.frame.width,
                        display.frame.height
                    )))
                    .style(if selected {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .on_press(Message::ReserveDisplay(display.id.clone())),
                );
            }
            work = work.push(text("Keep local windows on"));
            for display in snapshot
                .displays
                .iter()
                .filter(|display| !self.reserved_displays.contains(&display.id))
            {
                let selected = self.destination_displays.contains(&display.id);
                work = work.push(
                    button(text(format!(
                        "{}{}",
                        if selected { "● " } else { "" },
                        display.name
                    )))
                    .style(if selected {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .on_press(Message::DestinationDisplay(display.id.clone())),
                );
            }
            work = work.push(
                button("Review window moves")
                    .on_press_maybe((!self.busy).then_some(Message::ReviewStream)),
            );
            if let Some(plan) = &self.stream_plan {
                work=work.push(text(format!("{} windows will move. Their original positions will be restored when streaming stops.",plan.moves.len())));
                for movement in &plan.moves {
                    let destination = plan
                        .destinations
                        .iter()
                        .find(|d| {
                            movement.destination.x >= d.frame.x
                                && i64::from(movement.destination.x)
                                    < i64::from(d.frame.x) + i64::from(d.frame.width)
                        })
                        .map(|d| d.name.as_str())
                        .unwrap_or("selected display");
                    work = work.push(
                        text(format!(
                            "{} — {} → {}",
                            movement.window.app, movement.window.title, destination
                        ))
                        .size(metrics.small),
                    );
                }
                work = work.push(
                    button("Start stream")
                        .on_press_maybe((!self.busy).then_some(Message::StartStream)),
                );
            }
        }
        work = work.push(
            container(text(&self.status).size(metrics.small))
                .padding(metrics.space_3)
                .style(container::rounded_box),
        );
        let accent = color(palette.primary);
        container(
            row![
                container(rail).padding(20).width(190),
                container(scrollable(work)).padding(24).width(Length::Fill)
            ]
            .spacing(0),
        )
        .style(move |_| container::Style {
            border: iced::Border {
                color: accent,
                width: 4.0,
                radius: 0.0.into(),
            },
            background: Some(
                iced::Gradient::Linear(
                    iced::gradient::Linear::new(core::f32::consts::PI / 2.0)
                        .add_stop(0.0, color(palette.background))
                        .add_stop(1.0, color(palette.background_strong)),
                )
                .into(),
            ),
            ..Default::default()
        })
        .height(Length::Fill)
        .into()
    }
}
fn parse_area(value: &str) -> Result<Rect, String> {
    let values: Result<Vec<i32>, _> = value.split(',').map(|v| v.trim().parse()).collect();
    match values {
        Ok(v) if v.len() == 4 && v[2] > 0 && v[3] > 0 => Ok(Rect {
            x: v[0],
            y: v[1],
            width: v[2] as u32,
            height: v[3] as u32,
        }),
        _ => Err("Work area must be x,y,width,height with positive dimensions".into()),
    }
}
/// Every desktop a person can jump to: the bundled entries, the portfolio
/// folders Alfalfa seeded, and any ad hoc desktop made since.
///
/// Filtered to those with a saved workspace, so the switcher offers somewhere
/// that actually exists rather than a definition with nothing behind it.
fn desktop_catalog(base: &Catalog) -> Result<Catalog, String> {
    let path = canvas::workspace_store::default_path().map_err(|e| e.to_string())?;
    let (store, _recovery_notes) =
        canvas::workspace_store::load(&path, base).map_err(|e| e.to_string())?;
    let mut catalog = store.effective_catalog(base).map_err(|e| e.to_string())?;
    catalog
        .projects
        .retain(|project| store.projects.contains_key(&project.id));
    if catalog.projects.is_empty() {
        return Err("No saved Canvas desktops".into());
    }
    Ok(catalog)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["mcp-serve"] {
        canvas_mac::mcp::serve()?;
        return Ok(());
    }
    if let [command, port] = args.as_slice() {
        if command == "mcp-serve-http" {
            canvas_mac::mcp::serve_http(port.parse()?)?;
            return Ok(());
        }
    }
    if args.first().map(String::as_str) == Some("orchestrate") {
        let words: Vec<_> = args.iter().map(String::as_str).collect();
        let (tool, arguments) = match words.as_slice() {
            ["orchestrate", "list"] => ("projects_list", serde_json::json!({})),
            ["orchestrate", "displays"] => ("displays_list", serde_json::json!({})),
            ["orchestrate", "workspaces"] => ("workspaces_list", serde_json::json!({})),
            ["orchestrate", "canvas-sync"] => {
                ("canvas_workspaces_sync", serde_json::json!({}))
            }
            ["orchestrate", "canvas-switch", project] => (
                "canvas_workspace_switch",
                serde_json::json!({"project_id":project}),
            ),
            ["orchestrate", "presence"] => ("presence_route", serde_json::json!({})),
            ["orchestrate", "resources", project] => (
                "project_resources",
                serde_json::json!({"project_id":project}),
            ),
            ["orchestrate", "recall", project] => (
                "project_recall",
                serde_json::json!({"project_id":project}),
            ),
            ["orchestrate", "recall", project, target] => (
                "project_recall",
                serde_json::json!({"project_id":project,"target":target}),
            ),
            ["orchestrate", "activate", project] => (
                "workspace_activate",
                serde_json::json!({"project_id":project}),
            ),
            ["orchestrate", "activate", project, target] => (
                "workspace_activate",
                serde_json::json!({"project_id":project,"target":target}),
            ),
            ["orchestrate", "open", project, resource] => (
                "resource_open",
                serde_json::json!({"project_id":project,"resource_id":resource}),
            ),
            _ => {
                return Err("usage: canvas-mac orchestrate list | displays | workspaces | canvas-sync | canvas-switch PROJECT | presence | resources PROJECT | recall PROJECT [primary|presence] | activate PROJECT [primary|presence] | open PROJECT RESOURCE".into())
            }
        };
        let result = canvas_mac::mcp::execute(tool, &arguments)?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("resources") {
        let index = canvas_mac::resources::load()?;
        let query = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
        let entries: Vec<_> = canvas_mac::resources::matching(&index, &query)
            .into_iter()
            .map(|i| &index.entries[i])
            .collect();
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"companies":index.companies,"entries":entries,"warnings":index.warnings})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("desktop") {
        if args.len() != 2 {
            return Err("usage: canvas-mac desktop PROJECT".into());
        }
        let result = canvas_mac::remote::open_desktop(&args[1])?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("bridge") {
        let owner = match args.get(2).map(String::as_str) {
            Some("mac") => canvas_mac::bridge::Owner::Mac,
            Some("agora") => canvas_mac::bridge::Owner::Agora,
            _ => {
                return Err(
                    "usage: canvas-mac bridge status mac|agora | pair mac|agora PROJECT".into(),
                );
            }
        };
        let result = match args.get(1).map(String::as_str) {
            Some("status") if args.len() == 3 => canvas_mac::bridge::status(owner),
            Some("pair") if args.len() == 4 => canvas_mac::bridge::pair_in_project(owner, &args[3]),
            _ => Err("usage: canvas-mac bridge status mac|agora | pair mac|agora PROJECT".into()),
        }?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("alfalfa") {
        let words: Vec<_> = args.iter().map(String::as_str).collect();
        let result = match words.as_slice() {
            ["alfalfa", "list"] => canvas_mac::alfalfa::list(),
            ["alfalfa", "context", project] => canvas_mac::alfalfa::context(project),
            _ => Err("usage: canvas-mac alfalfa list | context PROJECT".into()),
        }?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("edgerunner") {
        let words: Vec<_> = args.iter().map(String::as_str).collect();
        let owner = |value: &str| -> Result<canvas_mac::bridge::Owner, String> {
            match value {
                "mac" => Ok(canvas_mac::bridge::Owner::Mac),
                "agora" => Ok(canvas_mac::bridge::Owner::Agora),
                _ => Err("Credential owner must be mac or agora".into()),
            }
        };
        let result = match words.as_slice() {
            ["edgerunner", "owner-list", target] => canvas_mac::edgerunner_authenticated::list(owner(target)?),
            ["edgerunner", "owner-status", target, session] => canvas_mac::edgerunner_authenticated::status(owner(target)?,session),
            ["edgerunner", "readiness", target, session] => canvas_mac::edgerunner_authenticated::readiness(owner(target)?,session),
            ["edgerunner", "list"] => canvas_mac::edgerunner::list(),
            ["edgerunner", "status", session] => canvas_mac::edgerunner::status(session),
            ["edgerunner", "bind", project, session] => canvas_mac::edgerunner::bind(project, session),
            ["edgerunner", "bound-status", project] => canvas_mac::edgerunner::status_for_project(project),
            _ => Err("usage: canvas-mac edgerunner owner-list mac|agora | owner-status mac|agora SESSION | readiness mac|agora SESSION | list | status SESSION | bind PROJECT SESSION | bound-status PROJECT".into()),
        }?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("remote") {
        if args.len() < 3 {
            return Err(
                "usage: canvas-mac remote PROJECT ACTION [ARGS]; session-prompt/resume read stdin"
                    .into(),
            );
        }
        let input = if matches!(args[2].as_str(), "session-prompt" | "session-resume") {
            use std::io::Read;
            let mut body = String::new();
            std::io::stdin().take(65537).read_to_string(&mut body)?;
            Some(body)
        } else {
            None
        };
        let result = canvas_mac::remote::rpc(&args[1], &args[2], &args[3..], input)?;
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"ok":true,"result":result})
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("work-area") {
        println!("{}", serde_json::to_string(&canvas_mac::work_area()?)?);
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("windows") {
        println!(
            "{}",
            serde_json::to_string_pretty(&canvas_mac::window_inventory()?)?
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("workspace") {
        let words = args.iter().map(String::as_str).collect::<Vec<_>>();
        match words.as_slice() {
            ["workspace", "list"] => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&canvas_mac::workspace::load()?)?
                );
            }
            ["workspace", "bind", project, window_id] => {
                canvas_mac::resources::project_entries(project)?;
                let inventory = canvas_mac::window_inventory()?;
                let window = inventory
                    .windows
                    .into_iter()
                    .find(|window| window.id == *window_id)
                    .ok_or("Window ID is absent from the current responsive inventory")?;
                let mut bindings = canvas_mac::workspace::load()?;
                bindings.bind(project, window);
                let window_count = bindings.windows(project).len();
                canvas_mac::workspace::save(&bindings)?;
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"ok":true,"project":project,"warnings":inventory.warnings,"window_count":window_count})
                );
            }
            ["workspace", "unbind", project, window_id] => {
                let mut bindings = canvas_mac::workspace::load()?;
                if !bindings.unbind_id(project, window_id) {
                    return Err("Window is not bound to that project".into());
                }
                let window_count = bindings.windows(project).len();
                canvas_mac::workspace::save(&bindings)?;
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"ok":true,"project":project,"window_count":window_count})
                );
            }
            ["workspace", "activate", project, "--human"] => {
                canvas_mac::resources::project_entries(project)?;
                let mut bindings = canvas_mac::workspace::load()?;
                let activation = canvas_mac::workspace::activate(&mut bindings, project, None)?;
                canvas_mac::workspace::save(&bindings)?;
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"ok":true,"result":activation})
                );
            }
            _ => {
                return Err("usage: canvas-mac workspace list | bind PROJECT WINDOW_ID | unbind PROJECT WINDOW_ID | activate PROJECT --human".into());
            }
        }
        return Ok(());
    }
    if matches!(args.first().map(String::as_str), Some("focus" | "tile")) {
        if args.len() < 4 {
            return Err("usage: canvas-mac focus PROJECT PID TITLE [--human] | tile PROJECT PID TITLE X,Y,WIDTH,HEIGHT [--human]".into());
        }
        let catalog = Catalog::bundled()?;
        catalog.project(&args[1])?;
        let state = state_store().load(&catalog)?;
        if !args.iter().any(|a| a == "--human")
            && state.projects[&args[1]].control != Control::Agent
        {
            return Err("Project is under human control. Release it through Dock before agent window actions.".into());
        }
        let mut w = Window {
            id: String::new(),
            pid: args[2].parse()?,
            title: args[3].clone(),
            app: String::new(),
        };
        let bindings = canvas_mac::workspace::load()?;
        if !bindings.0.get(&args[1]).is_some_and(|ws| {
            ws.iter()
                .any(|bound| bound.pid == w.pid && bound.title == w.title)
        }) {
            return Err("Window is not attached to this project; attach it in Dock first.".into());
        }
        w = bindings.0[&args[1]]
            .iter()
            .find(|bound| bound.pid == w.pid && bound.title == w.title)
            .unwrap()
            .clone();
        if args[0] == "focus" {
            canvas_mac::focus(&w)?;
        } else {
            canvas_mac::tile(&w, parse_area(args.get(4).ok_or("missing work area")?)?)?;
        }
        println!("{{\"ok\":true}}");
        return Ok(());
    }
    let base = if args.first().map(String::as_str) == Some("--catalog") {
        Catalog::load(args.get(1).ok_or("missing catalog path")?)?
    } else {
        Catalog::bundled()?
    };
    // The switcher lists the desktops that exist, not the two in the bundled
    // config file. Alfalfa's portfolio seeds them and ad hoc ones are added by
    // hand; the store holds the union, because sync is additive and never
    // deletes. Reading the bundled catalog here is why Ctrl+Option+Space only
    // ever offered Dock and Studio.
    //
    // A missing or unreadable store falls back to the bundled catalog rather
    // than failing to launch: an unopenable switcher is worse than a short one.
    let catalog = match desktop_catalog(&base) {
        Ok(catalog) => catalog,
        Err(_) => base,
    };
    if catalog.projects.is_empty() {
        return Err("Catalog needs at least one project".into());
    }
    if !args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--conversation" | "--diagnostics" | "--connect-project" | "--stream-host"
        )
    }) {
        resource_palette::run(catalog)?;
        return Ok(());
    }
    let mut state = state_store().load(&catalog)?;
    let connect_project = args
        .windows(2)
        .find(|pair| pair[0] == "--connect-project")
        .map(|pair| pair[1].clone());
    if let Some(project) = &connect_project {
        catalog.project(project)?;
        state = state_store().update(&catalog, |s| {
            s.apply(Actor::Human, Action::SelectProject(project.clone()))
        })?;
    }
    let stream_host = args
        .windows(2)
        .find(|pair| pair[0] == "--stream-host")
        .map(|pair| pair[1].clone());
    if let Some(host) = &stream_host {
        host.parse::<std::net::IpAddr>()?;
    }
    let diagnostics = args.iter().any(|a| a == "--diagnostics");
    let model = args
        .windows(2)
        .find(|p| p[0] == "--model")
        .map(|p| p[1].clone())
        .unwrap_or_default();
    iced::application(
        move || {
            let mut app = App::new(catalog.clone(), state.clone());
            app.diagnostics = diagnostics;
            match summon::SummonHotkey::register() {
                Ok(h) => app.summon = Some(h),
                Err(e) => {
                    app.summon_error =
                        Some(format!("Command-Tab to Dock · Shortcut unavailable: {e}"))
                }
            }
            for panel in app.panels.values_mut() {
                panel.configure_model(model.clone());
            }
            let task = if let Some(host) = &stream_host {
                app.stream_host = host.clone();
                app.busy = true;
                Task::perform(async { reservation::Native.snapshot() }, Message::Displays)
            } else if !diagnostics || connect_project.is_some() {
                app.update(Message::Panel(
                    app.catalog.projects[app.selected].id.clone(),
                    project_panel::Message::Run(project_panel::Action::Recall),
                ))
            } else {
                Task::none()
            };
            (app, task)
        },
        App::update,
        App::view,
    )
    .subscription(|_| {
        iced::Subscription::batch([
            iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::Tick),
            iced::time::every(std::time::Duration::from_millis(100)).map(|_| Message::SummonTick),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Keyboard(event) => Some(Message::Keyboard(event)),
                _ => None,
            }),
        ])
    })
    .title("Dock")
    .theme(App::theme)
    .window_size(if diagnostics {
        (1180.0, 1050.0)
    } else {
        (560.0, 400.0)
    })
    .centered()
    .run()?;
    Ok(())
}

#[cfg(test)]
mod compact_tests {
    use super::*;
    fn key(named: iced::keyboard::key::Named) -> Message {
        use iced::keyboard::*;
        Message::Keyboard(Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }
    fn drain(app: &mut App, task: Task<Message>) {
        use iced::futures::StreamExt;
        let mut queue = vec![task];
        while let Some(task) = queue.pop() {
            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                while let Some(action) = iced::futures::executor::block_on(stream.next()) {
                    if let iced_runtime::Action::Output(message) = action {
                        queue.push(app.update(message));
                    }
                }
            }
        }
    }
    #[test]
    fn arrows_choose_project_without_remote_side_effects() {
        let catalog = Catalog::bundled().unwrap();
        let state = WorkspaceState::new(&catalog).unwrap();
        let mut app = App::new(catalog, state);
        let _ = app.update(key(iced::keyboard::key::Named::ArrowDown));
        assert_eq!(app.picker, 1);
        assert_eq!(app.selected, 0);
        let _ = app.update(key(iced::keyboard::key::Named::ArrowUp));
        assert_eq!(app.picker, 0);
    }
    #[test]
    #[ignore = "live Agora SSH and inference; explicit DOCK_LIVE_TEST=1 required"]
    fn compact_enter_reply_and_escape_use_same_live_session() {
        assert_eq!(std::env::var("DOCK_LIVE_TEST").as_deref(), Ok("1"));
        let catalog = Catalog::bundled().unwrap();
        let state = WorkspaceState::new(&catalog).unwrap();
        let mut app = App::new(catalog, state);
        app.diagnostics = true;
        let task = app.update(Message::Panel(
            "dock".into(),
            project_panel::Message::Run(project_panel::Action::Recall),
        ));
        drain(&mut app, task);
        assert!(
            app.panels["dock"].error.is_none(),
            "{:?}",
            app.panels["dock"].error
        );
        let id = app.panels["dock"].session_id.clone();
        let marker = std::env::var("DOCK_LIVE_REPLY")
            .unwrap_or_else(|_| format!("DOCK_COMPACT_{}", std::process::id()));
        app.diagnostics = false;
        let _ = app.update(Message::Line(format!(
            "Reply with exactly {marker}. Do not use tools or modify files."
        )));
        let task = app.update(key(iced::keyboard::key::Named::Enter));
        drain(&mut app, task);
        assert!(
            app.panels["dock"].error.is_none(),
            "{:?}",
            app.panels["dock"].error
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(100);
        while std::time::Instant::now() < deadline {
            let task = app.update(Message::Panel(
                "dock".into(),
                project_panel::Message::Run(project_panel::Action::Status),
            ));
            drain(&mut app, task);
            if app.panels["dock"].latest_response().contains(&marker)
                && app.panels["dock"].controller.as_deref() == Some("human")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        assert!(app.panels["dock"].latest_response().contains(&marker));
        assert_eq!(app.panels["dock"].controller.as_deref(), Some("human"));
        let task = app.update(key(iced::keyboard::key::Named::Escape));
        drain(&mut app, task);
        assert_eq!(app.panels["dock"].session_id, id);
        assert_eq!(app.panels["dock"].controller.as_deref(), Some("human"));
        let rejected =
            canvas_mac::remote::rpc("dock", "desktop-text", &["must not type".into()], None);
        assert!(rejected.is_err());
        println!(
            "Compact Enter received {marker}; same session {}; automatic completion takeover and Escape acknowledged; desktop input rejected.",
            id.unwrap()
        );
    }
}
