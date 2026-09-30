//! Record a deterministic Canvas briefing and mux its pre-rendered narration.
//!
//! Canvas owns pixels, JARVIS/Bifrost owns speech, and this binary owns only
//! timing plus the final media artifact. The separation also permits adding a
//! Unibus PCM or music track without changing the video transport contract.

use anyhow::{Context, Result, bail, ensure};
use canvas::stream_binding::{BRIEFING_CONTRACT, STREAM_SCHEMA_VERSION, StreamEnvelope};
use canvas_widgets::BriefingBeat;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::net::UnixDatagram,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
struct Args {
    briefing: String,
    socket: PathBuf,
    output: PathBuf,
    display: u32,
    fps: u32,
    audio: Option<PathBuf>,
    mirror: Option<String>,
    no_tts: bool,
}

#[derive(Debug)]
struct Cue {
    beat: BriefingBeat,
    starts_at: Duration,
    narration: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    ensure!(
        args.socket.exists(),
        "Canvas stream socket does not exist: {}",
        args.socket.display()
    );
    ensure!(
        (1..=120).contains(&args.fps),
        "fps must be between 1 and 120"
    );

    let cache = cache_dir()?.join(&args.briefing);
    fs::create_dir_all(&cache)?;
    let (cues, duration) = prepare_timeline(&args, &cache)?;
    let raw = cache.join(format!("capture-{}.mov", std::process::id()));
    let mut capture = start_capture(
        &raw,
        duration,
        args.display,
        args.fps,
        args.mirror.as_deref(),
    )?;
    thread::sleep(Duration::from_millis(350));

    let socket = UnixDatagram::unbound()?;
    let started = Instant::now();
    for (sequence, cue) in cues.iter().enumerate() {
        sleep_until(started, cue.starts_at);
        send_beat(
            &socket,
            &args.socket,
            &args.briefing,
            cue.beat.beat,
            sequence as u64 + 1,
        )?;
    }
    sleep_until(started, duration);
    unsafe {
        libc::kill(capture.id() as i32, libc::SIGINT);
    }
    let status = capture.wait().context("wait for screen recorder")?;
    let captured_bytes = fs::metadata(&raw)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    ensure!(
        captured_bytes > 0,
        "screen recorder exited with {status} without producing video"
    );
    mux(&raw, &args.output, &cues, args.audio.as_deref())?;
    let _ = fs::remove_file(&raw);
    println!("{}", args.output.display());
    Ok(())
}

fn parse_args() -> Result<Args> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    #[cfg(target_os = "macos")]
    let default_socket =
        PathBuf::from(&home).join("Library/Application Support/Dock/canvas/stream.sock");
    #[cfg(not(target_os = "macos"))]
    let default_socket = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user/1000"))
        .join("dock/canvas/stream.sock");
    let mut args = Args {
        briefing: "intro-v1".into(),
        socket: default_socket,
        output: PathBuf::from("canvas-briefing.mp4"),
        display: 1,
        fps: 30,
        audio: None,
        mirror: None,
        no_tts: false,
    };
    let mut values = std::env::args().skip(1);
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--briefing" => args.briefing = values.next().context("--briefing needs a value")?,
            "--socket" => args.socket = values.next().context("--socket needs a value")?.into(),
            "--output" => args.output = values.next().context("--output needs a value")?.into(),
            "--display" => {
                args.display = values.next().context("--display needs a value")?.parse()?
            }
            "--fps" => args.fps = values.next().context("--fps needs a value")?.parse()?,
            "--audio" => args.audio = Some(values.next().context("--audio needs a value")?.into()),
            "--mirror" => {
                let endpoint = values.next().context("--mirror needs a value")?;
                ensure!(
                    endpoint.starts_with("udp://") || endpoint.starts_with("srt://"),
                    "mirror endpoint must use udp:// or srt://"
                );
                args.mirror = Some(endpoint);
            }
            "--no-tts" => args.no_tts = true,
            "-h" | "--help" => {
                println!(
                    "canvas-briefing-record [--briefing NAME] [--output FILE.mp4] [--display N] [--fps N] [--audio FILE] [--mirror udp://HOST:PORT] [--no-tts]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(args)
}

fn cache_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CANVAS_BRIEFING_CACHE") {
        return Ok(path.into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    #[cfg(target_os = "macos")]
    return Ok(home.join("Library/Caches/Dock/briefings"));
    #[cfg(not(target_os = "macos"))]
    Ok(std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"))
        .join("dock/briefings"))
}

fn envelope(briefing: &str, beat: u32, sequence: u64) -> StreamEnvelope {
    StreamEnvelope {
        schema_version: STREAM_SCHEMA_VERSION,
        owner: "canvas-recorder".into(),
        stream: "briefing".into(),
        contract: BRIEFING_CONTRACT.into(),
        project_id: None,
        sequence,
        emitted_at_ms: now_ms(),
        payload: json!({"briefing": briefing, "beat": beat}),
    }
}

fn prepare_timeline(args: &Args, cache: &Path) -> Result<(Vec<Cue>, Duration)> {
    let mut cues = Vec::new();
    let mut cursor = Duration::from_secs(1);
    for index in 0..128 {
        let beat =
            canvas::briefing::render_beat(&envelope(&args.briefing, index, index as u64 + 1))
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("render briefing beat {index}"))?;
        let narration = if args.no_tts {
            None
        } else {
            beat.narrate
                .as_deref()
                .map(|text| render_narration(cache, text))
                .transpose()?
        };
        let spoken = narration
            .as_deref()
            .map(audio_duration)
            .transpose()?
            .unwrap_or_default();
        let hold = Duration::from_secs_f32(beat.wait_seconds.max(0.1))
            .max(spoken + Duration::from_millis(350));
        let done = beat.done;
        cues.push(Cue {
            beat,
            starts_at: cursor,
            narration,
        });
        cursor += hold;
        if done {
            break;
        }
    }
    ensure!(
        cues.last().is_some_and(|cue| cue.beat.done),
        "briefing did not finish within 128 beats"
    );
    Ok((cues, cursor + Duration::from_secs(1)))
}

