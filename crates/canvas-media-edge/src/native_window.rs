//! Shared native-window selection and pixel extent planning for local and remote capture.
use anyhow::{Context, Result, ensure};
use screencapturekit::{
    prelude::*,
    shareable_content::{ContentSnapshot, SCShareableContent},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSelector {
    #[serde(default)]
    pub window_id: Option<u32>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub title_contains: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub window_id: u32,
    pub bundle_id: String,
    pub app_name: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
}

pub fn select_window(
    content: &SCShareableContent,
    selector: &WindowSelector,
) -> Result<(Candidate, SCWindow)> {
    let snapshot = content.snapshot().context("snapshot shareable windows")?;
    let candidates = matching_candidates(&snapshot, selector);
    let candidate = require_unique(candidates)?;
    let window = content
        .windows()
        .into_iter()
        .find(|window| window.window_id() == candidate.window_id)
        .context("selected window vanished")?;
    Ok((candidate, window))
}

pub fn require_unique(candidates: Vec<Candidate>) -> Result<Candidate> {
    ensure!(!candidates.is_empty(), "no window matches selector");
    ensure!(
        candidates.len() == 1,
        "selector is ambiguous; matches: {}",
        candidates
            .iter()
            .map(|window| format!("{}:{}", window.window_id, window.title))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(candidates.into_iter().next().expect("checked non-empty"))
}

pub fn capture_dimensions(
    width_points: f64,
    height_points: f64,
    capture_scale: f64,
    max_height: u32,
) -> (u32, u32) {
    let height = max_height
        .min((height_points * capture_scale).round() as u32)
        .max(2)
        & !1;
    let width = (((height as f64 * width_points / height_points).round() as u32).max(2)) & !1;
    (width, height)
}

pub fn matching_candidates(
    snapshot: &ContentSnapshot,
    selector: &WindowSelector,
) -> Vec<Candidate> {
    snapshot
        .windows
        .iter()
        .filter_map(|window| {
            if selector.window_id.is_some_and(|id| id != window.window_id) {
                return None;
            }
            let app = window
                .owning_app_index
                .and_then(|index| snapshot.applications.get(index))?;
            if selector
                .bundle_id
                .as_deref()
                .is_some_and(|id| id != app.bundle_identifier)
            {
                return None;
            }
            let title = window.title.clone().unwrap_or_default();
            if selector
                .title_contains
                .as_deref()
                .is_some_and(|needle| !title.to_lowercase().contains(&needle.to_lowercase()))
            {
                return None;
            }
            (window.frame.size.width >= 64.0 && window.frame.size.height >= 64.0).then(|| {
                Candidate {
                    window_id: window.window_id,
                    bundle_id: app.bundle_identifier.clone(),
                    app_name: app.application_name.clone(),
                    title,
                    width: window.frame.size.width,
                    height: window.frame.size.height,
                }
            })
        })
        .collect()
}

pub fn list_windows() -> Result<()> {
    let content = SCShareableContent::get()?;
    let snapshot = content.snapshot().context("snapshot shareable windows")?;
    for window in &snapshot.windows {
        let Some(app) = window
            .owning_app_index
            .and_then(|index| snapshot.applications.get(index))
        else {
            continue;
        };
        if window.frame.size.width < 64.0 || window.frame.size.height < 64.0 {
            continue;
        }
        println!(
            "{}\t{}\t{}\t{}\t{}x{}\t{}",
            window.window_id,
            app.bundle_identifier,
            app.application_name,
            window.title.as_deref().unwrap_or(""),
            window.frame.size.width,
            window.frame.size.height,
            if window.is_on_screen {
                "visible"
            } else {
                "obscured"
            }
        );
    }
    Ok(())
}
