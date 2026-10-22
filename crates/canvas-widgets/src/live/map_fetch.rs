//! Latest-view-only client of Galileo's independently compiled tile worker.
use fpl_gfx::{
    geo::Extent,
    geo_map::{
        tiles::{Request, Response},
        Map,
    },
    Rect,
};
use iced::widget::image::Handle;
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Snapshot {
    pub image: Option<Handle>,
    pub extent: Extent,
    pub status: String,
}
struct Shared {
    desired: Option<Request>,
    changed: Instant,
    snapshot: Option<Snapshot>,
}
pub(super) struct Client {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    enabled: bool,
}
impl Default for Client {
    fn default() -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            desired: None,
            changed: Instant::now(),
            snapshot: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let enabled = std::env::var_os("CANVAS_MAP_FETCH_BIN").is_some();
        if let (Some(bin), Some(cache), Some(assets)) = (
            std::env::var_os("CANVAS_MAP_FETCH_BIN"),
            std::env::var_os("CANVAS_MAP_FETCH_CACHE"),
            std::env::var_os("CANVAS_MAP_FETCH_ASSETS"),
        ) {
            let state = shared.clone();
            let stopped = stop.clone();
            std::thread::spawn(move || {
                run(state, stopped, bin.into(), cache.into(), assets.into())
            });
        }
        Self {
            shared,
            stop,
            enabled,
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Client {
    pub fn view(&self, extent: Extent, width: u32, height: u32) -> Option<Snapshot> {
        let mut state = self.shared.lock().expect("map fetch state poisoned");
        if state
            .desired
            .as_ref()
            .is_none_or(|r| r.extent != extent || r.width != width || r.height != height)
        {
            let request = Request {
                schema_version: 1,
                generation: state.desired.as_ref().map_or(1, |r| r.generation + 1),
                extent,
                width,
                height,
            };
            if request.validate().is_ok() {
                state.desired = Some(request);
                state.changed = Instant::now();
                if self.enabled {
                    if let Some(snapshot) = &mut state.snapshot {
                        snapshot.status = "Galileo · fetching current view…".into();
                    } else {
                        state.snapshot = Some(Snapshot {
                            image: None,
                            extent,
                            status: "Galileo · fetching current view…".into(),
                        });
                    }
                }
            }
        }
        state.snapshot.clone()
    }
}
fn current(shared: &Mutex<Shared>, request: &Request) -> bool {
    shared
        .lock()
        .expect("map fetch state poisoned")
        .desired
        .as_ref()
        == Some(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_view_does_not_enqueue_and_camera_change_supersedes_old_generation() {
        let client = Client {
            shared: Arc::new(Mutex::new(Shared {
                desired: None,
                changed: Instant::now(),
                snapshot: None,
            })),
            stop: Arc::new(AtomicBool::new(false)),
            enabled: true,
        };
        let extent = Extent {
            west_deg: -123.,
            east_deg: -121.,
            south_deg: 47.,
            north_deg: 49.,
        };
        client.view(extent, 1408, 881);
        let first = client.shared.lock().unwrap().desired.clone().unwrap();
        client.view(extent, 1408, 881);
        assert!(current(&client.shared, &first));
        client.view(extent, 1409, 881);
        assert!(!current(&client.shared, &first));
        assert_eq!(
            client
                .shared
                .lock()
                .unwrap()
                .desired
                .as_ref()
                .unwrap()
                .generation,
            2
        );
        client.view(extent, 9000, 9000);
        assert_eq!(
            client
                .shared
                .lock()
                .unwrap()
                .desired
                .as_ref()
                .unwrap()
                .generation,
            2
        );
    }
}
fn run(
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    bin: PathBuf,
    cache: PathBuf,
    assets: PathBuf,
) {
    let mut finished = 0;
    let mut retry = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
        let request = {
            let state = shared.lock().expect("map fetch state poisoned");
            state.desired.clone().filter(|r| {
                state.changed.elapsed() >= Duration::from_millis(300)
                    && (r.generation != finished || Instant::now() >= retry)
            })
        };
        let Some(request) = request else { continue };
        let result = fetch(&bin, &cache, &assets, &request, &shared, &stop);
        if stop.load(Ordering::Relaxed) || !current(&shared, &request) {
            continue;
        }
        let (snapshot, complete) = match result {
            Ok(response) => {
                let map = Map {
                    extent: request.extent,
                    markers: vec![],
                    paths: vec![],
                    selected: None,
                    basemap: response.basemap.clone(),
                };
                let extent = map
                    .inner(Rect::new(
                        0.,
                        0.,
                        request.width as f32,
                        request.height as f32,
                    ))
                    .and_then(|plot| {
                        fpl_gfx::geo_map::raster::cropped_extent(request.extent, plot)
                    });
                let image = if map.basemap.is_some() {
                    super::map_raster::retained(&map, request.width, request.height, &assets)
                        .map(Some)
                } else {
                    Ok(None)
                };
                match (extent, image) {
                    (Ok(extent), Ok(image)) => (
                        Snapshot {
                            image,
                            extent,
                            status: format!(
                                "Galileo · {} cached · {} fetched · {} pending{}",
                                response.cached,
                                response.fetched,
                                response.pending,
                                response
                                    .errors
                                    .first()
                                    .map(|e| format!(" · {e}"))
                                    .unwrap_or_default()
                            ),
                        },
                        response.pending == 0 && response.errors.is_empty(),
                    ),
                    _ => (
                        Snapshot {
                            image: None,
                            extent: request.extent,
                            status: "Galileo tile image rejected".into(),
                        },
                        false,
                    ),
                }
            }
            Err(error) => (
                Snapshot {
                    image: None,
                    extent: request.extent,
                    status: format!("Galileo · {error}"),
                },
                false,
            ),
        };
        let mut state = shared.lock().expect("map fetch state poisoned");
        if state.desired.as_ref() == Some(&request) {
            let mut snapshot = snapshot;
            if snapshot.image.is_none() {
                if let Some(previous) = &state.snapshot {
                    snapshot.image = previous.image.clone();
                    snapshot.extent = previous.extent;
                }
            }
            state.snapshot = Some(snapshot);
            finished = request.generation;
            retry = Instant::now()
                + if complete {
                    Duration::from_secs(7 * 86400)
                } else {
                    Duration::from_secs(5)
                };
        }
    }
}
fn fetch(
    bin: &PathBuf,
    cache: &PathBuf,
    assets: &PathBuf,
    request: &Request,
    shared: &Mutex<Shared>,
    stop: &AtomicBool,
) -> Result<Response, String> {
    let mut child = Command::new(bin)
        .arg(cache)
        .arg(assets)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("worker launch failed: {e}"))?;
    let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if let Err(error) = child.stdin.take().unwrap().write_all(&input) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("worker input failed: {error}"));
    }
    let output = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if stop.load(Ordering::Relaxed)
            || !current(shared, request)
            || start.elapsed() > Duration::from_secs(12)
        {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("view superseded or worker timed out".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("worker wait failed: {error}"));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "worker output reader failed")?
        .map_err(|e| e.to_string())?;
    if !status.success() || bytes.len() > 65536 {
        return Err("worker failed or exceeded output budget".into());
    }
    let response: Response =
        serde_json::from_slice(&bytes).map_err(|_| "invalid worker response")?;
    response.validate_for(request).map_err(|e| e.to_string())?;
    Ok(response)
}
