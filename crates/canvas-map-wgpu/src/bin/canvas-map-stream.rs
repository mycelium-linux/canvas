use anyhow::{Context, Result, bail, ensure};
use canvas::{
    backend::SurfaceMetrics,
    map::{
        globe::GlobeCamera,
        spatial::{SpatialPalette, decode_panopticon_snapshot},
    },
};
use canvas_core::ThemePalette;
use canvas_map_wgpu::GlobeRenderer;
use serde::Deserialize;
use std::{
    io::Write,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    width: u32,
    height: u32,
    frames_per_second: u32,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    local_frame_path: Option<String>,
    ffmpeg_binary: String,
    snapshot_path: String,
    theme: ThemePalette,
    camera: CameraConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CameraConfig {
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
    let snapshot = decode_panopticon_snapshot(
        &std::fs::read(&config.snapshot_path)
            .with_context(|| format!("read {}", config.snapshot_path))?,
    )
    .map_err(anyhow::Error::msg)?;

    pollster::block_on(run(config, snapshot))
}

fn validate(config: &Config) -> Result<()> {
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
        config.endpoint.is_some() ^ config.local_frame_path.is_some(),
        "set exactly one of endpoint or local_frame_path"
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

async fn run(config: Config, snapshot: canvas::map::spatial::SpatialFeatureSet) -> Result<()> {
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
    renderer.set_features(&device, &snapshot)?;
    let palette = SpatialPalette::from_theme(&config.theme);
    let mut encoder_process = config
        .endpoint
        .as_ref()
        .map(|_| Encoder::spawn(&config))
        .transpose()?;
    if let Some(path) = &config.local_frame_path {
        publish_frame_size(path, config.width, config.height)?;
    }
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.frames_per_second));
    let started = Instant::now();
    let mut deadline = started;
    eprintln!(
        "canvas-map-stream: adapter='{}' {}x{}@{} -> {}",
        adapter_info.name,
        config.width,
        config.height,
        config.frames_per_second,
        config
            .endpoint
            .as_deref()
            .or(config.local_frame_path.as_deref())
            .unwrap()
    );

    loop {
        let longitude = wrap_longitude(
            config.camera.longitude
                + started.elapsed().as_secs_f64() * config.camera.orbit_degrees_per_second,
        );
        let plan = GlobeCamera {
            latitude: config.camera.latitude,
            longitude,
            distance_m: config.camera.distance_m,
            bearing_degrees: 0.0,
            pitch_degrees: config.camera.pitch_degrees,
        }
        .frame_plan(
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
        renderer.render_into(&device, &queue, &mut commands, &view, &plan)?;
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
        if let Some(encoder) = &mut encoder_process {
            encoder.write_frame(&rgba)?;
        } else if let Some(path) = &config.local_frame_path {
            publish_frame(path, &rgba)?;
        }

        deadline += interval;
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        } else {
            deadline = Instant::now();
        }
    }
}

fn publish_frame_size(frame_path: &str, width: u32, height: u32) -> Result<()> {
    let path = std::path::Path::new(frame_path);
    let parent = path.parent().context("local frame path needs a parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    std::fs::write(path.with_extension("size"), format!("{width}x{height}"))
        .context("publish local frame size")
}

fn publish_frame(frame_path: &str, rgba: &[u8]) -> Result<()> {
    let path = std::path::Path::new(frame_path);
    let temporary = path.with_extension("rgba.next");
    std::fs::write(&temporary, rgba).context("write local frame")?;
    std::fs::rename(&temporary, path).context("publish local frame atomically")
}

fn wrap_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::wrap_longitude;

    #[test]
    fn orbit_wraps_continuously() {
        assert_eq!(wrap_longitude(181.0), -179.0);
        assert_eq!(wrap_longitude(-181.0), 179.0);
    }
}
