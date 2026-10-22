#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("canvas-window-stream requires macOS ScreenCaptureKit");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context, Result, ensure};
    use canvas::bus::{Bus, BusConfig};
    use canvas_core::unibus::Message;
    #[cfg(test)]
    use canvas_media_edge::native_window::{Candidate, require_unique};
    use canvas_media_edge::native_window::{
        WindowSelector, capture_dimensions, list_windows, select_window,
    };
    use screencapturekit::{
        cm::{CMSampleBufferExt, CMTime},
        cv::CVPixelBufferLockFlags,
        prelude::*,
        shareable_content::SCShareableContent,
        stream::{
            SCStream, configuration::SCStreamConfiguration, content_filter::SCContentFilter,
            output_trait::SCStreamOutputTrait, output_type::SCStreamOutputType,
        },
    };
    use serde::Deserialize;
    use std::{
        io::Write,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
            mpsc::{self, Receiver, SyncSender, TryRecvError},
        },
        thread,
        time::{Duration, Instant},
    };
    use unibus_media::{
        MEDIA_SCHEMA_VERSION, TransportRole, VIDEO_ALPHA_MASK_CONTRACT, VIDEO_STREAM_CONTRACT,
        VideoAlphaMask, VideoCodec, VideoStream, VideoTransport, encode_alpha_rle, payload_sha256,
    };

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        bus: BusConfig,
        destination: String,
        ffmpeg: PathBuf,
        selector: WindowSelector,
        stream_id: String,
        display_name: String,
        stream_endpoint: String,
        advertised_endpoint: String,
        #[serde(default = "default_max_height")]
        max_height: u32,
        #[serde(default = "default_capture_scale")]
        capture_scale: f64,
        #[serde(default = "default_fps")]
        fps: u32,
        #[serde(default = "default_ttl_ms")]
        ttl_ms: u64,
    }

    const fn default_max_height() -> u32 {
        960
    }
    const fn default_fps() -> u32 {
        30
    }
    const fn default_capture_scale() -> f64 {
        1.0
    }
    const fn default_ttl_ms() -> u64 {
        30_000
    }

    impl Config {
        fn read(path: &Path) -> Result<Self> {
            let config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
            ensure!(
                config.ffmpeg.is_file(),
                "ffmpeg not found: {}",
                config.ffmpeg.display()
            );
            ensure!(
                (240..=2160).contains(&config.max_height),
                "max_height must be 240..=2160"
            );
            ensure!(
                config.capture_scale.is_finite() && (0.25..=2.0).contains(&config.capture_scale),
                "capture_scale must be 0.25..=2.0"
            );
            ensure!((1..=60).contains(&config.fps), "fps must be 1..=60");
            ensure!((5_000..=300_000).contains(&config.ttl_ms), "invalid ttl_ms");
            ensure!(
                config.selector.window_id.is_some()
                    || config
                        .selector
                        .bundle_id
                        .as_deref()
                        .is_some_and(|v| !v.is_empty()),
                "selector requires window_id or bundle_id"
            );
            Ok(config)
        }
    }

    struct Encoder {
        child: Mutex<Option<Child>>,
        failure: Mutex<Option<String>>,
        width: usize,
        height: usize,
        frames: AtomicU64,
        alpha_tx: SyncSender<Vec<u8>>,
        last_alpha: Mutex<Vec<u8>>,
    }

    impl Encoder {
        fn new(
            config: &Config,
            width: u32,
            height: u32,
            alpha_tx: SyncSender<Vec<u8>>,
        ) -> Result<Self> {
            let child = Command::new(&config.ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "bgra",
                    "-video_size",
                    &format!("{width}x{height}"),
                    "-framerate",
                    &config.fps.to_string(),
                    "-i",
                    "pipe:0",
                    "-an",
                    "-c:v",
                    "h264_videotoolbox",
                    "-realtime",
                    "1",
                    "-b:v",
                    "4M",
                    "-g",
                    &config.fps.to_string(),
                    "-bf",
                    "0",
                    "-f",
                    "mpegts",
                    &config.stream_endpoint,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .with_context(|| format!("spawn {}", config.ffmpeg.display()))?;
            Ok(Self {
                child: Mutex::new(Some(child)),
                failure: Mutex::new(None),
                width: width as usize,
                height: height as usize,
                frames: AtomicU64::new(0),
                alpha_tx,
                last_alpha: Mutex::new(Vec::new()),
            })
        }

        fn write_pixel_buffer(&self, sample: CMSampleBuffer) {
            let Some(pixel_buffer) = sample.pixel_buffer() else {
                // ScreenCaptureKit emits status-only screen samples during window
                // transitions; only image-bearing samples belong in the encoder.
                return;
            };
            let Ok(guard) = pixel_buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
                self.fail("could not lock ScreenCaptureKit pixel buffer");
                return;
            };
            let Some(data) = (unsafe { guard.as_slice() }) else {
                self.fail("ScreenCaptureKit pixel buffer has no readable plane");
                return;
            };
            let stride = guard.bytes_per_row();
            let row_bytes = self.width * 4;
            if stride < row_bytes || data.len() < stride.saturating_mul(self.height) {
                self.fail(format!(
                    "invalid pixel buffer: stride={stride}, bytes={}, expected={}x{}",
                    data.len(),
                    self.width,
                    self.height
                ));
                return;
            }
            // RGB stays on H.264. Alpha is retained state and only crosses
            // Unibus when the complete plane changes (plus lease renewal).
            let mut alpha = Vec::with_capacity(self.width * self.height);
            for row in data.chunks_exact(stride).take(self.height) {
                alpha.extend(row[..row_bytes].chunks_exact(4).map(|pixel| pixel[3]));
            }
            if let Ok(mut last) = self.last_alpha.lock()
                && *last != alpha
                && self.alpha_tx.try_send(alpha.clone()).is_ok()
            {
                *last = alpha;
            }
            let Ok(mut child) = self.child.lock() else {
                self.fail("encoder lock poisoned");
                return;
            };
            let Some(stdin) = child.as_mut().and_then(|child| child.stdin.as_mut()) else {
                drop(child);
                self.fail("ffmpeg stdin is unavailable");
                return;
            };
            let result = if stride == row_bytes {
                stdin.write_all(&data[..row_bytes * self.height])
            } else {
                data.chunks_exact(stride)
                    .take(self.height)
                    .try_for_each(|row| stdin.write_all(&row[..row_bytes]))
            };
            match result {
                Ok(()) => {
                    self.frames.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    drop(child);
                    self.fail(format!("write frame to ffmpeg: {error}"));
                }
            }
        }

        fn fail(&self, message: impl Into<String>) {
            if let Ok(mut failure) = self.failure.lock() {
                if failure.is_none() {
                    *failure = Some(message.into());
                }
            }
        }

        fn check_health(&self) -> Result<()> {
            if let Some(failure) = self
                .failure
                .lock()
                .map_err(|_| anyhow::anyhow!("failure lock poisoned"))?
                .as_ref()
            {
                anyhow::bail!("frame pipeline failed: {failure}");
            }
            let mut child = self
                .child
                .lock()
                .map_err(|_| anyhow::anyhow!("encoder lock poisoned"))?;
            let child = child.as_mut().context("ffmpeg encoder missing")?;
            ensure!(child.try_wait()?.is_none(), "ffmpeg encoder exited");
            Ok(())
        }
    }

    struct FrameHandler(Arc<Encoder>);

    impl SCStreamOutputTrait for FrameHandler {
        fn did_output_sample_buffer(
            &self,
            sample: CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            if matches!(output_type, SCStreamOutputType::Screen) {
                self.0.write_pixel_buffer(sample);
            }
        }
    }

    pub fn run() -> Result<()> {
        initialize_core_graphics();
        let argument = std::env::args()
            .nth(1)
            .context("usage: canvas-window-stream <config.json> | --list")?;
        if argument == "--list" {
            return list_windows();
        }
        let config = Config::read(Path::new(&argument))?;
        let content =
            SCShareableContent::get().context("read ScreenCaptureKit shareable content")?;
        let (candidate, window) = select_window(&content, &config.selector)?;
        let (width, height) = capture_dimensions(
            candidate.width,
            candidate.height,
            config.capture_scale,
            config.max_height,
        );
        let (alpha_tx, alpha_rx) = mpsc::sync_channel(1);
        let encoder = Arc::new(Encoder::new(&config, width, height, alpha_tx)?);
        let filter = SCContentFilter::create().with_window(&window).build()?;
        let interval = CMTime::new(1, i32::try_from(config.fps)?);
        let stream_config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_pixel_format(PixelFormat::BGRA)
            .with_minimum_frame_interval(&interval)
            .with_shows_cursor(false);
        let mut stream = SCStream::new(&filter, &stream_config)?;
        stream.add_output_handler(FrameHandler(encoder.clone()), SCStreamOutputType::Screen)?;
        stream.start_capture()?;
        eprintln!(
            "canvas-window-stream: capturing {} ({}) '{}' as {}x{}",
            candidate.app_name, candidate.bundle_id, candidate.title, width, height
        );
        let bus = Bus::connect(config.bus.clone())?;
        let mut alpha_generation = 0_u64;
        let mut latest_alpha: Option<Vec<u8>> = None;
        loop {
            announce(&bus, &config, width, height)?;
            let renewal = Instant::now() + Duration::from_millis(config.ttl_ms / 2);
            while Instant::now() < renewal {
                encoder.check_health()?;
                drain_alpha(
                    &alpha_rx,
                    &mut latest_alpha,
                    &mut alpha_generation,
                    |generation, alpha| {
                        announce_alpha(&bus, &config, width, height, generation, alpha)
                    },
                )?;
                thread::sleep(Duration::from_millis(250));
            }
            if let Some(alpha) = latest_alpha.as_deref() {
                announce_alpha(&bus, &config, width, height, alpha_generation, alpha)?;
            }
        }
    }

    fn drain_alpha(
        receiver: &Receiver<Vec<u8>>,
        latest: &mut Option<Vec<u8>>,
        generation: &mut u64,
        mut publish: impl FnMut(u64, &[u8]) -> Result<()>,
    ) -> Result<()> {
        loop {
            match receiver.try_recv() {
                Ok(alpha) => {
                    *generation = generation.saturating_add(1).max(1);
                    publish(*generation, &alpha)?;
                    *latest = Some(alpha);
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("alpha-mask producer disconnected")
                }
            }
        }
    }

    fn announce(bus: &Bus, config: &Config, width: u32, height: u32) -> Result<()> {
        let now = canvas_media_edge::now_unix_ms();
        let stream = VideoStream {
            schema_version: MEDIA_SCHEMA_VERSION,
            stream_id: config.stream_id.clone(),
            display_name: config.display_name.clone(),
            zone: "living-room".into(),
            transport: VideoTransport::UdpMpegTs,
            transport_role: TransportRole::Listen,
            endpoint: config.advertised_endpoint.clone(),
            codec: VideoCodec::H264,
            width,
            height,
            frame_rate_millihz: config.fps * 1_000,
            announced_at_unix_ms: now,
            valid_until_unix_ms: now + config.ttl_ms,
        };
        stream.validate().map_err(anyhow::Error::msg)?;
        let envelope = bus.to_destination(
            &config.destination,
            Message::new(VIDEO_STREAM_CONTRACT, serde_json::to_value(stream)?),
        )?;
        bus.send(&envelope, &[]).context("announce window stream")
    }

    fn announce_alpha(
        bus: &Bus,
        config: &Config,
        width: u32,
        height: u32,
        generation: u64,
        alpha: &[u8],
    ) -> Result<()> {
        let payload = encode_alpha_rle(alpha).map_err(anyhow::Error::msg)?;
        let now = canvas_media_edge::now_unix_ms();
        let mask = VideoAlphaMask {
            schema_version: MEDIA_SCHEMA_VERSION,
            stream_id: config.stream_id.clone(),
            width,
            height,
            generation,
            payload_sha256: payload_sha256(&payload),
            observed_at_unix_ms: now,
            valid_until_unix_ms: now + config.ttl_ms,
        };
        mask.validate(&payload).map_err(anyhow::Error::msg)?;
        let mut envelope = bus.to_destination(
            &config.destination,
            Message::new(VIDEO_ALPHA_MASK_CONTRACT, serde_json::to_value(mask)?),
        )?;
        envelope.payload_bytes = payload.len();
        bus.send(&envelope, &payload)
            .context("announce window alpha mask")
    }

    fn initialize_core_graphics() {
        unsafe extern "C" {
            fn sc_initialize_core_graphics();
        }
        unsafe { sc_initialize_core_graphics() }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn candidate(window_id: u32, title: &str) -> Candidate {
            Candidate {
                window_id,
                bundle_id: "dev.fpl.test".into(),
                app_name: "Test".into(),
                title: title.into(),
                width: 640.0,
                height: 480.0,
            }
        }

        #[test]
        fn unique_match_is_selected() {
            assert_eq!(
                require_unique(vec![candidate(7, "Only")])
                    .unwrap()
                    .window_id,
                7
            );
        }

        #[test]
        fn missing_match_fails_loudly() {
            assert!(
                require_unique(vec![])
                    .unwrap_err()
                    .to_string()
                    .contains("no window")
            );
        }

        #[test]
        fn ambiguous_match_lists_candidates() {
            let error = require_unique(vec![candidate(7, "A"), candidate(9, "B")])
                .unwrap_err()
                .to_string();
            assert!(error.contains("ambiguous"));
            assert!(error.contains("7:A"));
            assert!(error.contains("9:B"));
        }

        #[test]
        fn capture_scale_retains_retina_detail_with_a_pixel_height_cap() {
            assert_eq!(capture_dimensions(800.0, 600.0, 1.5, 900), (1200, 900));
            assert_eq!(capture_dimensions(800.0, 600.0, 2.0, 900), (1200, 900));
            assert_eq!(capture_dimensions(800.0, 600.0, 1.0, 900), (800, 600));
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    macos::run()
}
