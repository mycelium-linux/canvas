//! Surface state types: content sources, runtime surfaces, and loading results.
//! Extracted from main.rs for module clarity (dock-q26.14).

use std::path::PathBuf;
use std::time::Instant;

use dock_canvas::control::{SurfaceSnapshot, PresencePreview};
use dock_canvas::media::{MediaPlan, MediaCacheKey, MediaVariant, PixelSize, SourceDimensions};
use dock_canvas::Scene;
use dock_canvas::AssetRef;
use iced::widget::image::Handle;
use iced::widget::markdown;

#[derive(Debug, Clone)]
pub struct LoadedContent {
    pub snapshot: SurfaceSnapshot,
    pub source: ContentSource,
    pub handle: Handle,
    pub asset: AssetRef,
    pub source_pixels: [u32; 2],
    pub media_plan: MediaPlan,
    pub upload_pixels: [u32; 2],
}

#[derive(Debug, Clone)]
pub enum ContentSource {
    Image {
        _path: PathBuf,
    },
    Pdf {
        path: PathBuf,
        page_index: u32,
        page_count: u32,
    },
    Markdown {
        #[allow(dead_code)]
        path: PathBuf,
        items: Vec<markdown::Item>,
    },
    Terminal {
        #[allow(dead_code)]
        master_fd: i32,
        stdin_fd: i32,
        #[allow(dead_code)]
        pid: u32,
        buffer: String,
    },
    Scene {
        scene: Scene,
    },
    DataViz {
        data: Vec<(f32, f32)>,
        chart_type: String,
    },
    Stream {
        url: String,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeSurface {
    pub snapshot: SurfaceSnapshot,
    pub source: ContentSource,
    pub handle: Handle,
    pub source_pixels: [u32; 2],
    pub media_plan: MediaPlan,
    pub upload_pixels: [u32; 2],
}

pub struct PresencePreviewVisual {
    pub value: PresencePreview,
    pub handle: Handle,
    pub started: Instant,
    pub last_frame: Instant,
}

pub type ContentLoadResult = Result<LoadedContent, crate::Failure>;
pub type PageLoadResult = Result<(Handle, [u32; 2], MediaPlan), crate::Failure>;
