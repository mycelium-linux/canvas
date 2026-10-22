//! UI state caches server-owned conversations; it never invents a session ID.
use base64::Engine;
use canvas_mac::remote::rpc;
use iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, text, text_editor, text_input,
};
use iced::{ContentFit, Element, Length, Point, Task};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    SubmitTask,
    ViewDesktop,
    Recall,
    Status,
    Prompt,
    Resume,
    Abort,
    Capture,
    Human,
    Agent,
    Type,
    Key,
    Click,
}
#[derive(Debug, Clone)]
pub enum Message {
    Run(Action),
    Done(u64, Action, Result<Value, String>),
    PromptEdit(text_editor::Action),
    Model(String),
    Input(String),
    Key(String),
    Pointer(Point),
    Click,
    Poll,
}
pub struct Panel {
    pub session_id: Option<String>,
    pub line: String,
    pub queued_submit: bool,
    task_owned: bool,
    submitting: bool,
    cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task_baseline: Option<String>,
    pub error: Option<String>,
    pub controller: Option<String>,
    epoch: Option<u64>,
    pub busy: bool,
    pub recalling: bool,
    session: Value,
    project_context: Value,
    prompt: text_editor::Content,
    model: String,
    input: String,
    key: String,
    pointer: Option<Point>,
    screenshot: Option<image::Handle>,
    dimensions: (u32, u32),
    pub status: String,
    last_poll: std::time::Instant,
    generation: u64,
    submitted: Option<String>,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            session_id: None,
            line: String::new(),
            queued_submit: false,
            task_owned: false,
            submitting: false,
            cancellation: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_baseline: None,
            error: None,
            controller: None,
            epoch: None,
            busy: false,
            recalling: false,
            session: Value::Null,
            project_context: Value::Null,
            prompt: text_editor::Content::new(),
            model: String::new(),
            input: String::new(),
            key: "Return".into(),
            pointer: None,
            screenshot: None,
            dimensions: (0, 0),
            status: "Recall this project to open its persistent conversation and desktop.".into(),
            generation: 0,
            submitted: None,
            last_poll: std::time::Instant::now(),
        }
    }
}
impl Panel {
    pub fn update(&mut self, project: &str, message: Message) -> Task<Message> {
        match message {
            Message::PromptEdit(action) => self.prompt.perform(action),
            Message::Model(value) => self.model = value,
            Message::Input(value) => self.input = value,
            Message::Key(value) => self.key = value,
            Message::Pointer(p) => self.pointer = Some(p),
            Message::Click => return self.update(project, Message::Run(Action::Click)),
            Message::Poll => {
                if self.session_id.is_some()
                    && !self.busy
                    && self.last_poll.elapsed() > std::time::Duration::from_secs(3)
                {
                    return self.update(project, Message::Run(Action::Status));
                }
            }
            Message::Run(action) => {
                if action == Action::Human {
                    self.queued_submit = false;
                    self.cancellation
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if action == Action::SubmitTask && !self.busy {
                    self.cancellation =
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                }
                let cancellation = self.cancellation.clone();
                // Human takeover can interrupt an outstanding prompt/status request.
                if self.busy && !matches!(action, Action::Human | Action::Abort) {
                    if action == Action::SubmitTask && !self.line.trim().is_empty() {
                        self.queued_submit = true;
                    }
                    return Task::none();
                }
                if matches!(
                    action,
                    Action::Prompt | Action::Resume | Action::Type | Action::Key | Action::Click
                ) && self.controller.as_deref() != Some("agent")
                {
                    self.status =
                        "Allow agent control before sending a prompt or desktop input.".into();
                    return Task::none();
                }
                let project = project.to_string();
                let prompt = if action == Action::SubmitTask {
                    self.line.clone()
                } else {
                    self.prompt.text()
                };
                if action == Action::SubmitTask {
                    if prompt.trim().is_empty() {
                        return Task::none();
                    }
                    self.task_baseline = self.latest_assistant_id();
                    self.submitting = true;
                }
                let model = self.model.trim().to_string();
                let input = self.input.clone();
                let key = self.key.clone();
                let known = self.session_id.clone();
                let epoch = self.epoch;
                let coords = self.pointer.and_then(|p| map_pointer(p, self.dimensions));
                if action == Action::Click && coords.is_none() {
                    self.status = "Click inside the desktop image after capturing it.".into();
                    return Task::none();
                }
                if matches!(action, Action::Prompt | Action::Resume | Action::SubmitTask) {
                    self.submitted = Some(prompt.clone());
                }
                self.generation += 1;
                let generation = self.generation;
                self.busy = true;
                if action == Action::Recall {
                    self.recalling = true;
                }
                self.last_poll = std::time::Instant::now();
                return Task::perform(
                    async move {
                        match action {
                            Action::ViewDesktop => Ok(
                                json!({"viewer":canvas_mac::remote::open_desktop(&project)?,"notice":"Project desktop viewer opened over the private SSH tunnel."}),
                            ),
                            Action::Recall => {
                                // Independent owner read runs alongside desktop recall,
                                // avoiding another serial network wait before showing it.
                                let context_project = project.clone();
                                let context = std::thread::spawn(move || {
                                    canvas_mac::alfalfa::recall(&context_project)
                                });
                                let recalled = rpc(&project, "desktop-recall", &[], None)?;
                                let ensured = rpc(&project, "session-ensure", &[], None)?;
                                let id = ensured["session_id"]
                                    .as_str()
                                    .ok_or("Session ensure did not return a session ID")?;
                                let attached = rpc(&project, "session-attach", &[], None)?;
                                rpc(&project, "desktop-browser", &[], None)?;
                                if attached
                                    .get("session_id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|got| got != id)
                                {
                                    return Err(
                                        "Visible terminal attached a different conversation".into(),
                                    );
                                }
                                let session = rpc(&project, "session-status", &[], None)?;
                                if session["session_id"] != id {
                                    return Err(
                                        "Session status returned a different conversation".into()
                                    );
                                }
                                let control = rpc(&project, "desktop-status", &[], None)?;
                                Ok(
                                    json!({"session":session,"desktop":control,"recalled":recalled,"viewer":canvas_mac::remote::open_desktop(&project)?,"alfalfa":context.join().map_err(|_| "Alfalfa context reader failed")?}),
                                )
                            }
                            Action::Status => Ok(
                                json!({"session":rpc(&project,"session-status",&[],None)?,"desktop":rpc(&project,"desktop-status",&[],None)?}),
                            ),
                            Action::SubmitTask => {
                                let known = known.ok_or("Project is still opening")?;
                                if cancellation.load(std::sync::atomic::Ordering::SeqCst) {
                                    return Err("Task cancelled".into());
                                }
                                let epoch = epoch.ok_or("Project control is still opening")?;
                                let control = rpc(
                                    &project,
                                    "desktop-control",
                                    &["agent".into(), epoch.to_string()],
                                    None,
                                )?;
                                let result = (|| {
                                    if cancellation.load(std::sync::atomic::Ordering::SeqCst) {
                                        return Err("Task cancelled".into());
                                    }
                                    let args = if model.is_empty() {
                                        vec![]
                                    } else {
                                        vec![model]
                                    };
                                    let submitted =
                                        rpc(&project, "session-prompt", &args, Some(prompt))?;
                                    if submitted["session_id"] != known
                                        || submitted["accepted"] != true
                                    {
                                        return Err(
                                            "The project did not acknowledge this task".into()
                                        );
                                    }
                                    if cancellation.load(std::sync::atomic::Ordering::SeqCst) {
                                        return Err("Task cancelled".into());
                                    }
                                    Ok(json!({"desktop":control,"notice":"Working…"}))
                                })();
                                match result {
                                    Ok(value) => Ok(value),
                                    Err(error) => {
                                        match rpc(
                                            &project,
                                            "desktop-control",
                                            &["human".into()],
                                            None,
                                        ) {
                                            Ok(_) => Err(error),
                                            Err(cleanup) => Err(format!(
                                                "{error}; could not return control: {cleanup}"
                                            )),
                                        }
                                    }
                                }
                            }
                            Action::Prompt | Action::Resume => {
                                if action == Action::Prompt && prompt.trim().is_empty() {
                                    return Err("Write a prompt first".into());
                                }
                                let args = if model.is_empty() {
                                    vec![]
                                } else {
                                    vec![model]
                                };
                                let result = rpc(
                                    &project,
                                    if action == Action::Prompt {
                                        "session-prompt"
                                    } else {
                                        "session-resume"
                                    },
                                    &args,
                                    Some(prompt),
                                )?;
                                if result["session_id"].as_str() != known.as_deref()
                                    || result["accepted"] != true
                                {
                                    return Err(
                                        "Prompt acknowledgement did not match this conversation"
                                            .into(),
                                    );
                                }
                                Ok(
                                    json!({"session":rpc(&project,"session-status",&[],None)?,"notice":"Prompt accepted by the existing conversation."}),
                                )
                            }
                            Action::Abort => {
                                let result = rpc(&project, "session-abort", &[], None)?;
                                if result["session_id"].as_str() != known.as_deref()
                                    || result["aborted"] != true
                                {
                                    return Err(
                                        "Interrupt acknowledgement did not match this conversation"
                                            .into(),
                                    );
                                }
                                Ok(
                                    json!({"session":rpc(&project,"session-status",&[],None)?,"notice":"Conversation interrupted. Its context is preserved."}),
                                )
                            }
                            Action::Human | Action::Agent => {
                                let result = rpc(
                                    &project,
                                    "desktop-control",
                                    &[if action == Action::Human {
                                        "human".into()
                                    } else {
                                        "agent".into()
                                    }],
                                    None,
                                )?;
                                Ok(
                                    json!({"desktop":result,"notice":if action==Action::Human{"Human takeover acknowledged on Agora One."}else{"Agent control enabled on Agora One."}}),
                                )
                            }
                            Action::Capture => {
                                Ok(json!({"capture":rpc(&project,"desktop-capture",&[],None)?}))
                            }
                            Action::Type | Action::Key | Action::Click => {
                                match action {
                                    Action::Type => {
                                        if input.is_empty() {
                                            return Err("Enter text to type first".into());
                                        }
                                        rpc(&project, "desktop-text", &[input], None)?;
                                    }
                                    Action::Key => {
                                        rpc(&project, "desktop-key", &[key], None)?;
                                    }
                                    _ => {
                                        let (x, y) = coords.unwrap();
                                        rpc(
                                            &project,
                                            "desktop-click",
                                            &[x.to_string(), y.to_string(), "left".into()],
                                            None,
                                        )?;
                                    }
                                }
                                Ok(
                                    json!({"capture":rpc(&project,"desktop-capture",&[],None)?,"notice":"Desktop input acknowledged."}),
                                )
                            }
                        }
                    },
                    move |result| Message::Done(generation, action, result),
                );
            }
            Message::Done(generation, action, result) => {
                if action == Action::Recall {
                    self.recalling = false;
                }
                if generation != self.generation {
                    return Task::none();
                }
                self.busy = false;
                if matches!(action, Action::SubmitTask | Action::Human) {
                    self.submitting = false;
                }
                match result {
                    Err(error) => {
                        self.error = Some(error.clone());
                        if matches!(action, Action::Human | Action::Agent) {
                            self.controller = None;
                        }
                        self.status = error;
                    }
                    Ok(value) => {
                        self.error = None;
                        if let Some(context) = value.get("alfalfa") {
                            self.project_context = context.clone();
                        }
                        if action == Action::SubmitTask {
                            self.task_owned = true;
                            if self.submitted.as_deref() == Some(self.line.as_str()) {
                                self.line.clear();
                            }
                        }
                        if action == Action::Human {
                            self.task_owned = false;
                        }
                        if matches!(action, Action::Prompt | Action::Resume | Action::SubmitTask)
                            && self.submitted.as_deref() == Some(self.prompt.text().as_str())
                        {
                            self.prompt = text_editor::Content::new();
                            self.submitted = None;
                        }
                        if let Some(session) = value.get("session") {
                            if let Some(id) = session["session_id"].as_str() {
                                if self.session_id.as_deref().is_some_and(|known| known != id) {
                                    self.status="Backend returned a different conversation; no local session was replaced.".into();
                                    return Task::none();
                                }
                                self.session_id = Some(id.into());
                                self.session = session.clone();
                            }
                        }
                        if let Some(desktop) = value.get("desktop") {
                            self.controller = controller(desktop);
                            self.epoch = desktop
                                .pointer("/ownership/epoch")
                                .or_else(|| desktop.get("epoch"))
                                .and_then(Value::as_u64);
                        }
                        if matches!(action, Action::Human | Action::Agent) {
                            let wanted = if action == Action::Human {
                                "human"
                            } else {
                                "agent"
                            };
                            if self.controller.as_deref() != Some(wanted) {
                                self.status="Remote control acknowledgement did not match the requested owner.".into();
                                return Task::none();
                            }
                        }
                        if let Some(capture) = value.get("capture") {
                            match decode_capture(capture) {
                                Ok((handle, size)) => {
                                    self.screenshot = Some(handle);
                                    self.dimensions = size;
                                    self.pointer = None
                                }
                                Err(error) => {
                                    self.status = error;
                                    return Task::none();
                                }
                            }
                        }
                        self.status = value["notice"]
                            .as_str()
                            .unwrap_or(if action == Action::Recall {
                                "Project recalled; the terminal and Dock share this conversation."
                            } else if action == Action::Capture {
                                "Desktop captured from Agora One."
                            } else {
                                "Session refreshed."
                            })
                            .into();
                        if self.queued_submit && matches!(action, Action::Recall | Action::Status) {
                            self.queued_submit = false;
                            return self.update(project, Message::Run(Action::SubmitTask));
                        }
                        if self.task_owned && self.task_finished() {
                            return self.update(project, Message::Run(Action::Human));
                        }
                    }
                }
            }
        }
        Task::none()
    }
    pub fn owns_task(&self) -> bool {
        self.task_owned
    }
    pub fn configure_model(&mut self, model: String) {
        self.model = model;
    }
    fn latest_assistant_id(&self) -> Option<String> {
        self.session["messages"]
            .as_array()?
            .iter()
            .rev()
            .find(|m| m["info"]["role"] == "assistant")
            .and_then(|m| m["info"]["id"].as_str())
            .map(str::to_string)
    }
    fn task_finished(&self) -> bool {
        self.session["status"]["type"] == "idle"
            && self.latest_assistant_id() != self.task_baseline
            && self.session["messages"].as_array().is_some_and(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|m| m["info"]["role"] == "assistant")
                    .is_some_and(|m| !m["info"]["time"]["completed"].is_null())
            })
    }
    pub fn latest_response(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if let Some(messages) = self.session["messages"].as_array() {
            for message in messages.iter().rev() {
                if message["info"]["role"] != "assistant" {
                    continue;
                }
                let parts = message["parts"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter(|p| p["type"] == "text")
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !parts.trim().is_empty() {
                    return parts;
                }
            }
        }
        if self.recalling {
            "Opening your project…".into()
        } else {
            "Your desktop is ready. What would you like to work on?".into()
        }
    }
    pub fn progress(&self) -> &str {
        if self.submitting {
            "Sending…"
        } else if self.recalling {
            "Opening project…"
        } else if self.task_owned || self.session["status"]["type"] == "busy" {
            "Working…"
        } else if self.error.is_some() {
            "Couldn’t finish that action"
        } else {
            "Ready"
        }
    }
    pub fn view(&self) -> Element<'_, Message> {
        let idle = !self.busy;
        let controlled = self.controller.as_deref() == Some("agent");
        let mut content = column![row![
            text("Project conversation").size(22),
            button(if self.session_id.is_some() {
                "Recall project"
            } else {
                "Open project session"
            })
            .on_press_maybe(idle.then_some(Message::Run(Action::Recall))),
            button("Refresh").on_press_maybe(idle.then_some(Message::Run(Action::Status)))
        ]
        .spacing(8),]
        .spacing(10);
        if let Some(state) = self.project_context["state"].as_str() {
            let detail = if state == "ready" {
                let resources = self
                    .project_context
                    .pointer("/context/definition/resource")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                format!("Alfalfa context attached · {resources} project resources")
            } else {
                "Alfalfa context unavailable for this project".into()
            };
            content = content.push(text(detail).size(12));
        }
        if let Some(id) = &self.session_id {
            let status = self.session["status"]["type"].as_str().unwrap_or("unknown");
            content = content.push(text(format!("OpenCode · {status} · {id}")).size(12));
            let mut transcript = column![].spacing(10).width(Length::Fill);
            if let Some(messages) = self.session["messages"].as_array() {
                for message in messages {
                    let role = message["info"]["role"].as_str().unwrap_or("session");
                    let mut parts = Vec::new();
                    if let Some(array) = message["parts"].as_array() {
                        for part in array {
                            match part["type"].as_str() {
                                Some("text") => {
                                    if let Some(t) = part["text"].as_str() {
                                        parts.push(t.to_string())
                                    }
                                }
                                Some("tool") => parts.push(format!(
                                    "Tool {}: {}",
                                    part["tool"].as_str().unwrap_or("call"),
                                    part["state"]["status"].as_str().unwrap_or("pending")
                                )),
                                _ => {}
                            }
                        }
                    }
                    if !parts.is_empty() {
                        transcript = transcript.push(
                            column![
                                text(role).size(12),
                                text(parts.join("\n")).size(14).width(Length::Fill)
                            ]
                            .spacing(4),
                        );
                    }
                }
            }
            content = content.push(
                container(scrollable(transcript).width(Length::Fill).height(180))
                    .width(Length::Fill)
                    .padding(12)
                    .style(container::rounded_box),
            );
        }
        content = content
            .push(
                row![
                    text(match self.controller.as_deref() {
                        Some("human") => "You have control on Agora One",
                        Some("agent") => "Agent control is enabled on Agora One",
                        _ => "Remote control has not been checked",
                    })
                    .width(Length::Fill),
                    button("Take control").on_press(Message::Run(Action::Human)),
                    button("Allow agent")
                        .on_press_maybe(idle.then_some(Message::Run(Action::Agent)))
                ]
                .spacing(8),
            )
            .push(
                text_editor(&self.prompt)
                    .placeholder("Prompt this project's existing conversation…")
                    .height(90)
                    .on_action(Message::PromptEdit),
            )
            .push(
                text_input(
                    "Optional provider/model (uses server default when empty)",
                    &self.model,
                )
                .on_input(Message::Model),
            )
            .push(
                row![
                    button("Send prompt").on_press_maybe(
                        (idle && controlled && self.session_id.is_some())
                            .then_some(Message::Run(Action::Prompt))
                    ),
                    button("Interrupt").on_press_maybe(
                        self.session_id
                            .is_some()
                            .then_some(Message::Run(Action::Abort))
                    ),
                    button("Resume conversation").on_press_maybe(
                        (idle && controlled && self.session_id.is_some())
                            .then_some(Message::Run(Action::Resume))
                    )
                ]
                .spacing(8),
            )
            .push(
                row![
                    text("Project desktop").size(22),
                    button("Open desktop")
                        .on_press_maybe(idle.then_some(Message::Run(Action::ViewDesktop))),
                    button("Capture desktop")
                        .on_press_maybe(idle.then_some(Message::Run(Action::Capture)))
                ]
                .spacing(12),
            );
        if let Some(handle) = &self.screenshot {
            content = content.push(
                mouse_area(
                    image(handle.clone())
                        .width(640)
                        .height(360)
                        .content_fit(ContentFit::Contain),
                )
                .on_move(Message::Pointer)
                .on_press(Message::Click),
            );
            content=content.push(text("Click the captured desktop to send a click at that position. Capture again to check the result.").size(12));
        }
        content = content
            .push(
                row![
                    text_input("Text to type on the remote desktop", &self.input)
                        .on_input(Message::Input),
                    button("Type")
                        .on_press_maybe((idle && controlled).then_some(Message::Run(Action::Type)))
                ]
                .spacing(8),
            )
            .push(
                row![
                    text_input("Key, e.g. Return, Escape, Tab", &self.key).on_input(Message::Key),
                    button("Send key")
                        .on_press_maybe((idle && controlled).then_some(Message::Run(Action::Key)))
                ]
                .spacing(8),
            )
            .push(text(&self.status).size(12));
        content.width(Length::Fill).into()
    }
}
fn controller(value: &Value) -> Option<String> {
    [
        value.get("mode"),
        value.pointer("/ownership/mode"),
        value.pointer("/control/mode"),
        value.get("owner"),
        value.get("controller"),
        value.pointer("/control/owner"),
        value.pointer("/control/controller"),
        value.get("control"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|v| matches!(*v, "human" | "agent"))
    .map(str::to_string)
}
fn decode_capture(value: &Value) -> Result<(image::Handle, (u32, u32)), String> {
    let encoded = value["png_base64"]
        .as_str()
        .ok_or("Desktop capture did not include PNG data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("Invalid desktop PNG".into());
    }
    let size = (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    );
    if size.0 == 0
        || size.1 == 0
        || value["width"].as_u64() != Some(u64::from(size.0))
        || value["height"].as_u64() != Some(u64::from(size.1))
    {
        return Err("Desktop capture geometry does not match its PNG".into());
    }
    if size.0 > 8192 || size.1 > 8192 {
        return Err("Desktop capture dimensions exceed the supported limit".into());
    }
    ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png)
        .map_err(|e| format!("Desktop PNG could not be decoded: {e}"))?;
    Ok((image::Handle::from_bytes(bytes), size))
}
fn map_pointer(point: Point, (width, height): (u32, u32)) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let scale = (640.0 / width as f32).min(360.0 / height as f32);
    let x = (point.x - (640.0 - width as f32 * scale) / 2.0) / scale;
    let y = (point.y - (360.0 - height as f32 * scale) / 2.0) / scale;
    (x >= 0.0 && y >= 0.0 && x < width as f32 && y < height as f32).then_some((x as u32, y as u32))
}
#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn task_completion_requires_new_completed_assistant() {
        let mut panel = super::Panel::default();
        panel.task_baseline = Some("old".into());
        panel.session = serde_json::json!({"status":{"type":"idle"},"messages":[{"info":{"id":"old","role":"assistant","time":{"completed":1}}}]});
        assert!(!panel.task_finished());
        panel.session["messages"][0]["info"]["id"] = serde_json::json!("new");
        panel.session["status"]["type"] = serde_json::json!("busy");
        assert!(!panel.task_finished());
        panel.session["status"]["type"] = serde_json::json!("idle");
        assert!(panel.task_finished());
    }

    use super::*;
    #[test]
    fn pointer_mapping_respects_letterboxing() {
        assert_eq!(
            map_pointer(Point::new(320.0, 180.0), (1920, 1080)),
            Some((960, 540))
        );
        assert_eq!(map_pointer(Point::new(0.0, 180.0), (1000, 1000)), None);
    }
    #[test]
    fn capture_rejects_inconsistent_geometry() {
        assert!(decode_capture(&json!({"png_base64":"AAAA","width":2,"height":2})).is_err());
    }
    #[test]
    fn stale_status_cannot_reenable_control_after_takeover() {
        let mut panel = Panel {
            generation: 2,
            controller: Some("human".into()),
            ..Panel::default()
        };
        let _ = panel.update(
            "dock",
            Message::Done(
                1,
                Action::Status,
                Ok(json!({"desktop":{"ownership":{"mode":"agent"}}})),
            ),
        );
        assert_eq!(panel.controller.as_deref(), Some("human"));
    }
    #[test]
    fn returned_conversation_cannot_replace_pinned_session() {
        let mut panel = Panel {
            session_id: Some("original".into()),
            ..Panel::default()
        };
        let _ = panel.update(
            "dock",
            Message::Done(
                0,
                Action::Status,
                Ok(json!({"session":{"session_id":"replacement"}})),
            ),
        );
        assert_eq!(panel.session_id.as_deref(), Some("original"));
        assert!(panel.status.contains("different conversation"));
    }
    #[test]
    fn ownership_uses_real_backend_shapes() {
        assert_eq!(
            controller(&json!({"ownership":{"mode":"agent"}})).as_deref(),
            Some("agent")
        );
        assert_eq!(
            controller(&json!({"control":{"mode":"human"},"mode":"human"})).as_deref(),
            Some("human")
        );
    }
    #[test]
    fn superseded_recall_releases_global_navigation_guard() {
        let mut panel = Panel {
            generation: 2,
            recalling: true,
            controller: Some("human".into()),
            ..Panel::default()
        };
        let _ = panel.update(
            "dock",
            Message::Done(1, Action::Recall, Ok(json!({"desktop":{"mode":"agent"}}))),
        );
        assert!(!panel.recalling);
        assert_eq!(panel.controller.as_deref(), Some("human"));
    }
}
