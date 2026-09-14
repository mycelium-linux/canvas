//! `canvas-frigate` — replay Frigate NVR camera streams into Canvas frames.
//!
//! Transport and decoding live here; the screen stays dumb. One persistent
//! ffmpeg child per camera pulls the go2rtc RTSP restream and decodes on the
//! GPU (`-hwaccel videotoolbox`), re-encoding only the sampled frames as JPEG.
//! Complete frames are written atomically to `<runtime root>/frames/<id>.jpg`;
//! the Canvas host reloads a `live_image` surface when those bytes change.
//! Nothing is cached, fabricated, or polled twice: a dead camera leaves the
//! last good frame plus this process's loud restart log, which is the honest
//! state of a wall, not a bug to hide.

/// Maximum accepted JPEG frame (an 8-bit 4K still is far under this).
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// A camera row as Canvas and this producer both name it: the `source_id` is
/// the `live_image` surface's identity, the RTSP url is where go2rtc serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    pub source_id: String,
    pub rtsp_url: String,
}

/// Source-id validation, frame path, and atomic writes come from canvas-core
/// so every producer (this and `canvas-comics`) lands frames where the Canvas
/// host reads them, with one shared rule. Re-exported for this crate's callers.
pub use canvas_core::{frame_file as frame_path, is_safe_frame_id as is_safe_source_id, write_atomic};

/// Hardware video decode backend for the input stream. The wall's JPEG encode
/// stays on CPU everywhere (no Pi has a usable JPEG *encode* path); this is
/// about the H.264 decode, which is where a Pi 4 would otherwise burn its
/// whole budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwDecode {
    /// videotoolbox on macOS, v4l2m2m on Linux-on-ARM (Raspberry Pi 4), CPU
    /// elsewhere — each host gets its real silicon, and the producer binary
    /// stays portable instead of carrying per-deployment flags.
    Auto,
    /// Apple's hardware video path (`-hwaccel videotoolbox`).
    VideoToolbox,
    /// V4L2 request-API decoder (`-c:v h264_v4l2m2m`), the Raspberry Pi 4's
    /// H.264 block. Note it is an input codec override, not a `-hwaccel`
    /// value: feeding videotoolbox's flag style here silently decodes on CPU.
    V4l2M2m,
    /// Plain CPU decode.
    None,
}

impl HwDecode {
    pub fn parse(token: &str) -> Result<Self, String> {
        match token {
            "auto" => Ok(Self::Auto),
            "videotoolbox" => Ok(Self::VideoToolbox),
            "v4l2m2m" => Ok(Self::V4l2M2m),
            "none" => Ok(Self::None),
            other => Err(format!("unknown --hwaccel {other}; expected auto|videotoolbox|v4l2m2m|none")),
        }
    }

    pub fn resolve(self) -> Self {
        match self {
            Self::Auto if cfg!(target_os = "macos") => Self::VideoToolbox,
            Self::Auto => {
                if cfg!(target_os = "linux")
                    && (cfg!(target_arch = "aarch64") || cfg!(target_arch = "arm"))
                {
                    Self::V4l2M2m
                } else {
                    Self::None
                }
            }
            explicit => explicit,
        }
    }

    fn input_args(self) -> Vec<String> {
        match self.resolve() {
            Self::VideoToolbox => vec!["-hwaccel".into(), "videotoolbox".into()],
            Self::V4l2M2m => vec!["-c:v".into(), "h264_v4l2m2m".into()],
            _ => Vec::new(),
        }
    }
}

/// Build the per-camera ffmpeg invocation. `fps` is the sample rate onto the
/// wall, not the camera's detect rate: 5 fps cameras replay convincingly at 2.
///
/// Frames are scaled down to `max_height` during decode (the `-2` keeps the
/// aspect). This is not cosmetic: a 2960×1668 JPEG costs seconds to decode
/// on a debug build's main thread, which starves the wall's event loop; a
/// 540-line frame decodes in a fraction and no wall tile is wider than that
/// source would ever need to be.
pub fn ffmpeg_args(rtsp_url: &str, fps: f64, max_height: u32, hwaccel: HwDecode) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-nostdin".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
    ];
    args.extend(hwaccel.input_args());
    args.extend([
        "-rtsp_transport".to_string(),
        "tcp".to_string(),
        // Microseconds; without this a silent stream wedges the child forever.
        "-timeout".to_string(),
        "5000000".to_string(),
        "-i".to_string(),
        rtsp_url.to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-vf".to_string(),
        format!("fps={fps},scale=-2:{max_height}"),
        "-an".to_string(),
        "-f".to_string(),
        "image2pipe".to_string(),
        "-c:v".to_string(),
        "mjpeg".to_string(),
        "-q:v".to_string(),
        "6".to_string(),
        "pipe:1".to_string(),
    ]);
    args
}


/// Slices a continuous JPEG byte stream (image2pipe concatenates codestreams
/// back-to-back) into whole frames by scanning SOI (FFD8) and EOI (FFD9).
/// In scan data every 0xFF is stuffed as 0xFF00, so an unescaped FFD9 is a
/// genuine end-of-image marker and the split is exact, not heuristic.
#[derive(Default)]
pub struct JpegFramer {
    buffer: Vec<u8>,
}

