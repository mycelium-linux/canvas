//! Preview-only persistence through eframe's host storage. Components own no filesystem paths.
use crate::shell::Shell;
const STORAGE_KEY: &str = "canvas.mock-workspace.v1";
pub struct App {
    pub shell: Shell,
    storage_error: Option<String>,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let shell = match cc.storage.and_then(|s| s.get_string(STORAGE_KEY)) {
            Some(json) => Shell::restore_preview(&json)?,
            None => Shell::demo()?,
        };
        Ok(Self {
            shell,
            storage_error: None,
        })
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH) {
            Ok(now) => {
                self.shell.owner.advance_to(now.as_secs());
            }
            Err(error) => self.storage_error = Some(format!("Clock error: {error}")),
        }
        self.shell.show(ctx);
        if let Some(error) = &self.storage_error {
            egui::Window::new("Preview storage error").show(ctx, |ui| {
                ui.label(error);
            });
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        match self.shell.save_preview() {
            Ok(json) => {
                storage.set_string(STORAGE_KEY, json);
                self.storage_error = None;
            }
            Err(error) => self.storage_error = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct MemoryStorage(std::collections::HashMap<String, String>);
    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn flush(&mut self) {}
    }
    #[test]
    fn host_checkpoint_roundtrips_and_invalid_state_cannot_replace_it() {
        use eframe::App as _;
        let shell = Shell::demo().unwrap();
        let mut app = App {
            shell,
            storage_error: None,
        };
        let mut storage = MemoryStorage::default();
        app.save(&mut storage);
        let checkpoint = storage.0[STORAGE_KEY].clone();
        assert_eq!(
            Shell::restore_preview(&checkpoint)
                .unwrap()
                .owner
                .threads
                .len(),
            48
        );
        app.shell.owner.threads[0].id.account = "unknown@example.com".into();
        app.save(&mut storage);
        assert_eq!(storage.0[STORAGE_KEY], checkpoint);
        assert!(app.storage_error.is_some());
        assert!(Shell::restore_preview("{broken").is_err());
    }
}
