//! Target-bound desktop choices. Rust owns discovery/actions; Lua only paints.
use crate::control::{
    CONTROL_VERSION, Command, ControlReply, ControlRequest, Outcome, Receipt, RuntimeSnapshot,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};
pub const LEASE_MS: u64 = 10_000;
pub const MAX_ENTRIES: usize = 512;
/// Frameless instruments are desktop content, not app-switching destinations.
pub fn dock_surfaces(document: &crate::WorkspaceDocument) -> impl Iterator<Item = &crate::Surface> {
    document.surfaces.iter().filter(|surface| {
        !matches!(&surface.content, crate::SurfaceContent::Widget { spec }
            if spec.chrome == canvas_widgets::WidgetChrome::Frameless)
    })
}
fn generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Choice {
    /// A composed Lua UI that can be opened as a surface in this workspace.
    ///
    /// This is how a Spore's UI is launched before the spore host exists: the
    /// package's Lua is already composed for Canvas, so launching it needs no
    /// interpreter of its own. The module name is the whole reference -- a path
    /// would let a launcher entry steer the host at arbitrary files.
    Spore {
        module: String,
    },
    /// A live terminal on a real pty, which is what the Terminal app is.
    Terminal { shell: String },
    Workspace { id: String },
    Surface { id: String },
    Application { path: PathBuf },
}

/// Display filtering only. The owner still validates the complete catalog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Workspaces,
    Canvas,
    Native,
}
impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Workspaces => "workspaces",
            Self::Canvas => "canvas",
            Self::Native => "native",
        }
    }
    fn includes(self, choice: &Choice) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, choice),
                (Self::Workspaces, Choice::Workspace { .. })
                    | (
                        Self::Canvas,
                        Choice::Surface { .. } | Choice::Spore { .. } | Choice::Terminal { .. }
                    )
                    | (Self::Native, Choice::Application { .. })
            )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub available: bool,
    pub choice: Choice,
    /// What the row shows at a glance. A launcher that only has text makes every
    /// app look identical, which is the difference between a list and a launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Icon>,
}

/// The small mark beside a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Icon {
    /// One or two characters. A monogram is honest: it needs no asset, cannot
    /// ship malware, and reads at 20 px where a bitmap would not.
    pub glyph: String,
    /// RGB, from the Spore's own manifest. A host that invents colours makes two
    /// versions of one app look unrelated.
    pub tint: [u8; 3],
}

