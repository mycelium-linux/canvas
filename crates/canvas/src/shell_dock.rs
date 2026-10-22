//! Host-owned local application pins. Lua receives labels/state, never file paths.
use crate::shell_launcher::{Choice, Entry};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

pub const MAX_PINS: usize = 4;
const CONFIG: &str = "shell/dock-pins.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pins(pub Vec<String>);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    pins: Vec<String>,
}

impl Pins {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let doc: Document = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let mut seen = BTreeSet::new();
        if doc.schema_version != 1
            || doc.pins.len() > MAX_PINS
            || doc.pins.iter().any(|id| {
                id.len() > 1024
                    || id.chars().any(char::is_control)
                    || !id.starts_with("app:")
                    || !Path::new(&id[4..]).is_absolute()
                    || !seen.insert(id)
            })
        {
            return Err(
                "invalid dock pins: require unique catalog application IDs (maximum four)".into(),
            );
        }
        Ok(Self(doc.pins))
    }
    pub fn load() -> Result<Self, String> {
        crate::runtime_config::read(Path::new(CONFIG), 8192)
            .map_err(|e| e.to_string())?
            .map(|bytes| Self::parse(&bytes))
            .transpose()
            .map(|p| p.unwrap_or_default())
    }
    pub fn updated(&self, id: &str, pinned: bool, apps: &[Entry]) -> Result<Self, String> {
        let mut next = self.clone();
        if pinned {
            if !apps.iter().any(|e| {
                e.id == id && e.available && matches!(e.choice, Choice::Application { .. })
            }) {
                return Err("only an available catalog application can be pinned".into());
            }
            if !next.0.iter().any(|p| p == id) {
                next.0.push(id.into());
            }
        } else {
            next.0.retain(|p| p != id);
        }
        Self::parse(
            &serde_json::to_vec(&Document {
                schema_version: 1,
                pins: next.0,
            })
            .map_err(|e| e.to_string())?,
        )
    }
    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&Document {
            schema_version: 1,
            pins: self.0.clone(),
        })
        .map_err(|e| e.to_string())?;
        Self::parse(&bytes)?;
        crate::runtime_config::write(Path::new(CONFIG), &bytes, 8192)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Cache at load/pin changes, never from the per-frame view. Failures are visible
/// to the caller; the semantic icon remains a presentation fallback only.
pub fn icon(entry: &Entry) -> Result<Option<iced::widget::image::Handle>, String> {
    let Choice::Application { path } = &entry.choice else {
        return Ok(None);
    };
    #[cfg(target_os = "macos")]
    {
        let png = canvas_control::app_icon::png(path)?;
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(2048);
        limits.max_image_height = Some(2048);
        limits.max_alloc = Some(16 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().map_err(|e| e.to_string())?;
        let rgba = image
            .resize(128, 128, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        Ok(Some(iced::widget::image::Handle::from_rgba(
            rgba.width(),
            rgba.height(),
            rgba.into_raw(),
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(id: &str) -> Entry {
        Entry {
            id: id.into(),
            title: "App".into(),
            detail: "Native".into(),
            available: true,
            choice: Choice::Application {
                path: id[4..].into(),
            },
        }
    }
    #[test]
    fn pins_are_ordered_idempotent_and_removable_after_uninstall() {
        let a = app("app:/Applications/A.app");
        let b = app("app:/Applications/B.app");
        let pins = Pins::default()
            .updated(&b.id, true, &[a.clone(), b.clone()])
            .unwrap()
            .updated(&a.id, true, &[a.clone(), b.clone()])
            .unwrap();
        assert_eq!(pins.0, vec![b.id.clone(), a.id.clone()]);
        assert_eq!(pins.updated(&a.id, true, &[a.clone()]).unwrap(), pins);
        assert_eq!(pins.updated(&a.id, false, &[]).unwrap().0, vec![b.id]);
        assert!(
            Pins::default()
                .updated("app:/Applications/Missing.app", true, &[])
                .is_err()
        );
    }
    #[test]
    fn rejects_unbounded_duplicate_and_foreign_entries() {
        for pins in [
            vec!["surface:clock"],
            vec!["app:relative.app"],
            vec!["app:/a", "app:/a"],
            vec!["app:/a", "app:/b", "app:/c", "app:/d", "app:/e"],
        ] {
            let bytes =
                serde_json::to_vec(&serde_json::json!({"schema_version":1,"pins":pins})).unwrap();
            assert!(Pins::parse(&bytes).is_err());
        }
        assert!(Pins::parse(br#"{"schema_version":2,"pins":[]}"#).is_err());
    }
}
