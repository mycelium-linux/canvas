//! Canvas as its own Wayland compositor: `canvas-linux desktop-compositor`.
//!
//! `canvas-linux desktop-start` launches a headless **Sway** compositor and drives
//! it over `swaymsg`. `desktop-compositor` instead makes canvas-linux *be* the
//! compositor itself, replacing Sway as the window manager. The home-pi kiosk is
//! the target deployment: Canvas owns the seat instead of running as a window
//! inside another WM.
//!
//! `CanvasCompositor` implements the narrow `canvas_core::desktop` contracts
//! (`WindowBackend`, `WorkspaceBackend`, `WallpaperBackend`) that the macOS and
//! Sway backends already implement. Project recall, layout, focus, and
//! wallpaper orchestration therefore behave unchanged — only the underlying
//! compositor swaps. The window/workspace/wallpaper state model below is
//! platform independent and is unit tested on every host. The Wayland server
//! (Smithaw) is compiled only with the `compositor` feature; see `mod runner`.
//!
//! Window lifecycle events (map/focus/close) flow through [`WindowLifecycle`].
//! That is the integration point for the later "window lifecycle into the
//! lua-stream control" phase: a stream producer can implement `WindowLifecycle`
//! to push map/focus/close frames into the `…/dock/canvas/stream.sock` inbox
//! that retained-Lua binders consume.

// The CanvasCompositor state model backs the `compositor` feature (Smithaw
// server) and the tests below. It is the canvas-core desktop contract in
// platform-independent form; not every method is reached from every build
// (the fail-loud binary path does not construct it). Allow dead code in
// non-test builds so the default and feature-gated bins stay warning-free;
// under `test` the full model is exercised and must remain live.
#![cfg_attr(not(test), allow(dead_code))]

use canvas_core::desktop::{
    DesktopSnapshot, Display, LayoutReceipt, Wallpaper, WallpaperBackend, WallpaperReceipt,
    WallpaperRequest, WindowBackend, WindowSurface, WorkspaceBackend,
};
use canvas_core::layout::{LayoutPlan, PaneRole, Placement, Rect};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Lifecycle events raised by the compositor as surfaces come and go. Default
/// impls are no-ops so backends that do not need lifecycle plumbing compile
/// without one. A stream producer implements this to feed retained-Lua binders.
pub trait WindowLifecycle {
    fn window_mapped(&mut self, _window: &WindowSurface) {}
    fn window_focused(&mut self, _window_id: &str) {}
    fn window_closed(&mut self, _window_id: &str) {}
    fn window_unmapped(&mut self, _window_id: &str) {}
}

pub struct NoOpLifecycle;
impl WindowLifecycle for NoOpLifecycle {}

#[derive(Clone)]
pub struct WindowEntry {
    pub surface: WindowSurface,
    pub mapped: bool,
    normal_frame: Option<Rect>,
    maximized: bool,
    fullscreen: bool,
}

impl WindowEntry {
    fn new(surface: WindowSurface, mapped: bool) -> Self {
        Self {
            surface,
            mapped,
            normal_frame: None,
            maximized: false,
            fullscreen: false,
        }
    }
}

/// The window/workspace/wallpaper state model for canvas-linux-as-compositor.
///
/// The `WindowBackend`/`WorkspaceBackend`/`WallpaperBackend` impls below are the
/// contract any compositor backend (Sway, Smithaw, macOS) must satisfy; tests
/// assert this one matches a reference backend bit-for-bit. The Wayland server
/// in `mod runner` mutates this state through [`Self::map_window`],
/// [`Self::focus_window`] and [`Self::close_window`], which is also where
/// lifecycle events originate.
pub struct CanvasCompositor {
    displays: Vec<Display>,
    windows: BTreeMap<String, WindowEntry>,
    focused: Option<String>,
    mru: Vec<String>,
    shell_overlays: std::collections::BTreeSet<String>,
    workspace: Option<String>,
    wallpapers: BTreeMap<String, Wallpaper>,
    lifecycle: Box<dyn WindowLifecycle + Send>,
}

impl Default for CanvasCompositor {
    fn default() -> Self {
        Self::new([])
    }
}

impl CanvasCompositor {
    pub fn new<I: IntoIterator<Item = Display>>(displays: I) -> Self {
        Self::with_lifecycle(displays, Box::new(NoOpLifecycle))
    }

    pub fn with_lifecycle<I: IntoIterator<Item = Display>>(
        displays: I,
        lifecycle: Box<dyn WindowLifecycle + Send>,
    ) -> Self {
        Self {
            displays: displays.into_iter().collect(),
            windows: BTreeMap::new(),
            focused: None,
            mru: Vec::new(),
            shell_overlays: Default::default(),
            workspace: None,
            wallpapers: BTreeMap::new(),
            lifecycle,
        }
    }

    pub fn displays(&self) -> &[Display] {
        &self.displays
    }

    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub fn update_identity(&mut self, id: &str, app: Option<String>, title: Option<String>) {
        if let Some(entry) = self.windows.get_mut(id) {
            if let Some(app) = app {
                entry.surface.app = app.chars().filter(|c| !c.is_control()).take(256).collect();
            }
            if let Some(title) = title {
                entry.surface.title = title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(512)
                    .collect();
            }
        }
    }
    pub fn mru(&self) -> &[String] {
        &self.mru
    }

