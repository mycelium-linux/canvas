//! Owner-local ingress for producer-owned GPU surfaces.
//!
//! The document keeps only a stable source id. This adapter owns the volatile
//! socket and native handles, while `canvas-model-viewer` owns importing or
//! uploading the accepted generation into the compositor's WGPU device.

use canvas_model_viewer::external_surface::{ExternalSurfaceFrame, ExternalSurfaceInbox};
use std::{
    fs,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

#[derive(Debug, Clone, Default)]
pub struct ExternalSurfaceFeed {
    inbox: Arc<RwLock<ExternalSurfaceInbox>>,
}

impl ExternalSurfaceFeed {
    pub fn frame(&self, source_id: &str) -> Option<ExternalSurfaceFrame> {
        self.inbox
            .read()
            .expect("Canvas external-surface inbox poisoned")
            .frame(source_id)
    }
}

#[derive(Debug)]
pub struct ExternalSurfaceSocketGuard {
    path: PathBuf,
}

impl Drop for ExternalSurfaceSocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() }
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn start_server() -> Result<(ExternalSurfaceFeed, ExternalSurfaceSocketGuard), String> {
    let path = canvas_core::external_surface_socket_path()
        .ok_or("Canvas external-surface socket home is not configured")?;
    start_server_at(path)
}

fn start_server_at(
    path: PathBuf,
) -> Result<(ExternalSurfaceFeed, ExternalSurfaceSocketGuard), String> {
    prepare_parent(&path)?;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("refusing to replace unsafe Canvas external-surface socket".into());
        }
        fs::remove_file(&path).map_err(|error| error.to_string())?;
    }
    let socket = UnixDatagram::bind(&path).map_err(|error| error.to_string())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    let feed = ExternalSurfaceFeed::default();
    let receiver = feed.clone();
    std::thread::Builder::new()
        .name("canvas-external-surfaces".into())
        .spawn(move || receive(socket, receiver))
        .map_err(|error| error.to_string())?;
    Ok((feed, ExternalSurfaceSocketGuard { path }))
}

fn prepare_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Canvas external-surface socket has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Canvas external-surface socket parent is unsafe".into());
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())
}

fn receive(socket: UnixDatagram, feed: ExternalSurfaceFeed) {
    let mut consecutive_failures = 0_u32;
    loop {
        match canvas_core::external_surface_ipc::receive(&socket) {
            Ok((message, handles)) => {
                consecutive_failures = 0;
                if let Err(error) = feed
                    .inbox
                    .write()
                    .expect("Canvas external-surface inbox poisoned")
                    .apply(message, handles)
                {
                    eprintln!("Canvas rejected external surface: {error}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                eprintln!(
                    "Canvas external-surface receive failed ({consecutive_failures} in a row): {error}"
                );
                std::thread::sleep(Duration::from_millis(
                    (50 * u64::from(consecutive_failures)).min(1_000),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_existing_path_is_not_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("surface.sock");
        fs::write(&path, b"not a socket").unwrap();
        let error = start_server_at(path.clone()).unwrap_err();
        assert!(error.contains("unsafe"));
        assert_eq!(fs::read(path).unwrap(), b"not a socket");
    }
}
