//! Transport-neutral camera state and hardware decoder planning for Canvas.
//!
//! Unibus owns discovery and expiring observations. The media plane remains a
//! direct, negotiated video connection; Canvas sees only a named JPEG frame
//! and a matching, self-expiring detection snapshot. No producer-provided
//! command, pipeline, or executable string is ever evaluated.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use unibus_media::{
    CameraDetections, CameraStream, MEDIA_SCHEMA_VERSION, VideoCodec, VideoTransport,
};

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVideoPlan {
    pub argv: Vec<String>,
    pub width: u32,
    pub height: u32,
}

impl RawVideoPlan {
    pub fn frame_bytes(&self) -> Result<usize> {
        usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(self.height as usize))
            .and_then(|pixels| pixels.checked_mul(4))
            .context("RGBA frame dimensions overflow")
    }
}

/// Build a shell-free GStreamer pipeline for the Pi's stateless H.264 decoder
/// and V4L2 color converter. Video bytes remain on the negotiated media plane;
/// Unibus only carries the typed, expiring stream description.
pub fn gstreamer_raw_plan(stream: &CameraStream, policy: DecodePolicy) -> Result<RawVideoPlan> {
    stream.validate().map_err(anyhow::Error::msg)?;
    policy.validate()?;
    validate_endpoint(stream)?;
    ensure!(
        policy.decoder == HardwareDecoder::V4l2M2m,
        "raw edge currently requires v4l2m2m"
    );
    ensure!(
        stream.codec == VideoCodec::H264,
        "raw edge currently requires H.264"
    );
    ensure!(
        stream.transport == VideoTransport::UdpMpegTs,
        "raw edge currently requires UDP MPEG-TS"
    );

    let endpoint = stream
        .endpoint
        .strip_prefix("udp://")
        .context("invalid UDP endpoint")?;
    let (_host, port) = endpoint
        .rsplit_once(':')
        .context("UDP endpoint must include a port")?;
    let port: u16 = port.parse().context("invalid UDP port")?;
    let height = stream.height.min(policy.max_height).max(16) & !1;
    let width = ((u64::from(stream.width) * u64::from(height) / u64::from(stream.height)) as u32)
        .max(16)
        & !1;
    let fps = (f64::from(stream.frame_rate_millihz) / 1_000.0).min(policy.max_fps);
    let fps_numerator = (fps * 1_000.0).round() as u32;
    Ok(RawVideoPlan {
        width,
        height,
        argv: vec![
            "-q".into(),
            "udpsrc".into(),
            format!("port={port}"),
            "caps=video/mpegts,systemstream=(boolean)true,packetsize=(int)188".into(),
            "!".into(),
            "tsdemux".into(),
            "!".into(),
            "h264parse".into(),
            "!".into(),
            "v4l2h264dec".into(),
            "!".into(),
            "v4l2convert".into(),
            "!".into(),
            format!(
                "video/x-raw,format=RGBA,width={width},height={height},framerate={fps_numerator}/1000"
            ),
            "!".into(),
            "fdsink".into(),
            "fd=1".into(),
            "sync=false".into(),
        ],
    })
}

pub fn write_raw_frame(source_id: &str, expected_bytes: usize, bytes: &[u8]) -> Result<PathBuf> {
    ensure!(bytes.len() == expected_bytes, "invalid RGBA frame size");
    ensure!(
        expected_bytes <= 64 * 1024 * 1024,
        "RGBA frame exceeds safety limit"
    );
    let path = canvas_core::video_frame_file(source_id).context("unsafe video source id")?;
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// Host-owned hardware decoder. There is deliberately no software variant:
/// an unavailable accelerator is an explicit deployment error, never a CPU
/// saturation fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareDecoder {
    VideoToolbox,
    V4l2M2m,
}