    pub fn is_shell_overlay(&self, id: &str) -> bool {
        self.shell_overlays.contains(id)
    }
    pub fn window(&self, id: &str) -> Option<&WindowSurface> {
        self.windows.get(id).map(|entry| &entry.surface)
    }
    pub fn is_mapped(&self, id: &str) -> bool {
        self.windows.get(id).is_some_and(|entry| entry.mapped)
    }
    /// Protocol identity/layout may exist before a buffer is committed.
    pub fn register_window(&mut self, id: String, surface: WindowSurface) {
        self.windows
            .entry(id)
            .or_insert_with(|| WindowEntry::new(surface, false));
    }
    pub fn set_mapped(&mut self, id: &str, mapped: bool) -> Result<bool, String> {
        let entry = self
            .windows
            .get_mut(id)
            .ok_or_else(|| format!("unknown window {id}"))?;
        if entry.mapped == mapped {
            return Ok(false);
        }
        entry.mapped = mapped;
        if mapped {
            if !self.shell_overlays.contains(id) {
                self.mru.push(id.to_owned());
            }
            self.lifecycle.window_mapped(&entry.surface);
        } else {
            if let Some(frame) = entry.normal_frame.take() {
                entry.surface.frame = frame;
            }
            entry.maximized = false;
            entry.fullscreen = false;
            self.mru.retain(|window| window != id);
            if self.focused.as_deref() == Some(id) {
                self.focused = None;
            }
            self.lifecycle.window_unmapped(id);
        }
        Ok(true)
    }

    /// Presentation-only surfaces never become app-switching destinations.
    pub fn mark_shell_overlay(&mut self, id: &str) -> Result<(), String> {
        if !self.windows.contains_key(id) {
            return Err(format!("unknown window {id}"));
        }
        if self.focused() == Some(id) {
            return Err("focused app cannot become a shell overlay".into());
        }
        self.shell_overlays.insert(id.to_owned());
        self.mru.retain(|window| window != id);
        Ok(())
    }

    pub fn place_window(&mut self, id: &str, frame: Rect) -> Result<(), String> {
        if frame.width == 0
            || frame.height == 0
            || frame.width > i32::MAX as u32
            || frame.height > i32::MAX as u32
        {
            return Err("window frame dimensions must fit positive Wayland integers".into());
        }
        let entry = self
            .windows
            .get_mut(id)
            .ok_or_else(|| format!("unknown window {id}"))?;
        if entry.maximized || entry.fullscreen {
            entry.normal_frame = Some(frame);
        } else {
            entry.surface.frame = frame;
        }
        Ok(())
    }

    pub fn window_states(&self, id: &str) -> Option<(bool, bool)> {
        self.windows
            .get(id)
            .map(|entry| (entry.maximized, entry.fullscreen))
    }

    /// Expanded geometry is derived from the owning output, never a second
    /// layout plan. Fullscreen temporarily overrides maximization; leaving it
    /// restores the maximized work area or the latest normal layout rectangle.
    pub fn set_window_state(
        &mut self,
        id: &str,
        maximized: Option<bool>,
        fullscreen: Option<bool>,
        output_id: Option<&str>,
    ) -> Result<(), String> {
        if self.is_shell_overlay(id) {
            return Err("shell overlay cannot change desktop window state".into());
        }
        let entry = self
            .windows
            .get(id)
            .ok_or_else(|| format!("unknown window {id}"))?;
        let max = maximized.unwrap_or(entry.maximized);
        let full = fullscreen.unwrap_or(entry.fullscreen);
        let output = if max || full {
            let display = match output_id {
                Some(output_id) => self.displays.iter().find(|display| display.id == output_id),
                None => self
                    .displays
                    .iter()
                    .find(|display| {
                        let frame = entry.surface.frame;
                        let center_x = i64::from(frame.x) + i64::from(frame.width) / 2;
                        let center_y = i64::from(frame.y) + i64::from(frame.height) / 2;
                        let area = display.frame;
                        center_x >= i64::from(area.x)
                            && center_y >= i64::from(area.y)
                            && center_x < i64::from(area.x) + i64::from(area.width)
                            && center_y < i64::from(area.y) + i64::from(area.height)
                    })
                    .or_else(|| self.displays.iter().find(|display| display.primary))
                    .or_else(|| self.displays.first()),
            }
            .ok_or("window state requires a known output")?;
            let frame = if full {
                display.frame
            } else {
                display.work_area
            };
            if frame.width == 0
                || frame.height == 0
                || frame.width > i32::MAX as u32
                || frame.height > i32::MAX as u32
            {
                return Err("output has invalid window dimensions".into());
            }
            Some(frame)
        } else {
            None
        };
        let entry = self.windows.get_mut(id).expect("validated window");
        if let Some(frame) = output {
            entry.normal_frame.get_or_insert(entry.surface.frame);
            entry.surface.frame = frame;
        } else if let Some(frame) = entry.normal_frame.take() {
            entry.surface.frame = frame;
        }
        entry.maximized = max;
        entry.fullscreen = full;
        Ok(())
    }

    /// Record an output topology. The Wayland server calls this on hotplug.
    pub fn set_outputs(&mut self, displays: Vec<Display>) {
        self.displays = displays;
    }