fn render_narration(cache: &Path, text: &str) -> Result<PathBuf> {
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    let output = cache.join(format!("{digest}.wav"));
    if output.exists() {
        return Ok(output);
    }
    let bifrost_key = std::env::var_os("BIFROST_API_KEY_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".jarvis/bifrost-key"))
        });
    if bifrost_key.as_ref().is_some_and(|path| path.exists()) {
        let endpoint =
            std::env::var("BIFROST_BASE_URL").unwrap_or_else(|_| "https://ai.fpl.dev/v1".into());
        render_bifrost(&endpoint, text, &output)?;
    } else if cfg!(target_os = "macos") {
        let status = Command::new("say")
            .args(["-v", "Daniel", "-o"])
            .arg(&output)
            .args(["--data-format=LEI16@24000", text])
            .status()?;
        ensure!(status.success(), "macOS say failed");
    } else {
        bail!("set BIFROST_BASE_URL to pre-render narration on this platform");
    }
    Ok(output)
}

fn render_bifrost(endpoint: &str, text: &str, output: &Path) -> Result<()> {
    let key_path = std::env::var_os("BIFROST_API_KEY_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".jarvis/bifrost-key"))
        })
        .context("BIFROST_API_KEY_FILE or HOME is required")?;
    let key =
        fs::read_to_string(&key_path).with_context(|| format!("read {}", key_path.display()))?;
    let body = json!({
        "model": std::env::var("BIFROST_TTS_MODEL").unwrap_or_else(|_| "fpl/tts-jarvis".into()),
        "voice": std::env::var("BIFROST_TTS_VOICE").unwrap_or_else(|_| "jarvis".into()),
        "response_format": "wav", "input": text
    });
    let body_path = output.with_extension("request.json");
    fs::write(&body_path, serde_json::to_vec(&body)?)?;
    let url = format!("{}/audio/speech", endpoint.trim_end_matches('/'));
    let mut child = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--config", "-"])
        .arg("--data-binary")
        .arg(format!("@{}", body_path.display()))
        .arg("--output")
        .arg(output)
        .arg(url)
        .stdin(Stdio::piped())
        .spawn()?;
    let config = format!(
        "header = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\n",
        key.trim()
    );
    child
        .stdin
        .take()
        .context("curl stdin")?
        .write_all(config.as_bytes())?;
    let status = child.wait()?;
    let _ = fs::remove_file(body_path);
    ensure!(status.success(), "Bifrost TTS request failed");
    Ok(())
}

fn audio_duration(path: &Path) -> Result<Duration> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(path)
        .output()?;
    ensure!(
        output.status.success(),
        "ffprobe failed for {}",
        path.display()
    );
    let seconds: f64 = String::from_utf8(output.stdout)?.trim().parse()?;
    Ok(Duration::from_secs_f64(seconds))
}