impl HardwareDecoder {
    pub fn native() -> Result<Self> {
        if cfg!(target_os = "macos") {
            Ok(Self::VideoToolbox)
        } else if cfg!(target_os = "linux")
            && (cfg!(target_arch = "aarch64") || cfg!(target_arch = "arm"))
        {
            Ok(Self::V4l2M2m)
        } else {
            bail!("no supported hardware video decoder on this platform")
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "videotoolbox" => Ok(Self::VideoToolbox),
            "v4l2m2m" => Ok(Self::V4l2M2m),
            _ => bail!("hardware decoder must be videotoolbox or v4l2m2m"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodePolicy {
    pub max_height: u32,
    pub max_fps: f64,
    pub decoder: HardwareDecoder,
}

impl DecodePolicy {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (240..=2160).contains(&self.max_height),
            "max height must be 240..=2160"
        );
        ensure!(
            self.max_fps.is_finite() && (0.5..=60.0).contains(&self.max_fps),
            "max fps must be 0.5..=60"
        );
        Ok(())
    }
}

/// Build an ffmpeg argv from a validated typed stream. The endpoint occupies
/// one argument and is never interpreted as a shell or pipeline.
pub fn ffmpeg_args(stream: &CameraStream, policy: DecodePolicy) -> Result<Vec<String>> {
    stream.validate().map_err(anyhow::Error::msg)?;
    policy.validate()?;
    validate_endpoint(stream)?;

    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-fflags".into(),
        "+genpts+discardcorrupt".into(),
    ];
    match policy.decoder {
        HardwareDecoder::VideoToolbox => {
            args.extend(["-hwaccel".into(), "videotoolbox".into()]);
        }
        HardwareDecoder::V4l2M2m => {
            let codec = match stream.codec {
                VideoCodec::H264 => "h264_v4l2m2m",
                VideoCodec::H265 => "hevc_v4l2m2m",
            };
            args.extend(["-c:v".into(), codec.into()]);
        }
    }
    if stream.transport == VideoTransport::Rtsp {
        args.extend([
            "-rtsp_transport".into(),
            "tcp".into(),
            "-timeout".into(),
            "5000000".into(),
        ]);
    }
    let source_fps = f64::from(stream.frame_rate_millihz) / 1_000.0;
    let fps = source_fps.min(policy.max_fps);
    args.extend([
        "-i".into(),
        stream.endpoint.clone(),
        "-map".into(),
        "0:v:0".into(),
        "-vf".into(),
        format!("fps={fps},scale=-2:{}", policy.max_height),
        "-an".into(),
        "-f".into(),
        "image2pipe".into(),
        "-c:v".into(),
        "mjpeg".into(),
        "-q:v".into(),
        "6".into(),
        "pipe:1".into(),
    ]);
    Ok(args)
}

fn validate_endpoint(stream: &CameraStream) -> Result<()> {
    let expected = match stream.transport {
        VideoTransport::Rtsp => "rtsp://",
        VideoTransport::Srt => "srt://",
        VideoTransport::UdpMpegTs => "udp://",
    };
    ensure!(
        stream.endpoint.starts_with(expected),
        "{} stream endpoint must start with {expected}",
        stream.camera_id
    );
    ensure!(
        !stream.endpoint.contains(['\n', '\r', '\0']),
        "media endpoint contains control characters"
    );
    Ok(())
}

pub fn detection_file(runtime_root: &Path, camera_id: &str) -> Result<PathBuf> {
    canvas_core::detection_file(runtime_root, camera_id).context("unsafe camera id")
}

/// Persist the canonical contract unchanged so renderers can validate and
/// expire it against their own display clock without an adapter schema.
pub fn write_detections(
    runtime_root: &Path,
    value: &CameraDetections,
    now_ms: u64,
) -> Result<PathBuf> {
    value.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        value.schema_version == MEDIA_SCHEMA_VERSION,
        "unsupported detection schema"
    );
    ensure!(
        value.valid_until_unix_ms > now_ms,
        "detection snapshot is already expired"
    );
    let path = detection_file(runtime_root, &value.camera_id)?;
    write_atomic(
        &path,
        &serde_json::to_vec(value).context("encode detections")?,
    )?;
    Ok(path)
}

pub fn write_frame(runtime_root: &Path, camera_id: &str, bytes: &[u8]) -> Result<PathBuf> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_FRAME_BYTES,
        "invalid JPEG frame size"
    );
    ensure!(
        bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]),
        "decoder output is not a complete JPEG"
    );
    let path = canvas_core::frame_file(runtime_root, camera_id).context("unsafe camera id")?;
    write_atomic(&path, bytes)?;
    Ok(path)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, bytes).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("publish {}", path.display()))?;
    Ok(())
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeStatus {
    pub camera_id: String,
    pub announced_at_unix_ms: u64,
    pub valid_until_unix_ms: u64,
}

/// Reassembles the decoder's concatenated image2pipe JPEG output.
#[derive(Default)]
pub struct JpegFramer {
    buffer: Vec<u8>,
}