impl Entry {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        detail: impl Into<String>,
        available: bool,
        choice: Choice,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: detail.into(),
            available,
            choice,
            icon: None,
        }
    }

    /// Attach a monogram and tint, taken from the app's own manifest.
    pub fn badged(mut self, glyph: impl Into<String>, tint: [u8; 3]) -> Self {
        let glyph = glyph.into();
        if !glyph.is_empty() && glyph.len() <= 2 && !glyph.chars().any(char::is_control) {
            self.icon = Some(Icon { glyph, tint });
        }
        self
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub revision: String,
    pub entries: Vec<Entry>,
}
impl Catalog {
    pub fn collect(snapshot: &RuntimeSnapshot) -> Self {
        let mut entries = Vec::new();
        for s in &snapshot.surfaces {
            entries.push(Entry::new(
                format!("surface:{}", s.id),
                s.title.clone(),
                if snapshot.minimized_surface_ids.contains(&s.id) {
                    "WINDOW · minimized"
                } else {
                    "WINDOW · open"
                },
                s.content != crate::control::ContentKind::Empty,
                Choice::Surface { id: s.id.clone() },
            )
            .badged("▢", [126, 138, 156]));
        }
        for id in &snapshot.workspaces {
            let summary = snapshot
                .workspace_summaries
                .iter()
                .find(|s| &s.project_id == id);
            entries.push(Entry::new(
                format!("workspace:{id}"),
                summary.map(|s| s.name.clone()).unwrap_or(id.clone()),
                if id == &snapshot.project_id {
                    "WORKSPACE · current"
                } else {
                    "WORKSPACE"
                },
                true,
                Choice::Workspace { id: id.clone() },
            )
            .badged(
                summary
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| id.clone())
                    .chars()
                    .next()
                    .map(|first| first.to_uppercase().to_string())
                    .unwrap_or_else(|| "W".into()),
                [140, 150, 176],
            ));
        }
        entries.extend(applications());
        entries.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        entries.truncate(MAX_ENTRIES);
        let revision = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&snapshot.project_id, &entries)).expect("catalog serializes")
            )
        );
        Self { revision, entries }
    }
}
pub fn applications() -> Vec<Entry> {
    let mut roots = Vec::new();
    #[cfg(target_os = "macos")]
    {
        roots.extend([
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            PathBuf::from("/Applications/Utilities"),
            PathBuf::from("/System/Applications/Utilities"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        roots.extend([
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/local/share/applications"),
        ]);
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        if let Some(data) = data {
            roots.insert(0, data.join("applications"));
        }
    }
    let mut seen = BTreeSet::new();
    let mut apps = Vec::new();
    for root in roots {
        let Ok(dir) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut paths: Vec<_> = dir.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths.into_iter().take(MAX_ENTRIES) {
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            #[cfg(target_os = "macos")]
            let label = if name.ends_with(".app") && path.is_dir() {
                Some(name.trim_end_matches(".app").to_string())
            } else {
                None
            };
            #[cfg(target_os = "linux")]
            let label = if name.ends_with(".desktop") {
                desktop_label(&path)
            } else {
                None
            };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let label: Option<String> = None;
            if let Some(label) = label
                && !label.is_empty()
                && label.len() <= 128
                && !label.chars().any(char::is_control)
                && seen.insert(name.to_string())
            {
                apps.push(Entry::new(
                    format!("app:{}", path.display()),
                    label,
                    "APPLICATION · native",
                    true,
                    Choice::Application { path },
                ));
            }
        }
    }
    apps
}
#[cfg(target_os = "linux")]
fn desktop_label(path: &std::path::Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > 64 * 1024 {
        return None;
    }
    let source = std::fs::read_to_string(path).ok()?;
    let mut section = false;
    let mut name = None;
    let mut application = false;
    for line in source.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line == "[Desktop Entry]";
            continue;
        }
        if !section {
            continue;
        }
        if line == "Hidden=true" || line == "NoDisplay=true" {
            return None;
        }
        if line == "Type=Application" {
            application = true;
        }
        if let Some(value) = line.strip_prefix("Name=") {
            name = Some(value.to_owned());
        }
    }
    application.then_some(name).flatten()
}
pub fn launch(path: &std::path::Path) -> Result<(), String> {
    // Only a fresh catalog-owned path can reach this function. No shell/Exec parser.
    #[cfg(target_os = "macos")]
    let child = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(path)
        .spawn();
    #[cfg(target_os = "linux")]
    let child = std::process::Command::new("gio")
        .arg("launch")
        .arg(path)
        .spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err("native application launch is unsupported on this platform".into());
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut child = child.map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("native application owner rejected launch".into())
                };
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("native application owner timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub mycelium_peer: Option<String>,
    #[serde(default)]
    pub unix_user: Option<String>,
}
pub fn targets() -> Result<Vec<Target>, String> {
    let mut targets = vec![Target {
        id: "local".into(),
        label: "This screen".into(),
        mycelium_peer: None,
        unix_user: None,
    }];
    if let Some(bytes) =
        crate::runtime_config::read(std::path::Path::new("shell/targets.json"), 16 * 1024)
            .map_err(|e| e.to_string())?
    {
        let remote: Vec<Target> =
            serde_json::from_slice(&bytes).map_err(|e| format!("launcher targets: {e}"))?;
        let mut seen = BTreeSet::from(["local".to_string()]);
        for target in remote {
            let slug = |s: &str| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            };
            if targets.len() >= 8
                || !slug(&target.id)
                || !seen.insert(target.id.clone())
                || target.label.is_empty()
                || target.label.len() > 64
                || target.label.chars().any(char::is_control)
                || !target.mycelium_peer.as_deref().is_some_and(slug)
                || target.unix_user.as_deref().is_some_and(|s| !slug(s))
            {
                return Err("invalid explicit launcher target".into());
            }
            targets.push(target);
        }
    }
    Ok(targets)
}
/// The same captured target is used for catalog reads and invocation. Never
/// fall back to local after a remote error. UI callers run this off-thread.
pub fn request(target: &Target, command: Command) -> Result<Receipt, String> {
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!(
            "launcher-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        expected_revision: None,
        actor: None,
        command,
    };
    let reply = if let Some(peer) = &target.mycelium_peer {
        remote(peer, target.unix_user.as_deref(), &request)?
    } else {
        crate::control::send(&request)?
    };
    match reply.outcome {
        Outcome::Ok { receipt } => Ok(receipt),
        Outcome::Error { message, .. } => Err(message),
    }
}
fn remote(
    peer: &str,
    unix_user: Option<&str>,
    request: &ControlRequest,
) -> Result<ControlReply, String> {
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut process = Command::new("mycelium");
    process.args(["ssh", peer]);
    if let Some(user) = unix_user {
        process.args(["--user", user]);
    }
    let mut child = process
        .args(["--", ".local/bin/canvasctl"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().ok_or("remote stdin unavailable")?;
    if let Err(error) = serde_json::to_writer(&mut input, request)
        .and_then(|_| input.write_all(b"\n").map_err(serde_json::Error::io))
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.to_string());
    }
    drop(input);
    let stdout = child.stdout.take().ok_or("remote stdout unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(crate::control::MAX_CONTROL_MESSAGE + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(6);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("target disconnected or catalog timed out".into());
            }
        }
    };
    if !status.success() {
        return Err("target unavailable; remote Canvas/SSH failed".into());
    }
    let bytes = reader
        .join()
        .map_err(|_| "remote reply worker failed")?
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > crate::control::MAX_CONTROL_MESSAGE {
        return Err("remote reply exceeds limit".into());
    }
    let reply: ControlReply =
        serde_json::from_slice(&bytes).map_err(|e| format!("remote reply: {e}"))?;
    if reply.request_id != request.request_id || reply.schema_version != CONTROL_VERSION {
        return Err("target reply provenance mismatch".into());
    }
    Ok(reply)
}