fn start_capture(
    path: &Path,
    duration: Duration,
    display: u32,
    fps: u32,
    mirror: Option<&str>,
) -> Result<Child> {
    #[cfg(target_os = "macos")]
    {
        let _ = fps; // macOS screencapture follows the display cadence.
        let _ = (duration, mirror);
        return Command::new("screencapture")
            .arg("-v")
            .arg("-x")
            .arg(format!("-D{display}"))
            .arg(path)
            .spawn()
            .context("start macOS screen recording (Screen Recording permission may be required)");
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (duration, display);
        let fps = fps.to_string();
        let mut command = Command::new("sudo");
        command.args([
            "-n",
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-f",
            "kmsgrab",
            "-device",
            "/dev/dri/card1",
            "-framerate",
            &fps,
            "-i",
            "-",
            "-vf",
            "hwdownload,format=bgr0,format=yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-crf",
            "24",
        ]);
        if let Some(endpoint) = mirror {
            command.args(["-map", "0:v:0", "-f", "tee"]);
            command.arg(format!(
                "[f=matroska]{}|[f=mpegts:onfail=ignore]{endpoint}",
                path.display()
            ));
        } else {
            command.args(["-f", "matroska"]).arg(path);
        }
        command.spawn().context("start DRM Canvas capture")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("screen recording is not supported on this platform")
}

fn send_beat(
    socket: &UnixDatagram,
    destination: &Path,
    briefing: &str,
    beat: u32,
    sequence: u64,
) -> Result<()> {
    let bytes = serde_json::to_vec(&envelope(briefing, beat, sequence))?;
    ensure!(
        bytes.len() <= 64 * 1024,
        "briefing event exceeds datagram limit"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match socket.send_to(&bytes, destination) {
            Ok(_) => return Ok(()),
            Err(error)
                if Instant::now() < deadline
                    && (error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(libc::ENOBUFS)) =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error).context("publish Canvas briefing beat"),
        }
    }
}

fn mux(video: &Path, output: &Path, cues: &[Cue], extra_audio: Option<&Path>) -> Result<()> {
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let tracks = cues
        .iter()
        .filter_map(|cue| cue.narration.as_ref().map(|path| (cue.starts_at, path)))
        .collect::<Vec<_>>();
    if tracks.is_empty() && extra_audio.is_none() {
        fs::copy(video, output)?;
        return Ok(());
    }
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(video);
    let mut filters = Vec::new();
    let mut labels = Vec::new();
    for (index, (offset, path)) in tracks.iter().enumerate() {
        command.arg("-i").arg(path);
        let input = index + 1;
        let label = format!("a{input}");
        filters.push(format!(
            "[{input}:a]adelay={}|{}[{label}]",
            offset.as_millis(),
            offset.as_millis()
        ));
        labels.push(format!("[{label}]"));
    }
    if let Some(path) = extra_audio {
        command.arg("-i").arg(path);
        labels.push(format!("[{}:a]", tracks.len() + 1));
    }
    filters.push(format!(
        "{}amix=inputs={}:normalize=0[aout]",
        labels.join(""),
        labels.len()
    ));
    let status = command
        .args([
            "-filter_complex",
            &filters.join(";"),
            "-map",
            "0:v:0",
            "-map",
            "[aout]",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
        ])
        .arg(output)
        .status()?;
    ensure!(status.success(), "ffmpeg mux failed");
    Ok(())
}

fn sleep_until(start: Instant, offset: Duration) {
    if let Some(wait) = offset.checked_sub(start.elapsed()) {
        thread::sleep(wait);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intro_timeline_ends_and_is_monotonic_without_tts() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            briefing: "intro-v1".into(),
            socket: "unused".into(),
            output: "unused".into(),
            display: 1,
            fps: 30,
            audio: None,
            mirror: None,
            no_tts: true,
        };
        let (cues, duration) = prepare_timeline(&args, dir.path()).unwrap();
        assert!(cues.len() >= 2);
        assert!(cues.last().unwrap().beat.done);
        assert!(
            cues.windows(2)
                .all(|pair| pair[0].starts_at < pair[1].starts_at)
        );
        assert!(duration > cues.last().unwrap().starts_at);
    }

    #[test]
    fn speedy_demo_tours_four_games_then_restores_overview() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            briefing: "speedy-demo".into(),
            socket: "unused".into(),
            output: "unused".into(),
            display: 1,
            fps: 30,
            audio: None,
            mirror: None,
            no_tts: true,
        };
        let (cues, _) = prepare_timeline(&args, dir.path()).unwrap();
        assert_eq!(cues.len(), 6);
        assert!(cues.last().unwrap().beat.done);
    }
}
