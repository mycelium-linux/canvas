//! Narrow contracts implemented by operating-system desktop adapters.

use crate::layout::{LayoutPlan, Placement, Rect};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub frame: Rect,
    pub work_area: Rect,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowSurface {
    pub id: String,
    pub process_id: Option<u32>,
    pub app: String,
    pub title: String,
    pub frame: Rect,
    pub movable: bool,
    pub resizable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopSnapshot {
    pub displays: Vec<Display>,
    pub windows: Vec<WindowSurface>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutReceipt {
    pub placements: Vec<Placement>,
    pub focused: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperFit {
    Fill,
    Fit,
    Stretch,
    Center,
    Tile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Wallpaper {
    Solid { rgb: [u8; 3] },
    ManagedImage { asset_id: String, fit: WallpaperFit },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WallpaperRequest {
    pub project: String,
    pub display_id: String,
    pub wallpaper: Wallpaper,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WallpaperReceipt {
    pub project: String,
    pub display_id: String,
    pub wallpaper: Wallpaper,
}

/// Inventory, focus, and geometry changes for already identified windows.
/// Permission-bearing helpers may implement this across a local IPC boundary.
pub trait WindowBackend {
    type Error;

    fn snapshot(&mut self) -> Result<DesktopSnapshot, Self::Error>;
    fn apply_layout(&mut self, plan: &LayoutPlan) -> Result<LayoutReceipt, Self::Error>;
    fn focus(&mut self, window_id: &str) -> Result<WindowSurface, Self::Error>;
}

/// Wallpaper changes are separate from window privileges and workspace policy.
pub trait WallpaperBackend {
    type Error;

    fn set_wallpaper(
        &mut self,
        request: &WallpaperRequest,
    ) -> Result<WallpaperReceipt, Self::Error>;
}

/// Native workspace selection remains optional because not every desktop has a
/// stable public workspace API.
pub trait WorkspaceBackend {
    type Error;

    fn recall_workspace(&mut self, project: &str) -> Result<String, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_contract_contains_no_backend_path_or_command() {
        let request = WallpaperRequest {
            project: "dock".into(),
            display_id: "display-1".into(),
            wallpaper: Wallpaper::Solid { rgb: [24, 25, 28] },
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["wallpaper"]["kind"], "solid");
        assert!(value.get("path").is_none());
        assert!(value.get("command").is_none());

        let image = serde_json::to_value(Wallpaper::ManagedImage {
            asset_id: "sha256:abc123".into(),
            fit: WallpaperFit::Fill,
        })
        .unwrap();
        assert_eq!(image["kind"], "managed_image");
        assert!(image.get("path").is_none());
    }
}