#[derive(Debug, Clone)]
pub struct Launcher {
    pub viewport: [f32; 2],
    pub window_cycle: bool,
    pub scope: Scope,
    /// Local document's application surfaces. Never use these IDs on a remote target.
    pub canvas_surface_ids: Option<BTreeSet<String>>,
    pub targets: Vec<Target>,
    pub target: usize,
    pub generation: u64,
    pub query: String,
    pub selected: usize,
    pub catalog: Option<Catalog>,
    pub observed: u64,
    pub status: String,
    pub busy: bool,
    pub loading: bool,
    #[cfg(feature = "lua-scenes")]
    pub scene: Option<crate::Scene>,
    #[cfg(feature = "lua-scenes")]
    pub buttons: Vec<crate::lua_module::ModuleButton>,
    #[cfg(feature = "lua-scenes")]
    presentation: crate::shell_presentation::Presentation,
}
impl Launcher {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            viewport: [600.0, 440.0],
            window_cycle: false,
            scope: Scope::All,
            canvas_surface_ids: None,
            targets: targets()?,
            target: 0,
            generation: generation(),
            query: String::new(),
            selected: 0,
            catalog: None,
            observed: 0,
            status: "Loading live catalog…".into(),
            busy: false,
            loading: true,
            #[cfg(feature = "lua-scenes")]
            scene: None,
            #[cfg(feature = "lua-scenes")]
            buttons: Vec::new(),
            #[cfg(feature = "lua-scenes")]
            presentation: Default::default(),
        })
    }
    pub fn matches(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        let mut rows: Vec<_> = self
            .catalog
            .iter()
            .flat_map(|c| &c.entries)
            .filter(|e| self.scope.includes(&e.choice))
            .filter(|e| {
                if self.scope == Scope::Canvas
                    && self.targets[self.target].id == "local"
                    && let (Some(ids), Choice::Surface { id }) =
                        (&self.canvas_surface_ids, &e.choice)
                {
                    return ids.contains(id);
                }
                true
            })
            .filter(|e| {
                format!("{} {}", e.title, e.detail)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        rows.sort_by_key(|e| !e.title.to_lowercase().starts_with(&query));
        rows
    }
    /// Change views without changing the owner catalog or its revision.
    pub fn set_scope(&mut self, scope: Scope) {
        if self.loading || self.busy || self.window_cycle {
            return;
        }
        self.scope = scope;
        self.query.clear();
        self.selected = 0;
        self.generation = generation();
    }
    pub fn visible_rows(&self) -> usize {
        if self.window_cycle {
            (((self.viewport[0] - 32.0) / 112.0).floor() as usize).clamp(1, 6)
        } else {
            6
        }
    }
    pub fn row_start(&self) -> usize {
        self.selected.saturating_sub(self.visible_rows() - 1)
    }
    pub fn accept(&mut self, generation: u64, result: Result<Catalog, String>, now: u64) {
        if generation != self.generation {
            return;
        }
        let selected = self.matches().get(self.selected).map(|e| e.id.clone());
        self.observed = now;
        self.busy = false;
        self.loading = false;
        self.selected = 0;
        match result {
            Ok(c) => {
                self.catalog = Some(c);
                self.selected = selected
                    .and_then(|id| self.matches().iter().position(|e| e.id == id))
                    .unwrap_or(0);
                self.status = "Live catalog · Enter to open".into();
            }
            Err(e) => {
                self.catalog = None;
                self.status = e;
            }
        }
    }
    pub fn action(&self, now: u64) -> Option<(Target, Command)> {
        if self.busy || now.saturating_sub(self.observed) >= LEASE_MS {
            return None;
        }
        let catalog = self.catalog.as_ref()?;
        let entry = *self.matches().get(self.selected)?;
        entry.available.then(|| {
            (
                self.targets[self.target].clone(),
                Command::ShellActivate {
                    catalog_revision: catalog.revision.clone(),
                    entry_id: entry.id.clone(),
                },
            )
        })
    }
    /// Pointer choices carry the same provenance as keyboard activation.
    pub fn select_entry(&mut self, generation: u64, revision: &str, id: &str, now: u64) -> bool {
        if self.generation != generation
            || self.busy
            || now.saturating_sub(self.observed) >= LEASE_MS
            || self.catalog.as_ref().is_none_or(|c| c.revision != revision)
        {
            return false;
        }
        let Some(index) = self
            .matches()
            .iter()
            .position(|e| e.id == id && e.available)
        else {
            return false;
        };
        self.selected = index;
        true
    }
    pub fn refresh_due(&mut self, now: u64) -> Option<(Target, u64)> {
        if self.loading || self.busy || now.saturating_sub(self.observed) < 6000 {
            return None;
        }
        self.generation = generation();
        self.loading = true;
        Some((self.targets[self.target].clone(), self.generation))
    }
    pub fn handle(&mut self, event: &iced::keyboard::Event) -> Input {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed {
            key,
            modifiers,
            text,
            ..
        } = event
        else {
            return Input::Consumed;
        };
        match key {
            Key::Named(Named::Escape) => Input::Cancel,
            Key::Named(Named::Enter) => Input::Apply,
            Key::Named(Named::F2)
                if self.scope != Scope::Workspaces
                    && !self.window_cycle
                    && !self.busy
                    && !self.loading =>
            {
                self.set_scope(if self.scope == Scope::Native {
                    Scope::Canvas
                } else {
                    Scope::Native
                });
                Input::Consumed
            }
            Key::Named(Named::Tab) => {
                self.target = (self.target + 1) % self.targets.len();
                self.generation = generation();
                self.busy = false;
                self.loading = true;
                self.catalog = None;
                self.observed = 0;
                self.selected = 0;
                self.status = "Loading target catalog…".into();
                Input::Reload
            }
            Key::Named(Named::F5) => {
                self.generation = generation();
                self.busy = false;
                self.loading = true;
                self.catalog = None;
                self.observed = 0;
                self.status = "Refreshing…".into();
                Input::Reload
            }
            Key::Named(Named::ArrowUp) => {
                self.selected = self.selected.saturating_sub(1);
                Input::Consumed
            }
            Key::Named(Named::ArrowDown) => {
                self.selected = (self.selected + 1).min(self.matches().len().saturating_sub(1));
                Input::Consumed
            }
            Key::Named(Named::Backspace) => {
                self.query.pop();
                self.selected = 0;
                Input::Consumed
            }
            _ => {
                if !modifiers.control() && !modifiers.logo() && !modifiers.alt() {
                    if let Some(text) = text
                        && self.query.len() + text.len() <= 128
                        && !text.chars().any(char::is_control)
                    {
                        self.query.push_str(text);
                        self.selected = 0;
                    }
                }
                Input::Consumed
            }
        }
    }
    #[cfg(feature = "lua-scenes")]
    /// The inputs one render will see, as the module receives them.
    ///
    /// Exposed so a test can ask "did the row get its mark" without a window, and
    /// so a future chrome Spore can be fed from the same contract.
    pub fn inputs(
        &self,
        now: u64,
    ) -> std::collections::BTreeMap<String, crate::lua_module::ModuleInputValue> {
        let mut values = std::collections::BTreeMap::new();
        let fresh = self.catalog.is_some()
            && (self.window_cycle || now.saturating_sub(self.observed) < LEASE_MS)
            && !self.busy;
        let rows = self.matches();
        let start = self.row_start();
        for (slot, (_, entry)) in rows
            .iter()
            .enumerate()
            .skip(start)
            .take(self.visible_rows())
            .enumerate()
        {
            if let Some(icon) = &entry.icon {
                values.insert(
                    format!("row{slot}-badge"),
                    crate::lua_module::ModuleInputValue::Text(icon.glyph.clone()),
                );
                values.insert(
                    format!("row{slot}-tint"),
                    crate::lua_module::ModuleInputValue::Text(format!(
                        "#{:02x}{:02x}{:02x}",
                        icon.tint[0], icon.tint[1], icon.tint[2]
                    )),
                );
            }
        }
        let _ = fresh;
        values
    }

    pub fn render(
        &mut self,
        theme: crate::lua_module::ModuleThemeTokens,
        now: u64,
    ) -> Result<(), String> {
        use crate::lua_module::{ModuleInputSnapshot, ModuleInputValue};
        let mut values = std::collections::BTreeMap::new();
        values.insert(
            "width".into(),
            ModuleInputValue::Number(f64::from(self.viewport[0])),
        );
        values.insert(
            "height".into(),
            ModuleInputValue::Number(f64::from(self.viewport[1])),
        );
        let fresh = self.catalog.is_some()
            && (self.window_cycle || now.saturating_sub(self.observed) < LEASE_MS)
            && !self.busy;
        values.insert(
            "window-cycle".into(),
            ModuleInputValue::Boolean(self.window_cycle),
        );
        values.insert(
            "scope".into(),
            ModuleInputValue::Text(self.scope.name().into()),
        );
        for (id, value) in [
            ("target", self.targets[self.target].label.clone()),
            ("query", self.query.clone()),
            (
                "health",
                if fresh || self.busy {
                    self.status.clone()
                } else if self.catalog.is_some() {
                    "STALE · F5 to refresh".into()
                } else {
                    self.status.clone()
                },
            ),
        ] {
            if !value.trim().is_empty() {
                values.insert(id.into(), ModuleInputValue::Text(value));
            }
        }
        values.insert("live".into(), ModuleInputValue::Boolean(fresh));
        let rows = self.matches();
        let start = self.row_start();
        for (slot, (index, e)) in rows
            .iter()
            .enumerate()
            .skip(start)
            .take(self.visible_rows())
            .enumerate()
        {
            for (suffix, value) in [
                (
                    "title",
                    e.title
                        .chars()
                        .scan(0usize, |bytes, ch| {
                            *bytes += ch.len_utf8();
                            (*bytes <= 80).then_some(ch)
                        })
                        .collect(),
                ),
                ("detail", e.detail.clone()),
            ] {
                values.insert(format!("row{slot}-{suffix}"), ModuleInputValue::Text(value));
            }
            values.insert(
                format!("row{slot}-selected"),
                ModuleInputValue::Boolean(index == self.selected),
            );
            values.insert(
                format!("row{slot}-available"),
                ModuleInputValue::Boolean(fresh && e.available),
            );
            // A mark and a colour per row: two apps whose rows differ only in text
            // are two entries a person has to read to tell apart.
            if let Some(icon) = &e.icon {
                values.insert(
                    format!("row{slot}-badge"),
                    ModuleInputValue::Text(icon.glyph.clone()),
                );
                values.insert(
                    format!("row{slot}-tint"),
                    ModuleInputValue::Text(format!(
                        "#{:02x}{:02x}{:02x}",
                        icon.tint[0], icon.tint[1], icon.tint[2]
                    )),
                );
            }
        }
        values.insert("count".into(), ModuleInputValue::Number(rows.len() as f64));
        values.extend(self.inputs(now));
        if std::env::var_os("CANVAS_LAUNCHER_TRACE").is_some() {
            eprintln!(
                "launcher trace: rows={} icon0={:?} badge0={:?}",
                rows.len(),
                rows.first().and_then(|entry| entry.icon.clone()),
                values.get("row0-badge")
            );
        }
        self.presentation
            .refresh_files(
                crate::shell_presentation::DESKTOP_LAUNCHER,
                ModuleInputSnapshot {
                    revision: self.generation,
                    now_ms: (now / 1000 * 1000).max(1),
                    values,
                },
                theme,
            )
            .map_err(|e| e.to_string())?;
        self.scene = self
            .presentation
            .retained()
            .map(|output| output.scene.clone());
        if std::env::var_os("CANVAS_LAUNCHER_TRACE").is_some() {
            eprintln!(
                "launcher scene: {:?}",
                self.scene
                    .as_ref()
                    .map(|scene| scene.nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>())
            );
        }
        self.buttons = self
            .presentation
            .retained()
            .map(|output| output.buttons.clone())
            .unwrap_or_default();
        Ok(())
    }
}
pub enum Input {
    Consumed,
    Cancel,
    Apply,
    Reload,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> crate::lua_module::ModuleThemeTokens {
        canvas_core::ProjectTheme {
            mode: canvas_core::ThemeMode::Dark,
            accent: canvas_core::Accent::Info,
        }
        .into()
    }

    fn launcher_with(entries: Vec<Entry>) -> Launcher {
        let mut launcher = Launcher::new().expect("launcher");
        launcher.catalog = Some(Catalog {
            revision: "test".into(),
            entries,
        });
        launcher.loading = false;
        launcher.observed = 1_000;
        launcher
    }

    #[test]
    fn a_badged_app_row_reaches_the_module_as_badge_and_tint() {
        // The catalog carries the icon; whether the view ever sees it is a
        // question about this translation, not about the drawing.
        let mut launcher = launcher_with(vec![
            Entry::new(
                "app:dev.fungos.unibus",
                "Inbox",
                "APP · 0.1.0",
                true,
                Choice::Spore {
                    module: "spore-dev-inbox".into(),
                },
            )
            .badged("I", [0x6e, 0xa8, 0xfe]),
        ]);
        launcher.viewport = [600.0, 440.0];
        let inputs = launcher.inputs(1_000);
        let badge = inputs
            .get("row0-badge")
            .expect("the first row carries its monogram");
        let tint = inputs.get("row0-tint").expect("and its tint");
        assert_eq!(
            badge,
            &crate::lua_module::ModuleInputValue::Text("I".into()),
            "the module receives the glyph, not a placeholder"
        );
        assert_eq!(
            tint,
            &crate::lua_module::ModuleInputValue::Text("#6ea8fe".into())
        );
    }

    #[test]
    fn the_launcher_draws_the_badge_through_the_real_render() {
        let mut launcher = launcher_with(vec![
            Entry::new(
                "app:dev.fungos.unibus",
                "Inbox",
                "APP · 0.1.0",
                true,
                Choice::Spore {
                    module: "spore-dev-inbox".into(),
                },
            )
            .badged("I", [0x6e, 0xa8, 0xfe]),
        ]);
        launcher.viewport = [600.0, 440.0];
        launcher.render(theme(), 1_000).expect("the launcher renders");
        let scene = launcher.scene.as_ref().expect("a scene");
        let mark = scene.nodes.iter().find(|node| node.id == "row0-mark");
        assert!(
            mark.is_some(),
            "a badged row must draw its mark; nodes were {:?}",
            scene.nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>()
        );
        assert!(scene.nodes.iter().any(|node| node.id == "row0-glyph"));
    }
}
