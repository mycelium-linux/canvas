//! `canvas-frigate` binary: keeps Canvas `frigate/<id>.jpg` frames fresh.
//!
//!   canvas-frigate --camera door=rtsp://192.168.30.12:8554/door \
//!                  --camera cat=rtsp://192.168.30.12:8554/cat --fps 2
//!
//! Each camera gets one persistent ffmpeg child (GPU decode on macOS). If the
//! child exits it restarts with capped backoff and says so. Run in the
//! foreground; Ctrl-C stops the whole process group, ffmpeg children included.

use canvas_frigate::{Camera, ffmpeg_args, frame_path, is_safe_source_id, write_atomic};
use std::{
    collections::BTreeSet,
    io::{BufReader, Read},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const ARG_ERR: &str = "usage: canvas-frigate --camera <id=rtsp-url> [--camera ...] [--fps N] [--max-height PX] [--runtime-dir PATH] [--ffmpeg PATH]";

fn opt(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_cameras(args: &[String]) -> Result<Vec<Camera>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--camera" {
            let spec = args
                .get(i + 1)
                .ok_or_else(|| format!("--camera needs a value: {ARG_ERR}"))?;
            let (id, url) =
                spec.split_once('=').ok_or_else(|| {
                    format!("--camera wants id=rtsp-url, got {spec:?}: {ARG_ERR}")
                })?;
            if !is_safe_source_id(id) {
                return Err(format!("unsafe camera id {id:?}"));
            }
            if !url.starts_with("rtsp://") {
                return Err(format!("camera {id:?} url must be rtsp://, got {url:?}"));
            }
            out.push(Camera {
                source_id: id.to_string(),
                rtsp_url: url.to_string(),
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    if out.is_empty() {
        return Err(format!("at least one --camera required: {ARG_ERR}"));
    }
    Ok(out)
}

/// Run one camera forever: pull ffmpeg, frame-scan its stdout, write frames
/// atomically, and restart on death with backoff that resets after a healthy
/// run so a flapping source cannot spin the CPU.
fn serve_camera(camera: &Camera, ffmpeg: &str, fps: f64, max_height: u32, dest: &std::path::Path) {
    let mut backoff = Duration::from_millis(500);
    let cap = Duration::from_secs(15);
    loop {
        let started = Instant::now();
        let mut child = match Command::new(ffmpeg)
            .args(ffmpeg_args(&camera.rtsp_url, fps, max_height))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("canvas-frigate {}: spawn failed: {error}", camera.source_id);
                thread::sleep(backoff);
                backoff = backoff.mul_f32(2.0).min(cap);
                continue;
            }
        };
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut reader = BufReader::new(stdout);
        let mut framer = canvas_frigate::JpegFramer::new();
        let mut chunks = [0u8; 64 * 1024];
        loop {
            let read = match reader.read(&mut chunks) {
                Ok(read) => read,
                Err(error) => {
                    eprintln!("canvas-frigate {}: read error: {error}", camera.source_id);
                    break;
                }
            };
            if read == 0 {
                break;
            }
            for frame in framer.push(&chunks[..read]) {
                if let Err(error) = write_atomic(dest, &frame) {
                    eprintln!("canvas-frigate {}: write failed: {error}", camera.source_id);
                }
            }
        }
        let _ = child.wait();
        if started.elapsed() > Duration::from_secs(10) {
            backoff = Duration::from_millis(500);
        }
        eprintln!(
            "canvas-frigate {}: stream ended, restarting in {backoff:?}",
            camera.source_id
        );
        thread::sleep(backoff);
        backoff = backoff.mul_f32(2.0).min(cap);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{ARG_ERR}");
        return Ok(());
    }
    let cameras = parse_cameras(&args).map_err(|error| -> Box<dyn std::error::Error> {
        eprintln!("{error}");
        ARG_ERR.into()
    })?;
    let mut ids = BTreeSet::new();
    for camera in &cameras {
        if !ids.insert(camera.source_id.clone()) {
            return Err(format!("duplicate camera id {}", camera.source_id).into());
        }
    }
    let fps: f64 = opt(&args, "--fps")
        .map(|value| value.parse().map_err(|_| "invalid --fps"))
        .transpose()?
        .unwrap_or(2.0);
    if !fps.is_finite() || fps <= 0.0 || fps > 30.0 {
        return Err("--fps must be finite and in 0..=30".into());
    }
    let max_height: u32 = opt(&args, "--max-height")
        .map(|value| value.parse().map_err(|_| "invalid --max-height"))
        .transpose()?
        .unwrap_or(540);
    if !(240..=1080).contains(&max_height) {
        return Err("--max-height must be 240..=1080".into());
    }
    let runtime_dir = match opt(&args, "--runtime-dir") {
        Some(dir) => PathBuf::from(dir),
        None => canvas_core::runtime_root().ok_or("set CANVAS_CONFIG_DIR or --runtime-dir")?,
    };
    let ffmpeg = opt(&args, "--ffmpeg").unwrap_or_else(|| "ffmpeg".to_string());

    let mut builders = Vec::new();
    for camera in cameras {
        // The id was validated at parse; Option here keeps the shared helper
        // honest rather than trusting the caller from a distance.
        let dest = frame_path(&runtime_dir, &camera.source_id)
            .ok_or_else(|| format!("unsafe camera id {}", camera.source_id))?;
        eprintln!("canvas-frigate: {} -> {}", camera.source_id, dest.display());
        let ffmpeg = ffmpeg.clone();
        builders.push((camera, dest, ffmpeg));
    }
    let threads: Vec<_> = builders
        .into_iter()
        .map(|(camera, dest, ffmpeg)| {
            thread::Builder::new()
                .name(format!("frigate-{}", camera.source_id))
                .spawn(move || serve_camera(&camera, &ffmpeg, fps, max_height, &dest))
        })
        .collect::<Result<_, _>>()?;
    // Foreground daemon: block on the workers; SIGINT ends the process group.
    for thread in threads {
        let _ = thread.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cameras_parse_id_url_and_reject_escapes() {
        let args: Vec<String> = [
            "--camera",
            "door=rtsp://h:8554/door",
            "--camera",
            "cat=rtsp://h:8554/cat",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cameras = parse_cameras(&args).unwrap();
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].source_id, "door");
        assert_eq!(cameras[1].rtsp_url, "rtsp://h:8554/cat");
    }

    #[test]
    fn camera_arguments_are_validated() {
        let cases = [
            vec!["--camera".to_string()],
            vec!["--camera".to_string(), "Door=rtsp://h/x".to_string()],
            vec!["--camera".to_string(), "door=http://h/x".to_string()],
            vec!["--camera".to_string(), "door".to_string()],
            vec!["--fps".to_string(), "2".to_string()],
        ];
        for case in cases {
            assert!(parse_cameras(&case).is_err(), "accepted {case:?}");
        }
    }
}