impl JpegFramer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(start) = marker(&self.buffer, 0, [0xff, 0xd8]) else {
                let keep = usize::from(self.buffer.last() == Some(&0xff));
                self.buffer.drain(..self.buffer.len().saturating_sub(keep));
                break;
            };
            self.buffer.drain(..start);
            let Some(end) = marker(&self.buffer, 2, [0xff, 0xd9]) else {
                if self.buffer.len() > MAX_FRAME_BYTES {
                    self.buffer.drain(..1);
                }
                break;
            };
            frames.push(self.buffer.drain(..end + 2).collect());
        }
        frames
    }
}

fn marker(bytes: &[u8], from: usize, needle: [u8; 2]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(2)
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unibus_media::{Detection, NormalizedBox};

    fn stream(transport: VideoTransport, endpoint: &str) -> CameraStream {
        CameraStream {
            schema_version: 1,
            camera_id: "front-door".into(),
            display_name: "Front Door".into(),
            zone: "door".into(),
            transport,
            endpoint: endpoint.into(),
            codec: VideoCodec::H264,
            width: 1280,
            height: 720,
            frame_rate_millihz: 24_000,
            announced_at_unix_ms: 100,
            valid_until_unix_ms: 10_100,
        }
    }

    #[test]
    fn transport_selects_protocol_without_accepting_a_pipeline() {
        for (transport, endpoint) in [
            (VideoTransport::Rtsp, "rtsp://nvr/door"),
            (VideoTransport::Srt, "srt://nvr:9000"),
            (VideoTransport::UdpMpegTs, "udp://239.0.0.1:5000"),
        ] {
            let value = stream(transport, endpoint);
            let args = ffmpeg_args(
                &value,
                DecodePolicy {
                    max_height: 540,
                    max_fps: 12.0,
                    decoder: HardwareDecoder::V4l2M2m,
                },
            )
            .unwrap();
            assert!(args.iter().any(|arg| arg == endpoint));
            assert!(args.join(" ").contains("h264_v4l2m2m"));
            assert!(!args.iter().any(|arg| arg == "-c" || arg == "sh"));
        }
        assert!(
            ffmpeg_args(
                &stream(VideoTransport::Rtsp, "udp://wrong"),
                DecodePolicy {
                    max_height: 540,
                    max_fps: 12.0,
                    decoder: HardwareDecoder::V4l2M2m
                }
            )
            .is_err()
        );
    }

    #[test]
    fn raw_plan_is_fixed_size_and_hardware_only() {
        let plan = gstreamer_raw_plan(
            &stream(VideoTransport::UdpMpegTs, "udp://192.168.10.74:5004"),
            DecodePolicy {
                max_height: 480,
                max_fps: 24.0,
                decoder: HardwareDecoder::V4l2M2m,
            },
        )
        .unwrap();
        assert_eq!((plan.width, plan.height), (852, 480));
        assert_eq!(plan.frame_bytes().unwrap(), 852 * 480 * 4);
        assert!(plan.argv.iter().any(|arg| arg == "v4l2h264dec"));
        assert!(plan.argv.iter().any(|arg| arg == "v4l2convert"));
        assert!(!plan.argv.iter().any(|arg| arg.contains("avdec")));
        assert!(!plan.argv.iter().any(|arg| arg == "sh" || arg == "-c"));
    }

    #[test]
    fn detection_snapshot_uses_canonical_expiring_contract() {
        let root = tempfile::tempdir().unwrap();
        let value = CameraDetections {
            schema_version: 1,
            camera_id: "front-door".into(),
            observed_at_unix_ms: 1_000,
            valid_until_unix_ms: 2_000,
            detections: vec![Detection {
                tracking_id: "track-7".into(),
                label: "person".into(),
                confidence: 0.93,
                bounds: NormalizedBox {
                    x: 0.1,
                    y: 0.2,
                    width: 0.3,
                    height: 0.4,
                },
            }],
        };
        let path = write_detections(root.path(), &value, 1_500).unwrap();
        assert_eq!(
            serde_json::from_slice::<CameraDetections>(&fs::read(path).unwrap()).unwrap(),
            value
        );
        assert!(write_detections(root.path(), &value, 2_000).is_err());
    }

    #[test]
    fn jpeg_framer_reassembles_decoder_output() {
        let mut framer = JpegFramer::default();
        assert!(framer.push(&[0xff, 0xd8, 1]).is_empty());
        assert_eq!(
            framer.push(&[2, 0xff, 0xd9, 0xff, 0xd8, 3, 0xff, 0xd9]),
            vec![
                vec![0xff, 0xd8, 1, 2, 0xff, 0xd9],
                vec![0xff, 0xd8, 3, 0xff, 0xd9]
            ]
        );
    }
}
