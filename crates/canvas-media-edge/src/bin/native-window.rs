//! Direct owner-local ScreenCaptureKit → IOSurface producer. No codec or bus.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("canvas-native-window requires macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context, Result, ensure};
    use canvas_core::{external_surface::*, external_surface_ipc as ipc};
    use canvas_media_edge::native_window::{
        WindowSelector, capture_dimensions, list_windows, select_window,
    };
    use screencapturekit::{
        cm::{CMSampleBufferExt, CMSampleBufferSCExt, CMTime, IOSurface, SCFrameStatus},
        prelude::*,
        shareable_content::SCShareableContent,
        stream::{
            SCStream, configuration::SCStreamConfiguration, content_filter::SCContentFilter,
            output_trait::SCStreamOutputTrait, output_type::SCStreamOutputType,
        },
    };
    use serde::Deserialize;
    use std::{
        os::{fd::AsFd, unix::net::UnixDatagram},
        path::PathBuf,
        sync::{Arc, Condvar, Mutex},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        source_id: String,
        selector: WindowSelector,
        fps: u32,
        capture_scale: f64,
        max_height: u32,
        #[serde(default)]
        socket: Option<PathBuf>,
    }

    struct Observation {
        sample: Option<CMSampleBuffer>,
        status: Option<SCFrameStatus>,
        observed: Instant,
    }
    struct Mailbox {
        value: Mutex<Observation>,
        wake: Condvar,
    }
    struct Handler(Arc<Mailbox>);
    struct CaptureHold(IOSurface);
    impl CaptureHold {
        fn new(surface: IOSurface) -> Self {
            surface.increment_use_count();
            Self(surface)
        }
    }
    impl Drop for CaptureHold {
        fn drop(&mut self) {
            self.0.decrement_use_count();
        }
    }
    impl SCStreamOutputTrait for Handler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output: SCStreamOutputType) {
            if !matches!(output, SCStreamOutputType::Screen) {
                return;
            }
            let status = sample.frame_status();
            let mut value = self.0.value.lock().expect("capture mailbox poisoned");
            value.observed = Instant::now();
            value.status = status;
            if status == Some(SCFrameStatus::Complete) && sample.pixel_buffer().is_some() {
                // Replace, never append. At most one pending capture is held.
                value.sample = Some(sample);
            } else if !matches!(status, Some(SCFrameStatus::Idle | SCFrameStatus::Complete)) {
                value.sample = None;
            }
            self.0.wake.notify_one();
        }
    }

    struct Publisher {
        socket: UnixDatagram,
        source: String,
        generation: u64,
    }
    impl Publisher {
        fn register(
            &mut self,
            descriptor: ExternalSurfaceDescriptor,
            return_socket: &UnixDatagram,
        ) -> Result<()> {
            ensure!(
                descriptor.source_id == self.source && descriptor.generation > self.generation,
                "native registration identity is stale or mismatched"
            );
            let generation = descriptor.generation;
            ipc::send(
                &self.socket,
                &ExternalSurfaceMessage::Register {
                    surface: descriptor,
                },
                &[return_socket.as_fd()],
            )?;
            // Track the pending registration even before its receipt arrives.
            self.generation = generation;
            Ok(())
        }
        fn present(&self) -> Result<()> {
            ipc::send(
                &self.socket,
                &ExternalSurfaceMessage::Present {
                    source_id: self.source.clone(),
                    generation: self.generation,
                    slot: 0,
                    sync: FrameSync::Implicit,
                    presented_at_unix_ns: epoch_ns()?,
                },
                &[],
            )
            .context("present native frame")
        }
    }
    impl Drop for Publisher {
        fn drop(&mut self) {
            if self.generation != 0 {
                if let Err(error) = ipc::send(
                    &self.socket,
                    &ExternalSurfaceMessage::Withdraw {
                        source_id: self.source.clone(),
                        generation: self.generation,
                    },
                    &[],
                ) {
                    eprintln!("native withdrawal failed: {error}");
                }
            }
        }
    }
    fn epoch_ns() -> Result<u64> {
        Ok(u64::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        )?)
    }

    pub fn run() -> Result<()> {
        unsafe extern "C" {
            fn sc_initialize_core_graphics();
        }
        unsafe {
            sc_initialize_core_graphics();
        }
        let argument = std::env::args()
            .nth(1)
            .context("usage: canvas-native-window <profile.json> | --list")?;
        if argument == "--list" {
            return list_windows();
        }
        let config: Config = serde_json::from_slice(&std::fs::read(&argument)?)?;
        ensure!(valid_source_id(&config.source_id), "invalid source_id");
        ensure!((1..=60).contains(&config.fps), "fps must be 1..=60");
        ensure!(
            config.capture_scale.is_finite() && (0.25..=2.0).contains(&config.capture_scale),
            "capture_scale must be 0.25..=2.0"
        );
        ensure!(
            (240..=2160).contains(&config.max_height),
            "max_height must be 240..=2160"
        );
        ensure!(
            config.selector.window_id.is_some()
                || config
                    .selector
                    .bundle_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty()),
            "explicit window selector required"
        );
        let content = SCShareableContent::get()
            .context("read native windows (Screen Recording permission required)")?;
        let (candidate, window) = select_window(&content, &config.selector)?;
        let (width, height) = capture_dimensions(
            candidate.width,
            candidate.height,
            config.capture_scale,
            config.max_height,
        );
        ensure!(
            width <= MAX_SURFACE_DIMENSION && height <= MAX_SURFACE_DIMENSION,
            "capture extent exceeds compositor limits"
        );
        let socket = UnixDatagram::unbound()?;
        socket.set_write_timeout(Some(Duration::from_secs(1)))?;
        let socket_path = config
            .socket
            .or_else(canvas_core::external_surface_socket_path)
            .context("Canvas socket path unavailable")?;
        socket.connect(&socket_path)?;
        let rights =
            canvas_core::native_surface_port::NativeSurfacePortSender::connect(&socket_path)
                .map_err(anyhow::Error::msg)?;
        let mut publisher = Publisher {
            socket,
            source: config.source_id,
            generation: 0,
        };
        let mailbox = Arc::new(Mailbox {
            value: Mutex::new(Observation {
                sample: None,
                status: None,
                observed: Instant::now(),
            }),
            wake: Condvar::new(),
        });
        let filter = SCContentFilter::create().with_window(&window).build()?;
        let capture = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_pixel_format(screencapturekit::prelude::PixelFormat::BGRA)
            .with_background_color_rgba(0.0, 0.0, 0.0, 0.0)
            .with_queue_depth(5)
            .with_shows_cursor(false)
            .with_minimum_frame_interval(&CMTime::new(1, i32::try_from(config.fps)?))
            .with_color_space_name(screencapturekit::stream::configuration::color_space::SRGB)?;
        let mut stream = SCStream::new(&filter, &capture)?;
        stream.add_output_handler(Handler(mailbox.clone()), SCStreamOutputType::Screen)?;
        stream.start_capture()?;
        eprintln!(
            "native capture: {} '{}' → {} ({width}x{height}, {} fps, IOSurface)",
            candidate.app_name, candidate.title, publisher.source, config.fps
        );
        let mut last_observed = None;
        let mut accepted = 0_u64;
        loop {
            let (sample, status, observed) = {
                let mut value = mailbox.value.lock().expect("capture mailbox poisoned");
                if value.sample.is_none() && last_observed == Some(value.observed) {
                    value = mailbox
                        .wake
                        .wait_timeout(value, Duration::from_millis(250))
                        .expect("capture mailbox poisoned")
                        .0;
                }
                (value.sample.take(), value.status, value.observed)
            };
            ensure!(
                status != Some(SCFrameStatus::Stopped),
                "native capture stopped"
            );
            ensure!(
                observed.elapsed() < Duration::from_secs(15),
                "native capture stopped delivering observations"
            );
            if let Some(sample) = sample {
                let pixels = sample
                    .pixel_buffer()
                    .context("complete frame has no pixels")?;
                let capture_hold = CaptureHold::new(
                    pixels
                        .io_surface()
                        .context("capture frame is not IOSurface-backed")?,
                );
                let surface = &capture_hold.0;
                let generation = epoch_ns()?.max(publisher.generation.saturating_add(1));
                let (receipt, return_socket) = UnixDatagram::pair()?;
                receipt.set_read_timeout(Some(Duration::from_secs(10)))?;
                let descriptor = ExternalSurfaceDescriptor {
                    schema_version: EXTERNAL_SURFACE_VERSION,
                    source_id: publisher.source.clone(),
                    generation,
                    width: u32::try_from(surface.width())?,
                    height: u32::try_from(surface.height())?,
                    format: canvas_core::external_surface::PixelFormat::Bgra8Unorm,
                    color_space: ColorSpace::Srgb,
                    alpha_mode: AlphaMode::Premultiplied,
                    transport: SurfaceTransport::IoSurfaceMachPort {
                        surface_id: surface.id(),
                    },
                    input: None,
                    acceptance: Some(SurfaceInputTransport::UnixDatagram { handle_index: 0 }),
                    streams: vec![],
                };
                ensure!(
                    surface.pixel_format() == u32::from_be_bytes(*b"BGRA"),
                    "capture returned non-BGRA pixels"
                );
                descriptor.validate(1).map_err(anyhow::Error::msg)?;
                // SAFETY: capture_hold keeps this IOSurfaceRef and its pixels
                // alive until Canvas has retained the transferred right.
                let right = unsafe {
                    canvas_core::native_surface_port::NativeSurfacePort::from_iosurface(
                        surface.as_ptr(),
                    )
                }
                .map_err(anyhow::Error::msg)?;
                rights
                    .send(&publisher.source, generation, surface.id(), &right)
                    .map_err(anyhow::Error::msg)?;
                // Keep the CMSampleBuffer alive until Canvas confirms it has
                // independently acquired its IOSurface use-count hold.
                publisher.register(descriptor, &return_socket)?;
                // Registration may have committed even if acknowledgement is
                // delayed. Withdrawal must target this pending generation on
                // every error path, not the previously presented generation.
                let acceptance_started = Instant::now();
                let acknowledgement = ipc::receive_acceptance(&receipt).context(
                    "Canvas did not retain the native frame (requires updated compositor)",
                )?;
                if acceptance_started.elapsed() > Duration::from_millis(100) {
                    eprintln!(
                        "native acceptance delayed: {} ms",
                        acceptance_started.elapsed().as_millis()
                    );
                }
                ensure!(
                    acknowledgement.source_id == publisher.source
                        && acknowledgement.generation == generation,
                    "native frame acceptance mismatch"
                );
                publisher.present()?;
                drop(sample);
                accepted += 1;
                if accepted == 1 || accepted.is_multiple_of(300) {
                    eprintln!("native frames accepted: {accepted}");
                }
            } else if status == Some(SCFrameStatus::Idle)
                && publisher.generation != 0
                && last_observed != Some(observed)
            {
                // An actual capture idle observation proves the unchanged
                // window is still current. Process liveness alone does not.
                publisher.present()?;
            }
            last_observed = Some(observed);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn shutdown_withdraws_an_unacknowledged_pending_generation() {
            let (sender, receiver) = UnixDatagram::pair().unwrap();
            let (_, return_socket) = UnixDatagram::pair().unwrap();
            let mut publisher = Publisher {
                socket: sender,
                source: "native/test".into(),
                generation: 7,
            };
            let descriptor = ExternalSurfaceDescriptor {
                schema_version: EXTERNAL_SURFACE_VERSION,
                source_id: "native/test".into(),
                generation: 8,
                width: 2,
                height: 2,
                format: canvas_core::external_surface::PixelFormat::Bgra8Unorm,
                color_space: ColorSpace::Srgb,
                alpha_mode: AlphaMode::Premultiplied,
                transport: SurfaceTransport::IoSurfaceMachPort { surface_id: 1 },
                input: None,
                acceptance: Some(SurfaceInputTransport::UnixDatagram { handle_index: 0 }),
                streams: vec![],
            };
            publisher
                .register(descriptor.clone(), &return_socket)
                .unwrap();
            let (registration, handles) = ipc::receive(&receiver).unwrap();
            assert!(
                matches!(registration, ExternalSurfaceMessage::Register { surface } if surface.generation == 8)
            );
            assert_eq!(handles.len(), 1);
            assert!(publisher.register(descriptor, &return_socket).is_err());
            drop(publisher); // No acceptance or Present has occurred.
            let (withdrawal, handles) = ipc::receive(&receiver).unwrap();
            assert!(handles.is_empty());
            assert!(
                matches!(withdrawal, ExternalSurfaceMessage::Withdraw { source_id, generation: 8 } if source_id == "native/test")
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    macos::run()
}