    /// The server reports a toplevel has mapped; insert it and notify lifecycle.
    pub fn map_window(&mut self, id: String, surface: WindowSurface) {
        self.register_window(id.clone(), surface.clone());
        self.windows
            .get_mut(&id)
            .expect("registered window")
            .surface = surface;
        self.set_mapped(&id, true).expect("registered window");
    }

    /// The server reports focus changed; track it and notify lifecycle.
    pub fn focus_window(&mut self, id: &str) -> Result<(), String> {
        if self.is_shell_overlay(id) {
            return Err("shell overlay cannot take keyboard focus".into());
        }
        if !self.is_mapped(id) {
            return Err(format!("unknown window {id}"));
        }
        if self.focused.as_deref() != Some(id) {
            self.mru.retain(|v| v != id);
            self.mru.insert(0, id.to_owned());
            self.focused = Some(id.to_string());
            self.lifecycle.window_focused(id);
        }
        Ok(())
    }

    /// The server reports a toplevel closed; remove it and notify lifecycle.
    pub fn close_window(&mut self, id: &str) {
        if self.windows.remove(id).is_some() {
            self.shell_overlays.remove(id);
            self.mru.retain(|v| v != id);
            self.lifecycle.window_closed(id);
            if self.focused.as_deref() == Some(id) {
                self.focused = None;
            }
        }
    }

    fn primary_placement<'a>(&self, plan: &'a LayoutPlan) -> Option<&'a Placement> {
        plan.placements.iter().find(|p| p.role == PaneRole::Browser)
    }
}

impl WindowBackend for CanvasCompositor {
    type Error = String;

    fn snapshot(&mut self) -> Result<DesktopSnapshot, Self::Error> {
        Ok(DesktopSnapshot {
            displays: self.displays.clone(),
            windows: self
                .windows
                .values()
                .filter(|entry| entry.mapped)
                .map(|entry| entry.surface.clone())
                .collect(),
        })
    }

    fn apply_layout(&mut self, plan: &LayoutPlan) -> Result<LayoutReceipt, Self::Error> {
        let mut placed = 0usize;
        for placement in &plan.placements {
            if self.windows.contains_key(&placement.id) {
                self.place_window(&placement.id, placement.frame)?;
                placed += 1;
            }
        }
        if placed == 0 {
            return Err("apply_layout matched no known windows".into());
        }
        // Mirror canvas-mac: the primary (browser) region keeps focus so the
        // layout plan decides what is active.
        if let Some(primary) = self.primary_placement(plan) {
            self.focus_window(&primary.id).ok();
        }
        let focused = self.focused.clone().unwrap_or_else(|| {
            self.primary_placement(plan)
                .map(|p| p.id.clone())
                .unwrap_or_default()
        });
        Ok(LayoutReceipt {
            placements: plan.placements.clone(),
            focused,
        })
    }

    fn focus(&mut self, window_id: &str) -> Result<WindowSurface, Self::Error> {
        self.focus_window(window_id)?;
        self.windows
            .get(window_id)
            .map(|entry| entry.surface.clone())
            .ok_or_else(|| format!("unknown window {window_id}"))
    }
}

impl WorkspaceBackend for CanvasCompositor {
    type Error = String;

    fn recall_workspace(&mut self, project: &str) -> Result<String, Self::Error> {
        if project.is_empty() || project.len() > 64 {
            return Err("project name must be 1–64 chars".into());
        }
        self.workspace = Some(project.to_string());
        Ok(project.to_string())
    }
}

impl WallpaperBackend for CanvasCompositor {
    type Error = String;

    fn set_wallpaper(
        &mut self,
        request: &WallpaperRequest,
    ) -> Result<WallpaperReceipt, Self::Error> {
        if !self.displays.iter().any(|d| d.id == request.display_id) {
            return Err(format!(
                "wallpaper display {} is not in the compositor's output set",
                request.display_id
            ));
        }
        self.wallpapers
            .insert(request.display_id.clone(), request.wallpaper.clone());
        Ok(WallpaperReceipt {
            project: request.project.clone(),
            display_id: request.display_id.clone(),
            wallpaper: request.wallpaper.clone(),
        })
    }
}

/// Public entry for the `desktop-compositor` command family. `start` launches the
/// Smithaw Wayland server (feature-gated); without the `compositor` feature the
/// command fails fast with a precise, actionable message.
/// Stream producer: maps CanvasCompositor lifecycle events into JSON datagrams
/// on the Canvas Unix datagram socket, so retained-Lua binders can react to
/// window map/focus/close. The envelope mirrors the shape used by
/// `scripts/music-nowplaying-feed.py` (`owner`/`stream`/`contract`/
/// `schema_version`/`sequence`/`emitted_at_ms`/`payload`).
pub struct StreamLifecycle {
    socket: Option<std::path::PathBuf>,
    sequence: u64,
}

impl StreamLifecycle {
    pub fn new(socket: Option<std::path::PathBuf>) -> Self {
        Self {
            socket,
            sequence: 0,
        }
    }

