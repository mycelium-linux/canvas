use anyhow::{Context, Result, bail, ensure};
use canvas::{
    backend::SurfaceMetrics,
    map::{
        globe::GlobeCamera,
        spatial::{SpatialPalette, decode_panopticon_snapshot},
    },
};
use canvas_core::{ThemePalette, ThemeSelection};
use canvas_map_wgpu::GlobeRenderer;
use serde::Deserialize;
use std::{
    io::Write,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const BYTES_PER_PIXEL: u32 = 4;
#[path = "../camera_tour.rs"]
mod camera_tour;
#[path = "../live_snapshot.rs"]
mod live_snapshot;
#[path = "../tile_stream.rs"]
mod tile_stream;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    width: u32,
    height: u32,
    frames_per_second: u32,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    ffmpeg_binary: String,
    /// None is an explicit basemap-only presentation, with no synthetic tracks.
    #[serde(default)]
    snapshot_path: Option<String>,
    #[serde(default)]
    live_snapshot: Option<live_snapshot::Source>,
    #[serde(default)]
    raster_path: Option<String>,
    #[serde(default)]
    detail_tile_template: Option<String>,
    theme: StreamTheme,
    camera: CameraConfig,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StreamTheme {
    Selection(ThemeSelection),
    Palette(ThemePalette),
}

impl StreamTheme {
    fn palette(&self) -> ThemePalette {
        match self {
            Self::Selection(selection) => selection.palette(),
            Self::Palette(palette) => *palette,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CameraConfig {
    #[serde(default)]
    tour: Option<camera_tour::Tour>,
    latitude: f64,
    longitude: f64,
    distance_m: f64,
    pitch_degrees: f64,
    orbit_degrees_per_second: f64,
}

struct Encoder {
    child: Child,
    input: ChildStdin,
}

impl Encoder {
    fn spawn(config: &Config) -> Result<Self> {
        let size = format!("{}x{}", config.width, config.height);
        let rate = config.frames_per_second.to_string();
        let bitrate =
            ((config.width as u64 * config.height as u64 * config.frames_per_second as u64) / 85)
                .clamp(1_500_000, 12_000_000);
        let bitrate = bitrate.to_string();
        let endpoint = config.endpoint.as_deref().context("missing UDP endpoint")?;
        let mut child = Command::new(&config.ffmpeg_binary)
            .args([
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
                "-c:v",
                "h264_videotoolbox",
                "-allow_sw",
                "1",
                "-realtime",
                "1",
                "-g",
                &rate,
                "-bf",
                "0",
                "-b:v",
                &bitrate,
                "-muxdelay",
                "0",
                "-muxpreload",
                "0",
                "-flush_packets",
                "1",
                "-mpegts_flags",
                "+resend_headers",
                "-f",
                "mpegts",
                endpoint,
            ])
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("start {}", config.ffmpeg_binary))?;
        let input = child.stdin.take().context("FFmpeg stdin unavailable")?;
        Ok(Self { child, input })
    }

    fn write_frame(&mut self, rgba: &[u8]) -> Result<()> {
        if let Some(status) = self.child.try_wait().context("query FFmpeg")? {
            bail!("FFmpeg exited unexpectedly with {status}");
        }
        self.input.write_all(rgba).context("write frame to FFmpeg")
    }
}

fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .context("usage: canvas-map-stream <config.json>")?;
    let config: Config = serde_json::from_slice(
        &std::fs::read(&config_path).with_context(|| format!("read {config_path}"))?,
    )
    .context("parse stream config")?;
    validate(&config)?;
    let snapshot = config
        .snapshot_path
        .as_ref()
        .map(|path| {
            decode_panopticon_snapshot(
                &std::fs::read(path).with_context(|| format!("read {path}"))?,
            )
            .map_err(anyhow::Error::msg)
        })
        .transpose()?;

    pollster::block_on(run(config, snapshot))
}

fn validate(config: &Config) -> Result<()> {
    if let Some(tour) = &config.camera.tour {
        tour.validate().map_err(anyhow::Error::msg)?;
    }
    ensure!(
        config.snapshot_path.is_none() || config.live_snapshot.is_none(),
        "choose a file snapshot or live owner, not both"
    );
    ensure!(
        config.width >= 64 && config.width <= 7_680,
        "width must be 64..=7680"
    );
    ensure!(
        config.height >= 64 && config.height <= 4_320,
        "height must be 64..=4320"
    );
    ensure!(
        config.width % 2 == 0 && config.height % 2 == 0,
        "H.264 dimensions must be even"
    );
    ensure!(
        (1..=60).contains(&config.frames_per_second),
        "frames_per_second must be 1..=60"
    );
    ensure!(
        config.endpoint.is_some() ^ config.source_id.is_some(),
        "set exactly one of endpoint or source_id"
    );
    if let Some(endpoint) = &config.endpoint {
        ensure!(endpoint.starts_with("udp://"), "endpoint must use udp://");
    }
    ensure!(
        config.camera.orbit_degrees_per_second.is_finite(),
        "orbit speed must be finite"
    );
    Ok(())
}

async fn run(
    config: Config,
    snapshot: Option<canvas::map::spatial::SpatialFeatureSet>,
) -> Result<()> {
    let live = config.live_snapshot.as_ref().map(live_snapshot::start);
    let tiles = config.detail_tile_template.clone().map(tile_stream::start);
    let mut next_tile_request = Instant::now();
    let mut active_snapshot = snapshot.clone();
    let local_frame_path = config
        .source_id
        .as_deref()
        .map(|source_id| {
            canvas_core::video_frame_file(source_id)
                .with_context(|| format!("invalid Canvas video source id {source_id:?}"))
        })
        .transpose()?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .context("no WGPU adapter available")?;
    let adapter_info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Panopticon globe stream"),
            ..Default::default()
        })
        .await
        .context("request WGPU device")?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Panopticon globe stream target"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let unpadded_row = config.width * BYTES_PER_PIXEL;
    let padded_row = unpadded_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Panopticon globe stream readback"),
        size: u64::from(padded_row) * u64::from(config.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut renderer = GlobeRenderer::new(&device, format);
    if let Some(snapshot) = &snapshot {
        renderer.set_features(&device, snapshot)?;
        eprintln!(
            "canvas-map-stream: snapshot revision={} features={}",
            snapshot.revision,
            snapshot.features.len()
        );
    } else if live.is_some() {
        eprintln!("canvas-map-stream: waiting for live spatial owner; basemap until connected");
    } else {
        eprintln!("canvas-map-stream: basemap only; spatial layers are not connected");
    }
    if let Some(path) = &config.raster_path {
        let raster = image::open(path)
            .with_context(|| format!("decode raster {path}"))?
            .into_rgba8();
        renderer.set_raster_rgba8(
            &device,
            &queue,
            raster.width(),
            raster.height(),
            raster.as_raw(),
        )?;
        eprintln!(
            "canvas-map-stream: raster='{}' {}x{}",
            path,
            raster.width(),
            raster.height()
        );
    }
    let palette = SpatialPalette::from_theme(&config.theme.palette());
    let mut encoder_process = config
        .endpoint
        .as_ref()
        .map(|_| Encoder::spawn(&config))
        .transpose()?;
    if let Some(path) = &local_frame_path {
        publish_frame_size(path, config.width, config.height)?;
    }
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.frames_per_second));
    let started = Instant::now();
    let mut deadline = started;
    let mut timing = std::env::var_os("CANVAS_MAP_STREAM_TRACE")
        .map(std::fs::File::create)
        .transpose()
        .context("create frame timing trace")?
        .map(std::io::BufWriter::new);
    if let Some(trace) = timing.as_mut() {
        writeln!(
            trace,
            "elapsed_ms,updates_ms,render_readback_ms,encode_ms,total_ms"
        )?;
    }
    eprintln!(
        "canvas-map-stream: adapter='{}' {}x{}@{} -> {}",
        adapter_info.name,
        config.width,
        config.height,
        config.frames_per_second,
        config
            .endpoint
            .as_deref()
            .or(local_frame_path.as_ref().and_then(|path| path.to_str()))
            .unwrap()
    );

    loop {
        let frame_started = Instant::now();
        if let Some(live) = &live {
            if let Some(features) = active_snapshot.as_mut() {
                let count = features.features.len();
                live_snapshot::expire(features, live_snapshot::now_ms());
                if features.features.len() != count {
                    renderer.set_features(&device, features)?;
                }
            }
            if let Ok(update) = live.try_recv() {
                match update {
                    Ok(mut features) => {
                        live_snapshot::expire(&mut features, live_snapshot::now_ms());
                        eprintln!(
                            "spatial: revision={} features={}",
                            features.revision,
                            features.features.len()
                        );
                        renderer.set_features(&device, &features)?;
                        active_snapshot = Some(features);
                    }
                    Err(error) => {
                        eprintln!("spatial: disconnected: {error}");
                        if let Some(mut previous) = active_snapshot.take() {
                            previous.features.clear();
                            renderer.set_features(&device, &previous)?;
                        }
                    }
                }
            }
            if active_snapshot
                .as_ref()
                .is_some_and(|features| !live_snapshot::fresh(features, live_snapshot::now_ms()))
            {
                let mut expired = active_snapshot.take().unwrap();
                expired.features.clear();
                renderer.set_features(&device, &expired)?;
                eprintln!("spatial: stale; overlays cleared");
            }
        }
        let pose = config
            .camera
            .tour
            .as_ref()
            .map(|tour| tour.sample(started.elapsed().as_secs_f64()));
        let longitude = pose.as_ref().map_or_else(
            || {
                wrap_longitude(
                    config.camera.longitude
                        + started.elapsed().as_secs_f64() * config.camera.orbit_degrees_per_second,
                )
            },
            |pose| pose.longitude,
        );
        let camera = GlobeCamera {
            latitude: pose
                .as_ref()
                .map_or(config.camera.latitude, |pose| pose.latitude),
            longitude,
            distance_m: pose
                .as_ref()
                .map_or(config.camera.distance_m, |pose| pose.distance_m),
            bearing_degrees: 0.0,
            pitch_degrees: config.camera.pitch_degrees,
        };
        if let Some(tiles) = &tiles {
            if let Ok(result) = tiles.results.try_recv() {
                match result {
                    Ok(raster) => {
                        renderer.set_mercator_detail_rgba8(
                            &device,
                            &queue,
                            raster.image.width(),
                            raster.image.height(),
                            raster.bounds,
                            raster.image.as_raw(),
                        )?;
                        eprintln!(
                            "tiles: uploaded z={} count={} bounds={:?}",
                            raster.zoom, raster.tiles, raster.bounds
                        );
                    }
                    Err(error) => {
                        eprintln!("tiles: detail unavailable; retaining basemap: {error}")
                    }
                }
            }
            if Instant::now() >= next_tile_request && camera.distance_m < 4_000_000.0 {
                tiles.requests.submit(galileo::tile_lod::TileView {
                    latitude: camera.latitude,
                    longitude: camera.longitude,
                    meters_per_pixel: camera.meters_per_pixel(config.height),
                    viewport_width_px: f64::from(config.width),
                    viewport_height_px: f64::from(config.height),
                });
                next_tile_request = Instant::now() + Duration::from_secs(2);
            }
        }
        let updates_done = Instant::now();
        let plan = camera.frame_plan(
            SurfaceMetrics {
                logical_width: f64::from(config.width),
                logical_height: f64::from(config.height),
                scale_factor: 1.0,
            },
            palette,
        )?;
        let mut commands = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Panopticon globe stream frame"),
        });
        let timestamp_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system clock predates Unix epoch")?
            .as_millis() as u64;
        renderer.render_into(
            &device,
            &queue,
            &mut commands,
            &view,
            &plan,
            timestamp_unix_ms,
        )?;
        commands.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(config.height),
                },
            },
            target.size(),
        );
        queue.submit([commands.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("poll WGPU readback")?;
        rx.recv().context("WGPU mapping callback dropped")??;
        let mapped = readback.slice(..).get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded_row * config.height) as usize);
        for row in mapped
            .chunks_exact(padded_row as usize)
            .take(config.height as usize)
        {
            rgba.extend_from_slice(&row[..unpadded_row as usize]);
        }
        drop(mapped);
        readback.unmap();
        let render_done = Instant::now();
        if let Some(encoder) = &mut encoder_process {
            encoder.write_frame(&rgba)?;
        } else if let Some(path) = &local_frame_path {
            publish_frame(path, &rgba)?;
        }

        let frame_done = Instant::now();
        if let Some(trace) = timing.as_mut() {
            writeln!(
                trace,
                "{:.3},{:.3},{:.3},{:.3},{:.3}",
                frame_started.duration_since(started).as_secs_f64() * 1000.0,
                updates_done.duration_since(frame_started).as_secs_f64() * 1000.0,
                render_done.duration_since(updates_done).as_secs_f64() * 1000.0,
                frame_done.duration_since(render_done).as_secs_f64() * 1000.0,
                frame_done.duration_since(frame_started).as_secs_f64() * 1000.0
            )?;
            trace.flush()?;
        }
        deadline += interval;
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        } else {
            deadline = Instant::now();
        }
    }
}

