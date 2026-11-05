//! Native seat observation off the UI thread; Lua is presentation only.
use crate::{
    lua_module::{ModuleInputSnapshot, ModuleInputValue, ModuleThemeTokens},
    shell_presentation::{Artifact, Presentation},
};
use canvas_core::layout::Rect;
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const ARTIFACT: Artifact = Artifact {
    manifest_path: "modules/native-switcher-v1.json",
    source_path: "modules/native-switcher-v1.lua",
    manifest_fallback: include_str!("../../../config/modules/native-switcher-v1.json"),
    source_fallback: include_str!("../../../config/modules/native-switcher-v1.lua"),
    region: Some("switcher"),
};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Window {
    pub id: String,
    pub app: String,
    pub title: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Switcher {
    pub active: bool,
    pub windows: Vec<String>,
    pub selected_window: Option<String>,
    pub frame: Rect,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Snapshot {
    pub switcher: Switcher,
    pub windows: Vec<Window>,
}

pub struct Host {
    latest: Arc<Mutex<Option<Result<Snapshot, String>>>>,
    stop: Arc<AtomicBool>,
    pub window: Option<iced::window::Id>,
    pub snapshot: Option<Snapshot>,
    pub presentation: Presentation,
    last_reload: Instant,
    last_error: Option<String>,
    revision: u64,
    observed_ms: u64,
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Host {
    /// Explicit opt-in; never attach a Mac or another compositor by guessing a path.
    pub fn from_environment() -> Option<Self> {
        std::env::var_os("CANVAS_NATIVE_CONTROL_SOCKET")
            .map(PathBuf::from)
            .map(Self::start)
    }
    pub fn start(socket: PathBuf) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, stopping) = (latest.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let result = canvas_core::native_control::send_command(
                    &socket,
                    &serde_json::json!({"command":"list"}),
                )
                .and_then(|text| {
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    if value["ok"] != true {
                        return Err(format!("native switcher: {}", value["error"]));
                    }
                    serde_json::from_value(value["result"].clone()).map_err(|e| e.to_string())
                });
                let failed = result.is_err();
                *out.lock().expect("native switcher mailbox") = Some(result);
                std::thread::sleep(if failed {
                    Duration::from_secs(1)
                } else {
                    Duration::from_millis(40)
                });
            }
        });
        Self {
            latest,
            stop,
            window: None,
            snapshot: None,
            presentation: Default::default(),
            last_reload: Instant::now() - Duration::from_secs(1),
            last_error: None,
            revision: 0,
            observed_ms: 0,
        }
    }
    pub fn refresh(&mut self, theme: ModuleThemeTokens) -> Result<bool, String> {
        let update = self.latest.lock().map_err(|e| e.to_string())?.take();
        if let Some(result) = update {
            match result {
                Ok(snapshot) => {
                    if self.snapshot.as_ref() != Some(&snapshot) {
                        self.revision = self.revision.saturating_add(1);
                        self.observed_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_err(|e| e.to_string())?
                            .as_millis() as u64;
                    }
                    self.snapshot = Some(snapshot);
                    self.last_error = None;
                }
                Err(error) => {
                    if self.last_error.as_ref() != Some(&error) {
                        eprintln!("native switcher disconnected: {error}");
                    }
                    self.last_error = Some(error);
                }
            }
        }
        let Some(snapshot) = &self.snapshot else {
            return Ok(false);
        };
        let inputs = snapshot.inputs(self.last_error.as_deref(), self.revision, self.observed_ms);
        let reload = self.last_reload.elapsed() >= Duration::from_secs(1);
        let result = if reload {
            self.last_reload = Instant::now();
            self.presentation.refresh_files(ARTIFACT, inputs, theme)
        } else {
            self.presentation.refresh_current(ARTIFACT, inputs, theme)
        };
        result.map_err(|e| e.to_string())
    }
}
impl Snapshot {
    fn inputs(&self, error: Option<&str>, revision: u64, now_ms: u64) -> ModuleInputSnapshot {
        let mut values = std::collections::BTreeMap::from([
            (
                "width".into(),
                ModuleInputValue::Number(f64::from(self.switcher.frame.width)),
            ),
            (
                "height".into(),
                ModuleInputValue::Number(f64::from(self.switcher.frame.height)),
            ),
            (
                "health".into(),
                ModuleInputValue::Text(
                    if error.is_some() {
                        "Disconnected · last observed selection"
                    } else {
                        "Alt + Tab · release Alt to select"
                    }
                    .into(),
                ),
            ),
        ]);
        // Keep the selected window visible even with more than six native apps.
        let selected = self
            .switcher
            .windows
            .iter()
            .position(|id| Some(id) == self.switcher.selected_window.as_ref())
            .unwrap_or(0);
        let start = selected
            .saturating_sub(2)
            .min(self.switcher.windows.len().saturating_sub(6));
        for (slot, id) in self.switcher.windows.iter().skip(start).take(6).enumerate() {
            let Some(window) = self.windows.iter().find(|window| &window.id == id) else {
                continue;
            };
            let key = format!("row{slot}");
            values.insert(
                format!("{key}-title"),
                ModuleInputValue::Text(window.title.chars().take(34).collect()),
            );
            values.insert(
                format!("{key}-detail"),
                ModuleInputValue::Text(window.app.chars().take(22).collect()),
            );
            values.insert(
                format!("{key}-selected"),
                ModuleInputValue::Boolean(Some(id) == self.switcher.selected_window.as_ref()),
            );
        }
        ModuleInputSnapshot {
            revision,
            now_ms,
            values,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_window_stays_visible_without_reordering_the_gesture() {
        let windows: Vec<_> = (0..12)
            .map(|i| Window {
                id: i.to_string(),
                app: "foot".into(),
                title: format!("Terminal {i}"),
            })
            .collect();
        let snapshot = Snapshot {
            switcher: Switcher {
                active: true,
                windows: windows.iter().map(|w| w.id.clone()).collect(),
                selected_window: Some("10".into()),
                frame: Rect {
                    x: 0,
                    y: 0,
                    width: 900,
                    height: 180,
                },
            },
            windows,
        };
        let input = snapshot.inputs(None, 1, 1_768_000_000_000);
        assert_eq!(
            input.values["row4-selected"],
            ModuleInputValue::Boolean(true)
        );
        assert_eq!(
            input.values["row4-title"],
            ModuleInputValue::Text("Terminal 10".into())
        );
        let mut presentation = Presentation::default();
        assert!(
            presentation
                .refresh_current(
                    ARTIFACT,
                    input,
                    canvas_core::ProjectTheme {
                        mode: canvas_core::ThemeMode::Dark,
                        accent: canvas_core::Accent::Info
                    }
                    .into()
                )
                .unwrap()
        );
        assert!(presentation.retained().unwrap().buttons.is_empty());
    }
}
