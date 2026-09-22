use anyhow::{Context, Result, bail};
use canvas_core::VisualTheme;
use canvas_core::{Accent, ThemeMode, ThemeSelection};
use canvas_model_host::{CameraFlight, Material, Orbit, SceneContext, SceneState, lua, mesh, renderer::Renderer};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone)]
struct RenderJob {
    sequence: u64,
    flight: CameraFlight,
    orbit: Option<Orbit>,
    rotation: [f32; 3],
    material: Material,
    theme: VisualTheme,
    elapsed: f32,
}

enum WorkerCommand {
    Render(RenderJob),
    Upload(Arc<mesh::PreparedMesh>),
}

struct RenderResult {
    sequence: u64,
    frame: Result<Vec<u8>>,
}

struct RenderPool {
    workers: Vec<SyncSender<WorkerCommand>>,
    results: Receiver<RenderResult>,
    next_worker: usize,
}

impl RenderPool {
    fn new(width: u32, height: u32, mesh: Arc<mesh::PreparedMesh>, count: usize) -> Result<Self> {
        let (result_tx, results) = mpsc::sync_channel(count * 2);
        let mut workers = Vec::with_capacity(count);
        for index in 0..count {
            let mut renderer = Renderer::new(width, height)?;
            renderer.upload(&mesh);
            let (tx, rx) = mpsc::sync_channel::<WorkerCommand>(2);
            let output = result_tx.clone();
            thread::Builder::new()
                .name(format!("canvas-model-render-{index}"))
                .spawn(move || {
                    let mut sequences = VecDeque::new();
                    while let Ok(command) = rx.recv() {
                        match command {
                            WorkerCommand::Upload(mesh) => renderer.upload(&mesh),
                            WorkerCommand::Render(job) => {
                                sequences.push_back(job.sequence);
                                match renderer.render(
                                    job.flight,
                                    job.orbit,
                                    job.rotation,
                                    &job.material,
                                    &job.theme,
                                    job.elapsed,
                                ) {
                                    Ok(Some(frame)) => {
                                        if let Some(sequence) = sequences.pop_front()
                                            && output
                                                .send(RenderResult {
                                                    sequence,
                                                    frame: Ok(frame),
                                                })
                                                .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        let sequence =
                                            sequences.pop_front().unwrap_or(job.sequence);
                                        if output
                                            .send(RenderResult {
                                                sequence,
                                                frame: Err(error),
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                })
                .context("spawn model render worker")?;
            workers.push(tx);
        }
        Ok(Self {
            workers,
            results,
            next_worker: 0,
        })
    }

    fn submit(&mut self, job: RenderJob) -> Result<()> {
        let index = self.next_worker;
        self.next_worker = (index + 1) % self.workers.len();
        self.workers[index]
            .send(WorkerCommand::Render(job))
            .context("model render worker stopped")
    }

    fn upload(&self, mesh: Arc<mesh::PreparedMesh>) -> Result<()> {
        for worker in &self.workers {
            worker
                .send(WorkerCommand::Upload(Arc::clone(&mesh)))
                .context("model render worker stopped")?;
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 7 {
        bail!(
            "usage: canvas-model-host <scene.lua> <asset-root> <endpoint> <width> <height> <fps> <videotoolbox|nvenc|vaapi>"
        );
    }
    let scene_path = PathBuf::from(&args[0]);
    let asset_root = PathBuf::from(&args[1]);
    let endpoint = &args[2];
    let width: u32 = args[3].parse().context("invalid width")?;
    let height: u32 = args[4].parse().context("invalid height")?;
    let fps: u32 = args[5].parse().context("invalid fps")?;
    anyhow::ensure!((1..=30).contains(&fps), "fps must be 1..=30");

    // A scene cannot look at the asset root itself, so the host enumerates it
    // and re-enumerates whenever the set changes — that is what lets a scene
    // follow a build without anything rewriting the scene file per step.
    let mut assets = SceneContext::scan(&asset_root);
    let mut scene = read_scene(&scene_path, &assets)?;
    let mut scene_stamp = modified(&scene_path)?;
    let workers = std::thread::available_parallelism()
        .map_or(2, usize::from)
        .clamp(1, 8);
    let pool = mesh::BakePool::new(workers);
    let mut generation = 1;
    pool.submit(mesh::BakeRequest {
        generation,
        asset: scene.resolve_asset(&asset_root)?,
    })?;
    let baked = pool
        .results
        .recv_timeout(Duration::from_secs(60))
        .context("initial model bake timed out")?
        .result?;
    let render_workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(3, 6);
    let mut renderers = RenderPool::new(width, height, Arc::new(baked), render_workers)?;
    let mut encoder = spawn_encoder(endpoint, width, height, fps, &args[6])?;
    let mut stdin = encoder.stdin.take().context("encoder stdin unavailable")?;
    let theme = ThemeSelection::coherence(ThemeMode::Dark, Accent::Info).visual_theme();
    let mut flight = CameraFlight::new(
        scene.camera.direction(),
        scene.camera,
        scene.transition_ms,
        scene.fly_in,
    );
    let mut transition_started = Instant::now();
    let started = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut frame_count = 0u64;
    let mut submitted = 0u64;
    let mut next_output = 0u64;
    let mut completed = BTreeMap::new();
    let mut measured_at = Instant::now();
    // Twice a second is well inside human reaction time for a build landing a
    // step, and keeps a directory listing off the frame path.
    let asset_scan_interval = Duration::from_millis(500);
    let mut asset_scanned_at = Instant::now();

    loop {
        let frame_started = Instant::now();
        let mut assets_changed = false;
        if asset_scanned_at.elapsed() >= asset_scan_interval {
            asset_scanned_at = Instant::now();
            let next_assets = SceneContext::scan(&asset_root);
            assets_changed = next_assets != assets;
            if assets_changed {
                assets = next_assets;
            }
        }
        if assets_changed || modified(&scene_path).is_ok_and(|stamp| stamp > scene_stamp) {
            match read_scene(&scene_path, &assets) {
                Ok(next) => {
                    scene_stamp = modified(&scene_path)?;
                    let asset_changed = next.asset != scene.asset;
                    flight = CameraFlight::new(
                        scene.camera.direction(),
                        next.camera,
                        next.transition_ms,
                        asset_changed && next.fly_in,
                    );
                    transition_started = Instant::now();
                    if asset_changed {
                        generation += 1;
                        if let Err(error) = pool.submit(mesh::BakeRequest {
                            generation,
                            asset: next.resolve_asset(&asset_root)?,
                        }) {
                            eprintln!("canvas-model-host: {error}");
                        }
                    }
                    scene = next;
                }
                Err(error) => eprintln!("canvas-model-host: scene reload rejected: {error:#}"),
            }
        }
        while let Ok(baked) = pool.results.try_recv() {
            if baked.generation == generation {
                match baked.result {
                    Ok(mesh) => renderers.upload(Arc::new(mesh))?,
                    Err(error) => eprintln!("canvas-model-host: model bake rejected: {error:#}"),
                }
            }
        }
        renderers.submit(RenderJob {
            sequence: submitted,
            flight,
            orbit: scene.orbit,
            rotation: scene.model_rotation_degrees,
            material: scene.material.clone(),
            theme,
            elapsed: transition_started
                .elapsed()
                .as_secs_f32()
                .min(started.elapsed().as_secs_f32()),
        })?;
        submitted += 1;
        while let Ok(result) = renderers.results.try_recv() {
            completed.insert(result.sequence, result.frame);
        }
        while let Some(frame) = completed.remove(&next_output) {
            let frame = frame?;
            if let Err(error) = stdin.write_all(&frame) {
                let _ = encoder.kill();
                return Err(error).context("write RGBA frame to hardware encoder");
            }
            next_output += 1;
            frame_count += 1;
            if frame_count % 120 == 0 {
                let elapsed = measured_at.elapsed().as_secs_f64();
                eprintln!("canvas-model-host: {:.1} fps", 120.0 / elapsed.max(0.001));
                measured_at = Instant::now();
            }
        }
        if let Some(remaining) = frame_interval.checked_sub(frame_started.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
}

fn read_scene(path: &Path, assets: &SceneContext) -> Result<SceneState> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    lua::evaluate_with(&source, assets)
}

fn modified(path: &Path) -> Result<SystemTime> {
    std::fs::metadata(path)?.modified().map_err(Into::into)
}

fn spawn_encoder(
    endpoint: &str,
    width: u32,
    height: u32,
    fps: u32,
    encoder: &str,
) -> Result<Child> {
    anyhow::ensure!(
        endpoint.starts_with("tcp://") || endpoint.starts_with("udp://"),
        "endpoint must be tcp:// or udp://"
    );
    anyhow::ensure!(
        !endpoint.contains(['\n', '\r', '\0']),
        "endpoint contains control characters"
    );
    let size = format!("{width}x{height}");
    let rate = fps.to_string();
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostdin",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgba",
        "-video_size",
        &size,
        "-framerate",
        &rate,
        "-i",
        "pipe:0",
        "-an",
        "-vf",
        "format=nv12",
    ]);
    match encoder {
        "videotoolbox" => {
            command.args([
                "-c:v",
                "h264_videotoolbox",
                "-allow_sw",
                "0",
                "-realtime",
                "1",
            ]);
        }
        "nvenc" => {
            command.args(["-c:v", "h264_nvenc", "-preset", "p1", "-tune", "ull"]);
        }
        "vaapi" => {
            command.args(["-vaapi_device", "/dev/dri/renderD128", "-c:v", "h264_vaapi"]);
        }
        other => bail!("unknown encoder '{other}'"),
    }
    command
        .args([
            "-g",
            &rate,
            "-bf",
            "0",
            "-b:v",
            "1800k",
            "-maxrate",
            "2400k",
            "-bufsize",
            "3600k",
            "-mpegts_flags",
            "+resend_headers",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "-flush_packets",
            "1",
            "-f",
            "mpegts",
            endpoint,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn FFmpeg hardware encoder")
}