impl JpegFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed decoded output; returns every complete frame, in order.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(start) = find_sub(&self.buffer, 0, &[0xFF, 0xD8]) else {
                // No SOI anywhere: nothing here can begin a frame. Retain only
                // a trailing 0xFF that might lead a marker split across reads.
                let keep = usize::from(self.buffer.last() == Some(&0xFF));
                let begin = self.buffer.len().saturating_sub(keep);
                self.buffer.drain(..begin);
                break;
            };
            if start > 0 {
                self.buffer.drain(..start);
            }
            match find_sub(&self.buffer, 2, &[0xFF, 0xD9]) {
                Some(end) => {
                    let frame = self.buffer[..=end + 1].to_vec();
                    self.buffer.drain(..=end + 1);
                    frames.push(frame);
                }
                None => {
                    if self.buffer.len() > MAX_FRAME_BYTES {
                        // No EOI in 16 MB: this is garbage or a stream wedged
                        // mid-frame. Resync at the next byte rather than
                        // buffering forever.
                        self.buffer.drain(..1);
                    }
                    break;
                }
            }
        }
        frames
    }
}

fn find_sub(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn jpeg(scar: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(scar);
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn source_ids_cannot_escape_the_frames_dir() {
        assert!(is_safe_source_id("door"));
        assert!(is_safe_source_id("cam-2"));
        for bad in ["", "Door", "a/b", "..", "x.jpg", "a b", &"a".repeat(65)] {
            assert!(!is_safe_source_id(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn frame_path_lands_in_the_shared_frames_directory() {
        assert_eq!(
            frame_path(Path::new("/rt"), "door"),
            Some(PathBuf::from("/rt/frames/door.jpg"))
        );
        assert!(frame_path(Path::new("/rt"), "..").is_none());
    }

    #[test]
    fn ffmpeg_pulls_the_restream_with_gpu_decode_and_a_deadline() {
        let args = ffmpeg_args("rtsp://192.168.30.12:8554/door", 2.0, 540, HwDecode::Auto);
        let joined = args.join(" ");
        assert!(joined.contains("rtsp://192.168.30.12:8554/door"));
        assert!(joined.contains("image2pipe"));
        assert!(joined.contains("mjpeg"));
        assert!(joined.contains("fps=2"));
        assert!(joined.contains("scale=-2:540"));
        assert!(joined.contains("tcp"));
        assert!(joined.contains("-timeout"));
        assert!(!joined.contains("--"));
        if cfg!(target_os = "macos") {
            assert!(joined.contains("videotoolbox"), "GPU decode is the point");
        }
    }


    #[test]
    fn decode_backends_land_where_the_ffmpeg_api_put_them() {
        let vt = ffmpeg_args("rtsp://h/s", 2.0, 540, HwDecode::VideoToolbox).join(" ");
        assert!(vt.contains("-hwaccel videotoolbox -rtsp_transport"));
        let v4l = ffmpeg_args("rtsp://h/s", 2.0, 540, HwDecode::V4l2M2m);
        let joined = v4l.join(" ");
        assert!(joined.contains("-c:v h264_v4l2m2m -rtsp_transport"), "v4l2 is an INPUT codec override, before -i");
        assert!(joined.contains("-c:v mjpeg"), "output still re-encodes to JPEG");
        let input = v4l.iter().position(|a| a == "-i").unwrap();
        let decoder = v4l.iter().position(|a| a == "h264_v4l2m2m").unwrap();
        let encoder = v4l[input..].iter().position(|a| a == "mjpeg").unwrap() + input;
        assert!(decoder < input && input < encoder, "decoder precedes -i, encoder follows it");
        assert!(!ffmpeg_args("rtsp://h/s", 2.0, 540, HwDecode::None).join(" ").contains("hwaccel"));
        assert!(HwDecode::parse("nope").is_err());
    }

    #[test]
    fn framer_reassembles_split_and_concatenated_frames() {
        let a = jpeg(b"AAAA");
        let b = jpeg(b"BBBB");
        let mut stream = Vec::new();
        stream.extend_from_slice(&a[..3]);
        let mut framer = JpegFramer::new();
        assert!(framer.push(&stream).is_empty(), "partial frame holds");
        stream = a[3..].to_vec();
        stream.extend_from_slice(&b);
        let mut fed = framer.push(&stream);
        assert_eq!(fed.len(), 2, "split frame completes, next frame rides along");
        assert_eq!(fed.remove(0), a);
        assert_eq!(fed.remove(0), b);
    }

    #[test]
    fn framer_ignores_leading_garbage_and_keeps_the_tail() {
        let mut framer = JpegFramer::new();
        let mut input = b"log noise".to_vec();
        input.extend_from_slice(&jpeg(b"payload"));
        input.push(0xFF);
        let frames = framer.push(&input);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], jpeg(b"payload"));
        // A trailing 0xFF that could lead a split marker survives the next feed.
        let done = framer.push(&[0xD8, 0x42, 0xFF, 0xD9]);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0], jpeg(b"B"));
    }

    #[test]
    fn framer_resyncs_on_frames_that_never_end() {
        let mut framer = JpegFramer::new();
        let mut noise = vec![0xFF, 0xD8];
        noise.resize(MAX_FRAME_BYTES + 10, 0x00);
        let frames = framer.push(&noise);
        assert!(frames.is_empty());
        let good = jpeg(b"after resync");
        let recovered = framer.push(&good);
        assert_eq!(recovered.len(), 1, "buffer was bounded and resynced");
    }

}