fn publish_frame_size(path: &std::path::Path, width: u32, height: u32) -> Result<()> {
    let parent = path.parent().context("local frame path needs a parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    std::fs::write(path.with_extension("size"), format!("{width}x{height}"))
        .context("publish local frame size")
}

fn publish_frame(path: &std::path::Path, rgba: &[u8]) -> Result<()> {
    let temporary = path.with_extension("rgba.next");
    std::fs::write(&temporary, rgba).context("write local frame")?;
    std::fs::rename(&temporary, path).context("publish local frame atomically")
}

fn wrap_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_wraps_continuously() {
        assert_eq!(wrap_longitude(181.0), -179.0);
        assert_eq!(wrap_longitude(-181.0), 179.0);
    }

    #[test]
    fn firetv_stream_uses_canonical_theme_and_live_owner() {
        let config: Config = serde_json::from_str(include_str!(
            "../../../../config/deploy/mac-studio/panopticon-firetv-stream.json"
        ))
        .unwrap();
        validate(&config).unwrap();
        assert!(config.snapshot_path.is_none());
        assert!(config.live_snapshot.is_some());
        assert!(config.source_id.is_none());
        assert_eq!(
            config.theme.palette(),
            serde_json::from_str::<ThemeSelection>(
                r#"{"source":"coherence","mode":"dark","accent":"info"}"#
            )
            .unwrap()
            .palette()
        );
    }

    #[test]
    fn existing_snapshot_and_palette_config_remains_compatible() {
        let config: Config = serde_json::from_str(include_str!(
            "../../../../config/deploy/mac-studio/panopticon-globe-stream.json"
        ))
        .unwrap();
        validate(&config).unwrap();
        assert!(config.snapshot_path.is_some());
        assert!(matches!(config.theme, StreamTheme::Palette(_)));
    }

    #[test]
    fn firetv_document_uses_the_shared_canvas_stream_contract() {
        let document: canvas::WorkspaceDocument = serde_json::from_str(include_str!(
            "../../../../config/deploy/firetv/panopticon.json"
        ))
        .unwrap();
        let catalog: canvas_core::Catalog = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "projects": [{"id":"panopticon", "name":"Panopticon", "theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}]
        })).unwrap();
        document.validate(&catalog).unwrap();
        assert!(
            matches!(&document.surfaces[0].content, canvas::SurfaceContent::Stream { url } if url.ends_with("/panopticon.m3u8"))
        );
        assert_eq!(document.surfaces.len(), 2);
    }
}