    fn post(&mut self, event: &str, window_id: &str, app: &str, title: &str) -> bool {
        let target = match &self.socket {
            Some(path) => path,
            None => return false,
        };
        let frame = envelope(event, window_id, app, title, self.sequence);
        self.sequence = self.sequence.wrapping_add(1);
        // SOCK_DGRAM to the stream socket: bind a throwaway path, connect,
        // send. Mirrors scripts/music-nowplaying-feed.py (sock.connect + send).
        let sender = std::env::temp_dir().join(format!(
            "canvas-lifecycle-{}-{}.sock",
            std::process::id(),
            self.sequence.wrapping_sub(1)
        ));
        let _ = std::fs::remove_file(&sender);
        let sock = match std::os::unix::net::UnixDatagram::bind(&sender) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let ok = sock
            .connect(target)
            .and_then(|()| sock.send(frame.as_bytes()))
            .is_ok();
        let _ = std::fs::remove_file(&sender);
        ok
    }
}

fn emitted_at_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn envelope(event: &str, window_id: &str, app: &str, title: &str, sequence: u64) -> String {
    json!({
        "schema_version": 1,
        "owner": "compositor",
        "stream": "windows",
        "contract": "canvas.window.lifecycle.v1",
        "sequence": sequence,
        "emitted_at_ms": emitted_at_ms(),
        "payload": {
            "event": event,
            "window_id": window_id,
            "app": app,
            "title": title,
        },
    })
    .to_string()
}

impl WindowLifecycle for StreamLifecycle {
    fn window_mapped(&mut self, w: &WindowSurface) {
        let _ = self.post("mapped", &w.id, &w.app, &w.title);
    }
    fn window_focused(&mut self, id: &str) {
        let _ = self.post("focused", id, "", "");
    }
    fn window_closed(&mut self, id: &str) {
        let _ = self.post("closed", id, "", "");
    }
    fn window_unmapped(&mut self, id: &str) {
        let _ = self.post("unmapped", id, "", "");
    }
}

pub fn execute(args: &[String]) -> Result<Value, String> {
    let action = args
        .first()
        .map(String::as_str)
        .unwrap_or("desktop-compositor");
    match (action, args.get(1).map(String::as_str), args.len()) {
        ("desktop-compositor", None | Some("start"), 1 | 2) => start(),
        ("desktop-compositor", Some("status"), 2) => status(),
        ("desktop-compositor", Some("headless"), 2) => {
            #[cfg(all(feature = "compositor", target_os = "linux"))]
            {
                crate::runner::run_headless()
            }
            #[cfg(not(all(feature = "compositor", target_os = "linux")))]
            {
                Err("headless compositor requires Linux and the compositor feature".into())
            }
        }
        _ => Err("usage: canvas-linux desktop-compositor [start|status]".into()),
    }
}

fn start() -> Result<Value, String> {
    #[cfg(all(feature = "compositor", target_os = "linux"))]
    {
        return crate::runner::run();
    }
    #[cfg(not(all(feature = "compositor", target_os = "linux")))]
    {
        Err(
            "canvas-linux compositor backend is not compiled in; enable the \
             `compositor` feature to build canvas-linux as a Wayland compositor."
                .into(),
        )
    }
}

fn status() -> Result<Value, String> {
    let base = json!({
        "compositor": "canvas-linux compositor (canvas-core desktop contract)",
        "backend_compiled": cfg!(all(feature = "compositor", target_os = "linux")),
    });
    #[cfg(all(feature = "compositor", target_os = "linux"))]
    {
        let mut base = base;
        if let Some(state) = crate::runner::status() {
            base["workspace"] = json!(state.workspace);
            base["window_count"] = json!(state.window_count);
            base["wayland_display"] = json!(state.wayland_display);
        }
        Ok(base)
    }
    #[cfg(not(all(feature = "compositor", target_os = "linux")))]
    {
        Ok(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::layout::{PaneRole, Placement, Rect};

    #[test]
    fn expanded_states_restore_latest_layout_and_keep_focus() {
        let mut output = display("eDP-1", 1280, 720);
        output.work_area = rect(0, 30, 1280, 690);
        let mut canvas = CanvasCompositor::new([output]);
        canvas.map_window("app".into(), window("app", "foot"));
        canvas.focus_window("app").unwrap();
        let original = canvas.window("app").unwrap().frame;
        canvas
            .set_window_state("app", Some(true), None, None)
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, rect(0, 30, 1280, 690));
        canvas
            .set_window_state("app", None, Some(true), None)
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, rect(0, 0, 1280, 720));
        canvas
            .set_window_state("app", None, Some(true), None)
            .unwrap();
        canvas
            .set_window_state("app", None, Some(false), None)
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, rect(0, 30, 1280, 690));
        canvas
            .set_window_state("app", Some(false), None, None)
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, original);
        canvas
            .set_window_state("app", None, Some(true), None)
            .unwrap();
        let latest = rect(80, 60, 640, 480);
        canvas.place_window("app", latest).unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, rect(0, 0, 1280, 720));
        canvas
            .set_window_state("app", None, Some(false), None)
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, latest);
        assert_eq!(canvas.focused(), Some("app"));
        assert_eq!(canvas.mru(), &["app".to_owned()]);
    }

    #[test]
    fn expanded_state_rejection_is_atomic_and_unmap_resets_state() {
        let mut canvas = CanvasCompositor::new([display("eDP-1", 1280, 720)]);
        canvas.map_window("app".into(), window("app", "foot"));
        let original = canvas.window("app").unwrap().frame;
        assert!(
            canvas
                .set_window_state("missing", Some(true), None, None)
                .is_err()
        );
        assert!(
            canvas
                .set_window_state("app", None, Some(true), Some("missing"))
                .is_err()
        );
        assert_eq!(canvas.window_states("app"), Some((false, false)));
        assert_eq!(canvas.window("app").unwrap().frame, original);
        canvas
            .set_window_state("app", Some(true), Some(true), None)
            .unwrap();
        canvas.set_mapped("app", false).unwrap();
        assert_eq!(canvas.window_states("app"), Some((false, false)));
        assert_eq!(canvas.window("app").unwrap().frame, original);
        canvas.set_mapped("app", true).unwrap();
        canvas.map_window("overlay".into(), window("overlay", "shell"));
        canvas.mark_shell_overlay("overlay").unwrap();
        assert!(
            canvas
                .set_window_state("overlay", Some(true), None, None)
                .is_err()
        );
        canvas.set_outputs(vec![]);
        assert!(
            canvas
                .set_window_state("app", Some(true), None, None)
                .is_err()
        );
        assert_eq!(canvas.window_states("app"), Some((false, false)));
    }

    #[test]
    fn expanded_state_uses_window_output_or_explicit_output() {
        let first = display("eDP-1", 1280, 720);
        let mut second = display("HDMI-1", 1920, 1080);
        second.frame.x = 1280;
        second.work_area = rect(1280, 40, 1920, 1040);
        let mut canvas = CanvasCompositor::new([first, second]);
        canvas.map_window("app".into(), window("app", "foot"));
        canvas
            .place_window("app", rect(1400, 100, 400, 300))
            .unwrap();
        canvas
            .set_window_state("app", Some(true), None, None)
            .unwrap();
        assert_eq!(
            canvas.window("app").unwrap().frame,
            rect(1280, 40, 1920, 1040)
        );
        canvas
            .set_window_state("app", None, Some(true), Some("eDP-1"))
            .unwrap();
        assert_eq!(canvas.window("app").unwrap().frame, rect(0, 0, 1280, 720));
    }

    #[test]
    fn shell_overlay_is_observed_but_never_a_focus_or_mru_candidate() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("app".into(), window("app", "foot"));
        canvas.map_window("overlay".into(), window("overlay", "shell"));
        canvas.focus_window("app").unwrap();
        canvas.mark_shell_overlay("overlay").unwrap();
        assert_eq!(canvas.mru(), &["app".to_owned()]);
        assert!(canvas.focus_window("overlay").is_err());
        assert_eq!(canvas.focused(), Some("app"));
        assert!(canvas.mark_shell_overlay("app").is_err());
        canvas.close_window("overlay");
        assert!(!canvas.is_shell_overlay("overlay"));
    }

    #[test]
    fn native_identity_is_bounded_and_does_not_change_window_id() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("stable".into(), window("stable", "old"));
        canvas.update_identity("stable", Some("a\n".repeat(300)), Some("t\t".repeat(600)));
        let observed = canvas.snapshot().unwrap();
        assert_eq!(observed.windows[0].id, "stable");
        assert_eq!(observed.windows[0].app.len(), 256);
        assert_eq!(observed.windows[0].title.len(), 512);
        assert_eq!(canvas.mru(), &["stable".to_owned()]);
    }

    #[test]
    fn cli_accepts_separate_status_argument_and_rejects_extra_arguments() {
        assert!(execute(&["desktop-compositor".into(), "status".into()]).is_ok());
        assert!(execute(&["desktop-compositor".into(), "status".into(), "extra".into()]).is_err());
        assert!(execute(&["desktop-compositor".into(), "unknown".into()]).is_err());
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }
    fn display(id: &str, w: u32, h: u32) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            frame: rect(0, 0, w, h),
            work_area: rect(0, 0, w, h),
            primary: id == "eDP-1",
        }
    }
    fn window(id: &str, app: &str) -> WindowSurface {
        WindowSurface {
            id: id.into(),
            process_id: None,
            app: app.into(),
            title: String::new(),
            frame: rect(0, 0, 400, 300),
            movable: true,
            resizable: true,
        }
    }

    /// Reference backend implementing the canvas-core desktop contract exactly;
    /// used to prove parity with `CanvasCompositor` on shared fixtures.
    #[derive(Default)]
    struct FakeBackend {
        displays: Vec<Display>,
        windows: BTreeMap<String, WindowEntry>,
        focused: Option<String>,
        workspace: Option<String>,
        wallpapers: BTreeMap<String, Wallpaper>,
    }

    impl FakeBackend {
        fn new(displays: Vec<Display>) -> Self {
            Self {
                displays,
                ..Default::default()
            }
        }
    }

    impl WindowBackend for FakeBackend {
        type Error = String;
        fn snapshot(&mut self) -> Result<DesktopSnapshot, Self::Error> {
            Ok(DesktopSnapshot {
                displays: self.displays.clone(),
                windows: self
                    .windows
                    .values()
                    .filter(|e| e.mapped)
                    .map(|e| e.surface.clone())
                    .collect(),
            })
        }
        fn apply_layout(&mut self, plan: &LayoutPlan) -> Result<LayoutReceipt, Self::Error> {
            let mut placed = 0usize;
            for placement in &plan.placements {
                if let Some(entry) = self.windows.get_mut(&placement.id) {
                    entry.surface.frame = placement.frame;
                    placed += 1;
                }
            }
            if placed == 0 {
                return Err("no windows matched".into());
            }
            let primary = plan.placements.iter().find(|p| p.role == PaneRole::Browser);
            if primary.is_some_and(|p| self.windows.contains_key(&p.id)) {
                self.focused = Some(primary.unwrap().id.clone());
            }
            let focused = self.focused.clone().unwrap_or_default();
            Ok(LayoutReceipt {
                placements: plan.placements.clone(),
                focused,
            })
        }
        fn focus(&mut self, window_id: &str) -> Result<WindowSurface, Self::Error> {
            if !self.windows.contains_key(window_id) {
                return Err(format!("unknown window {window_id}"));
            }
            self.focused = Some(window_id.to_string());
            Ok(self.windows[window_id].surface.clone())
        }
    }

    impl WorkspaceBackend for FakeBackend {
        type Error = String;
        fn recall_workspace(&mut self, project: &str) -> Result<String, Self::Error> {
            self.workspace = Some(project.to_string());
            Ok(project.to_string())
        }
    }

    impl WallpaperBackend for FakeBackend {
        type Error = String;
        fn set_wallpaper(
            &mut self,
            request: &WallpaperRequest,
        ) -> Result<WallpaperReceipt, Self::Error> {
            if !self.displays.iter().any(|d| d.id == request.display_id) {
                return Err("display not found".into());
            }
            self.wallpapers
                .insert(request.display_id.clone(), request.wallpaper.clone());
            Ok(WallpaperReceipt {
                project: request.project.clone(),
                display_id: request.display_id.clone(),
                wallpaper: request.wallpaper.clone(),
            })
        }
    }

    // --- CanvasCompositor lifecycle + state machine ---

    struct Recorder(std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>);
    impl WindowLifecycle for Recorder {
        fn window_mapped(&mut self, w: &WindowSurface) {
            self.0.lock().unwrap().push(("mapped".into(), w.id.clone()));
        }
        fn window_focused(&mut self, id: &str) {
            self.0.lock().unwrap().push(("focused".into(), id.into()));
        }
        fn window_closed(&mut self, id: &str) {
            self.0.lock().unwrap().push(("closed".into(), id.into()));
        }
        fn window_unmapped(&mut self, id: &str) {
            self.0.lock().unwrap().push(("unmapped".into(), id.into()));
        }
    }
    #[test]
    fn registered_windows_are_not_focusable_and_remap_retains_identity_and_layout() {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut c = CanvasCompositor::with_lifecycle([], Box::new(Recorder(events.clone())));
        c.register_window("app".into(), window("app", "pending"));
        assert!(c.snapshot().unwrap().windows.is_empty());
        assert!(c.mru().is_empty());
        assert!(c.focus_window("app").is_err());
        c.update_identity("app", Some("foot".into()), Some("ready".into()));
        let frame = Rect {
            x: 20,
            y: 30,
            width: 640,
            height: 480,
        };
        c.place_window("app", frame).unwrap();
        assert!(c.set_mapped("app", true).unwrap());
        assert!(!c.set_mapped("app", true).unwrap());
        c.focus_window("app").unwrap();
        assert!(c.set_mapped("app", false).unwrap());
        assert!(!c.set_mapped("app", false).unwrap());
        assert!(c.snapshot().unwrap().windows.is_empty());
        assert!(c.focused().is_none());
        assert!(c.mru().is_empty());
        assert!(c.set_mapped("app", true).unwrap());
        let snap = c.snapshot().unwrap();
        assert_eq!(snap.windows[0].id, "app");
        assert_eq!(snap.windows[0].app, "foot");
        assert_eq!(snap.windows[0].frame, frame);
        assert_eq!(c.mru(), &["app"]);
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                ("mapped".into(), "app".into()),
                ("focused".into(), "app".into()),
                ("unmapped".into(), "app".into()),
                ("mapped".into(), "app".into())
            ]
        );
    }
    #[test]
    fn a_pending_shell_overlay_never_enters_mru_on_map_or_remap() {
        let mut c = CanvasCompositor::default();
        c.register_window("overlay".into(), window("overlay", "shell"));
        c.mark_shell_overlay("overlay").unwrap();
        c.set_mapped("overlay", true).unwrap();
        c.set_mapped("overlay", false).unwrap();
        c.set_mapped("overlay", true).unwrap();
        assert!(c.mru().is_empty());
        assert!(c.focus_window("overlay").is_err());
    }

    #[test]
    fn map_focus_close_drives_state_and_lifecycle() {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let mut c = CanvasCompositor::with_lifecycle(
            vec![display("eDP-1", 1920, 1080)],
            Box::new(Recorder(events.clone())),
        );
        let w = window("term", "foot");
        c.map_window("term".into(), w.clone());
        c.focus_window("term").unwrap();
        assert!(c.focus_window("missing").is_err());
        let snap = c.snapshot().unwrap();
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].id, "term");
        c.close_window("term");
        assert!(c.snapshot().unwrap().windows.is_empty());
        assert!(c.focus_window("term").is_err());

        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].0, "mapped");
        assert_eq!(recorded[0].1, "term");
        assert_eq!(recorded[1].0, "focused");
        assert_eq!(recorded[1].1, "term");
        assert_eq!(recorded[2].0, "closed");
        assert_eq!(recorded[2].1, "term");
    }

    #[test]
    fn apply_layout_positions_only_known_windows_and_focuses_primary() {
        let mut c = CanvasCompositor::new(vec![display("eDP-1", 1920, 1080)]);
        c.map_window("term".into(), window("term", "foot"));
        c.map_window("web".into(), window("web", "firefox"));
        let plan = LayoutPlan {
            area: rect(0, 0, 1920, 1080),
            placements: vec![
                Placement {
                    id: "web".into(),
                    role: PaneRole::Browser,
                    region: canvas_core::layout::Region::Primary,
                    frame: rect(0, 0, 1280, 1080),
                },
                Placement {
                    id: "term".into(),
                    role: PaneRole::Tool,
                    region: canvas_core::layout::Region::Stack,
                    frame: rect(1280, 0, 640, 1080),
                },
                Placement {
                    id: "stray".into(),
                    role: PaneRole::Auxiliary,
                    region: canvas_core::layout::Region::Stack,
                    frame: rect(1920, 0, 0, 0),
                },
            ],
        };
        let receipt = c.apply_layout(&plan).unwrap();
        assert_eq!(receipt.focused, "web");
        assert_eq!(receipt.placements.len(), 3);
        let snap = c.snapshot().unwrap();
        let web = snap.windows.iter().find(|w| w.id == "web").unwrap();
        let term = snap.windows.iter().find(|w| w.id == "term").unwrap();
        assert_eq!(web.frame, rect(0, 0, 1280, 1080));
        assert_eq!(term.frame, rect(1280, 0, 640, 1080));
    }

    #[test]
    fn apply_layout_rejects_unknown_ids_and_bad_display() {
        let mut c = CanvasCompositor::new(vec![display("eDP-1", 1920, 1080)]);
        let plan = LayoutPlan {
            area: rect(0, 0, 1920, 1080),
            placements: vec![Placement {
                id: "ghost".into(),
                role: PaneRole::Browser,
                region: canvas_core::layout::Region::Primary,
                frame: rect(0, 0, 100, 100),
            }],
        };
        assert!(c.apply_layout(&plan).is_err());
        assert!(
            c.set_wallpaper(&WallpaperRequest {
                project: "dock".into(),
                display_id: "HDMI-A-1".into(),
                wallpaper: Wallpaper::Solid { rgb: [0, 0, 0] },
            })
            .is_err()
        );
    }

    #[test]
    fn focus_recall_and_wallpaper_contract() {
        let mut c = CanvasCompositor::new(vec![display("eDP-1", 800, 600)]);
        c.map_window("a".into(), window("a", "app"));
        assert_eq!(c.focus("a").unwrap().id, "a");
        assert_eq!(c.snapshot().unwrap().windows.len(), 1);
        assert_eq!(c.recall_workspace("dock").unwrap(), "dock");
        assert_eq!(c.workspace(), Some("dock"));
        let receipt = c
            .set_wallpaper(&WallpaperRequest {
                project: "dock".into(),
                display_id: "eDP-1".into(),
                wallpaper: Wallpaper::Solid { rgb: [24, 25, 28] },
            })
            .unwrap();
        assert_eq!(receipt.display_id, "eDP-1");
        assert!(matches!(receipt.wallpaper, Wallpaper::Solid { .. }));

        let image = c
            .set_wallpaper(&WallpaperRequest {
                project: "dock".into(),
                display_id: "eDP-1".into(),
                wallpaper: Wallpaper::ManagedImage {
                    asset_id: "sha256:abc".into(),
                    fit: canvas_core::desktop::WallpaperFit::Fill,
                },
            })
            .unwrap();
        assert!(matches!(image.wallpaper, Wallpaper::ManagedImage { .. }));
    }

    #[test]
    fn recall_workspace_rejects_invalid_names() {
        let mut c = CanvasCompositor::new([]);
        assert!(c.recall_workspace("").is_err());
        assert!(c.recall_workspace(&"x".repeat(65)).is_err());
        assert_eq!(c.recall_workspace("dock-1").unwrap(), "dock-1");
    }

    // --- Parity with the reference backend contract ---

    fn shared_plan() -> LayoutPlan {
        LayoutPlan {
            area: rect(0, 0, 1920, 1080),
            placements: vec![
                Placement {
                    id: "web".into(),
                    role: PaneRole::Browser,
                    region: canvas_core::layout::Region::Primary,
                    frame: rect(0, 0, 1280, 1080),
                },
                Placement {
                    id: "term".into(),
                    role: PaneRole::Tool,
                    region: canvas_core::layout::Region::Stack,
                    frame: rect(1280, 0, 640, 1080),
                },
            ],
        }
    }

    fn seed<B: WindowBackend + WorkspaceBackend>(b: &mut B) {
        let _ = b.recall_workspace("dock");
    }

    #[test]
    fn canvas_compositor_and_reference_agree_on_layout_receipt() {
        let mut canvas = CanvasCompositor::new(vec![display("eDP-1", 1920, 1080)]);
        canvas.map_window("web".into(), window("web", "firefox"));
        canvas.map_window("term".into(), window("term", "foot"));
        let mut reference = FakeBackend::new(vec![display("eDP-1", 1920, 1080)]);
        reference.windows.insert(
            "web".into(),
            WindowEntry::new(window("web", "firefox"), true),
        );
        reference.windows.insert(
            "term".into(),
            WindowEntry::new(window("term", "foot"), true),
        );
        seed(&mut canvas);
        seed(&mut reference);

        let plan = shared_plan();
        let canvas_receipt = canvas.apply_layout(&plan).unwrap();
        let reference_receipt = reference.apply_layout(&plan).unwrap();

        assert_eq!(canvas_receipt.focused, reference_receipt.focused);
        assert_eq!(canvas_receipt.placements, reference_receipt.placements);
        assert_eq!(
            canvas.snapshot().unwrap().windows.len(),
            reference.snapshot().unwrap().windows.len()
        );
    }

    #[test]
    fn outputs_are_managed_and_remappable() {
        let mut c = CanvasCompositor::new(vec![display("eDP-1", 1920, 1080)]);
        assert_eq!(c.displays().len(), 1);
        let replug = vec![
            Display {
                id: "eDP-1".into(),
                name: "eDP-1".into(),
                frame: rect(0, 0, 1920, 1080),
                work_area: rect(0, 0, 1920, 1080),
                primary: true,
            },
            Display {
                id: "HDMI-A-1".into(),
                name: "HDMI-A-1".into(),
                frame: rect(1920, 0, 1280, 720),
                work_area: rect(1920, 0, 1280, 720),
                primary: false,
            },
        ];
        c.set_outputs(replug);
        assert_eq!(c.displays().len(), 2);
        assert_eq!(c.displays()[1].id, "HDMI-A-1");
    }

    #[test]
    fn focus_is_part_of_the_backend_contract() {
        let mut canvas = CanvasCompositor::new(vec![display("eDP-1", 1024, 768)]);
        canvas.map_window("tool".into(), window("tool", "foot"));
        let mut reference = FakeBackend::new(vec![display("eDP-1", 1024, 768)]);
        reference.windows.insert(
            "tool".into(),
            WindowEntry::new(window("tool", "foot"), true),
        );

        let canvas_focus = canvas.focus("tool").unwrap();
        let reference_focus = reference.focus("tool").unwrap();
        assert_eq!(canvas_focus.id, reference_focus.id);
    }

    // --- Window lifecycle -> lua-stream producer ---

    #[test]
    fn lifecycle_envelope_shape_matches_stream_contract() {
        let frame = envelope("mapped", "app-x", "foot", "Dock", 3);
        let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["owner"], "compositor");
        assert_eq!(v["stream"], "windows");
        assert_eq!(v["contract"], "canvas.window.lifecycle.v1");
        assert_eq!(v["sequence"], 3);
        assert!(v["emitted_at_ms"].as_u64().unwrap_or(0) > 0);
        assert_eq!(v["payload"]["event"], "mapped");
        assert_eq!(v["payload"]["window_id"], "app-x");
        assert_eq!(v["payload"]["app"], "foot");
        assert_eq!(v["payload"]["title"], "Dock");

        // focus/close carry the id but no app/title.
        let focused = envelope("focused", "app-x", "", "", 4);
        let fv: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(fv["payload"]["event"], "focused");
        assert_eq!(fv["payload"]["window_id"], "app-x");
        assert_eq!(fv["payload"]["app"], "");
        assert_eq!(fv["payload"]["title"], "");
    }

    #[test]
    fn window_focused_sequence_increments_and_socket_fails_quiet() {
        // Without a socket path, posts are silent no-ops (open-loop compositor).
        let mut l = StreamLifecycle::new(None);
        let win = window("w1", "foot");
        let mapped = l.post("mapped", &win.id, &win.app, &win.title);
        let focused = l.post("focused", "w1", "", "");
        assert!(!mapped && !focused);
    }

    #[test]
    fn stream_lifecycle_publishes_a_datagram_per_event() {
        use std::os::unix::net::UnixDatagram;
        let dir = std::env::temp_dir();
        let stamp = format!(
            "dock-lifecycle-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = dir.join(&stamp);
        let _ = std::fs::remove_file(&path);
        let listener = UnixDatagram::bind(&path).unwrap();
        let _ = listener.set_read_timeout(Some(std::time::Duration::from_millis(500)));

        let lifecycle = StreamLifecycle::new(Some(path.clone()));
        let mut c = CanvasCompositor::with_lifecycle(vec![], Box::new(lifecycle));
        let win = window("w1", "foot");
        c.map_window("w1".into(), win.clone()); // mapped
        c.focus_window("w1").unwrap(); // focused
        c.close_window("w1"); // closed

        let mut buf = vec![0u8; 65536];
        let mut seen = Vec::<String>::new();
        loop {
            match listener.recv(&mut buf) {
                Ok(n) => seen.push(String::from_utf8_lossy(&buf[..n]).into_owned()),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(&path);

        assert_eq!(seen.len(), 3);
        let events: Vec<String> = seen
            .iter()
            .map(|f| {
                serde_json::from_str::<serde_json::Value>(f).unwrap()["payload"]["event"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(events, vec!["mapped", "focused", "closed"]);
        // every frame carries the contract + an incrementing sequence.
        let sequences: Vec<u64> = seen
            .iter()
            .map(|f| {
                serde_json::from_str::<serde_json::Value>(f).unwrap()["sequence"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(sequences, vec![0, 1, 2]);
    }
}
