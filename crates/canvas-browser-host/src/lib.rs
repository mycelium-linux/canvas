//! Host-local browser workers with an authenticated, engine-neutral control edge.
//!
//! The browser engine is deliberately out of the Canvas process. Each agent
//! principal gets an OS process and profile directory; its loopback WebDriver
//! listener is reachable only through this broker. Optional pixels use the
//! existing `media.video_stream.v1` contract rather than a Servo-specific
//! Canvas surface.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use unibus::{
    Message,
    client::{Client, ClientConfig},
};
use unibus_media::{
    MEDIA_SCHEMA_VERSION, VIDEO_STREAM_CONTRACT, VideoCodec, VideoStream, VideoTransport,
};

const MIN_CREDENTIAL_BYTES: usize = 32;
const MAX_SESSIONS: usize = 128;

fn default_startup_timeout_ms() -> u64 {
    15_000
}
fn default_width() -> u32 {
    1280
}
fn default_height() -> u32 {
    720
}
fn default_fps() -> u32 {
    12
}
fn default_ttl_ms() -> u64 {
    15_000
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub listen: SocketAddr,
    pub credential_file: PathBuf,
    pub state_dir: PathBuf,
    pub servo_binary: PathBuf,
    pub max_sessions: usize,
    pub webdriver_ports: PortRange,
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
    #[serde(default)]
    pub media_profiles: BTreeMap<String, MediaProfile>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareEncoder {
    AppleVideoToolbox,
    NvidiaNvenc,
    Vaapi,
}

/// A trusted, host-authored route from screenshots to one Canvas media edge.
/// Remote callers select this by name and cannot supply commands or endpoints.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaProfile {
    pub bus: ClientConfig,
    pub destination: Option<String>,
    pub ffmpeg_binary: PathBuf,
    pub encoder: HardwareEncoder,
    pub stream_endpoint: String,
    pub advertised_endpoint: String,
    pub zone: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_ttl_ms")]
    pub ttl_ms: u64,
}

impl HostConfig {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let value: Self =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_SESSIONS).contains(&self.max_sessions),
            "max_sessions must be 1..={MAX_SESSIONS}"
        );
        ensure!(
            self.webdriver_ports.start >= 1024
                && self.webdriver_ports.end >= self.webdriver_ports.start,
            "invalid WebDriver port range"
        );
        ensure!(
            usize::from(self.webdriver_ports.end - self.webdriver_ports.start) + 1
                >= self.max_sessions,
            "WebDriver port range is smaller than max_sessions"
        );
        ensure!(
            (1_000..=120_000).contains(&self.startup_timeout_ms),
            "startup_timeout_ms must be 1000..=120000"
        );
        ensure!(
            self.servo_binary.is_file(),
            "Servo binary does not exist: {}",
            self.servo_binary.display()
        );
        if !self.listen.ip().is_loopback() {
            ensure!(
                self.listen.ip() != IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                "explicitly bind a tailnet/LAN address, not 0.0.0.0"
            );
        }
        for (name, profile) in &self.media_profiles {
            profile.validate(name)?;
        }
        Ok(())
    }

    pub fn credential(&self) -> Result<Vec<u8>> {
        let value = std::fs::read(&self.credential_file)
            .with_context(|| format!("read {}", self.credential_file.display()))?;
        let value = value.strip_suffix(b"\n").unwrap_or(&value).to_vec();
        ensure!(
            value.len() >= MIN_CREDENTIAL_BYTES,
            "broker credential must contain at least {MIN_CREDENTIAL_BYTES} bytes"
        );
        Ok(value)
    }
}

impl MediaProfile {
    fn validate(&self, name: &str) -> Result<()> {
        ensure!(safe_id(name), "unsafe media profile name");
        ensure!(
            self.ffmpeg_binary.is_file(),
            "FFmpeg binary does not exist: {}",
            self.ffmpeg_binary.display()
        );
        ensure!((1..=30).contains(&self.fps), "media fps must be 1..=30");
        ensure!(
            (320..=3840).contains(&self.width) && (240..=2160).contains(&self.height),
            "invalid media dimensions"
        );
        ensure!(
            (5_000..=300_000).contains(&self.ttl_ms),
            "media ttl_ms must be 5000..=300000"
        );
        ensure!(safe_id(&self.zone), "unsafe media zone");
        let expected = if self.advertised_endpoint.starts_with("udp://") {
            VideoTransport::UdpMpegTs
        } else if self.advertised_endpoint.starts_with("tcp://") {
            VideoTransport::TcpMpegTs
        } else {
            bail!("media endpoint must be udp:// or tcp://")
        };
        ensure!(
            self.stream_endpoint.starts_with(match expected {
                VideoTransport::UdpMpegTs => "udp://",
                _ => "tcp://",
            }),
            "stream and advertised transports differ"
        );
        ensure!(
            !self.stream_endpoint.contains(['\n', '\r', '\0'])
                && !self.advertised_endpoint.contains(['\n', '\r', '\0']),
            "media endpoint contains control characters"
        );
        Ok(())
    }

    fn transport(&self) -> VideoTransport {
        if self.advertised_endpoint.starts_with("udp://") {
            VideoTransport::UdpMpegTs
        } else {
            VideoTransport::TcpMpegTs
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
    pub session_id: String,
    pub initial_url: Option<String>,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    pub media_profile: Option<String>,
}

impl CreateSession {
    fn validate(&self, config: &HostConfig) -> Result<()> {
        ensure!(safe_id(&self.session_id), "unsafe session_id");
        ensure!(
            (320..=3840).contains(&self.width) && (240..=2160).contains(&self.height),
            "invalid browser dimensions"
        );
        if let Some(url) = &self.initial_url {
            ensure!(
                url.starts_with("https://")
                    || url.starts_with("http://127.0.0.1")
                    || url.starts_with("http://localhost"),
                "initial_url must be HTTPS or loopback HTTP"
            );
            ensure!(
                !url.contains(['\n', '\r', '\0']),
                "initial_url contains control characters"
            );
        }
        if let Some(profile) = &self.media_profile {
            ensure!(
                config.media_profiles.contains_key(profile),
                "unknown media profile '{profile}'"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub engine: &'static str,
    pub webdriver_path: String,
    pub webdriver_port: u16,
    pub process_id: u32,
    pub width: u32,
    pub height: u32,
    pub media_profile: Option<String>,
    pub streaming: bool,
}

struct Session {
    request: CreateSession,
    port: u16,
    child: Child,
    stream_stop: Option<Arc<AtomicBool>>,
}

impl Session {
    fn info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.request.session_id.clone(),
            engine: "servo",
            webdriver_path: format!("/v1/sessions/{}/webdriver", self.request.session_id),
            webdriver_port: self.port,
            process_id: self.child.id(),
            width: self.request.width,
            height: self.request.height,
            media_profile: self.request.media_profile.clone(),
            streaming: self.stream_stop.is_some(),
        }
    }
}

pub struct BrowserPool {
    config: HostConfig,
    credential: Vec<u8>,
    sessions: Mutex<BTreeMap<String, Session>>,
}

impl BrowserPool {
    pub fn new(config: HostConfig) -> Result<Self> {
        config.validate()?;
        let credential = config.credential()?;
        std::fs::create_dir_all(&config.state_dir)
            .with_context(|| format!("create {}", config.state_dir.display()))?;
        Ok(Self {
            config,
            credential,
            sessions: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn authenticate(&self, authorization: Option<&[u8]>) -> bool {
        let Some(value) = authorization.and_then(|v| v.strip_prefix(b"Bearer ")) else {
            return false;
        };
        value.len() == self.credential.len() && bool::from(value.ct_eq(&self.credential))
    }

    pub fn create(&self, request: CreateSession) -> Result<SessionInfo> {
        request.validate(&self.config)?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("browser pool poisoned"))?;
        reap(&mut sessions);
        ensure!(
            !sessions.contains_key(&request.session_id),
            "session already exists"
        );
        ensure!(
            sessions.len() < self.config.max_sessions,
            "browser pool is full"
        );
        if let Some(profile) = &request.media_profile {
            ensure!(
                !sessions
                    .values()
                    .any(|s| s.request.media_profile.as_ref() == Some(profile)),
                "media profile '{profile}' is already in use"
            );
        }
        let used: BTreeSet<u16> = sessions.values().map(|s| s.port).collect();
        let port = (self.config.webdriver_ports.start..=self.config.webdriver_ports.end)
            .find(|port| !used.contains(port) && port_available(*port))
            .context("no WebDriver port is available")?;
        let profile_dir = self.config.state_dir.join(&request.session_id);
        std::fs::create_dir_all(&profile_dir)
            .with_context(|| format!("create {}", profile_dir.display()))?;
        let mut command = Command::new(&self.config.servo_binary);
        command
            .arg("--headless")
            .arg(format!("--webdriver={port}"))
            .arg("--temporary-storage")
            .arg("--config-dir")
            .arg(&profile_dir)
            .arg("--window-size")
            .arg(format!("{}x{}", request.width, request.height))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(url) = &request.initial_url {
            command.arg(url);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn {}", self.config.servo_binary.display()))?;
        if let Err(error) = wait_for_port(
            &mut child,
            port,
            Duration::from_millis(self.config.startup_timeout_ms),
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let session = Session {
            request,
            port,
            child,
            stream_stop: None,
        };
        let info = session.info();
        sessions.insert(info.session_id.clone(), session);
        Ok(info)
    }

    pub fn list(&self) -> Result<Vec<SessionInfo>> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("browser pool poisoned"))?;
        reap(&mut sessions);
        Ok(sessions.values().map(Session::info).collect())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        ensure!(safe_id(id), "unsafe session_id");
        let mut session = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("browser pool poisoned"))?
            .remove(id)
            .context("unknown browser session")?;
        if let Some(stop) = session.stream_stop.take() {
            stop.store(true, Ordering::Release);
        }
        let _ = session.child.kill();
        session.child.wait().context("wait for Servo")?;
        Ok(())
    }

    pub fn webdriver_port(&self, id: &str) -> Result<u16> {
        ensure!(safe_id(id), "unsafe session_id");
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("browser pool poisoned"))?;
        Ok(sessions.get(id).context("unknown browser session")?.port)
    }

    pub fn start_stream(&self, id: &str, webdriver_session_id: &str) -> Result<()> {
        ensure!(
            safe_id(id) && safe_webdriver_id(webdriver_session_id),
            "unsafe session identity"
        );
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("browser pool poisoned"))?;
        let session = sessions.get_mut(id).context("unknown browser session")?;
        if session.stream_stop.is_some() {
            return Ok(());
        }
        let Some(profile_name) = &session.request.media_profile else {
            return Ok(());
        };
        let profile = self
            .config
            .media_profiles
            .get(profile_name)
            .context("media profile disappeared")?
            .clone();
        let stop = Arc::new(AtomicBool::new(false));
        spawn_stream(
            id.to_owned(),
            webdriver_session_id.to_owned(),
            session.port,
            profile,
            stop.clone(),
        )?;
        session.stream_stop = Some(stop);
        Ok(())
    }
}

impl Drop for BrowserPool {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in std::mem::take(&mut *sessions) {
                if let Some(stop) = session.stream_stop.take() {
                    stop.store(true, Ordering::Release);
                }
                let _ = session.child.kill();
                let _ = session.child.wait();
            }
        }
    }
}

fn reap(sessions: &mut BTreeMap<String, Session>) {
    sessions.retain(|_, session| match session.child.try_wait() {
        Ok(None) => true,
        Ok(Some(_)) | Err(_) => {
            if let Some(stop) = &session.stream_stop {
                stop.store(true, Ordering::Release);
            }
            false
        }
    });
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn safe_webdriver_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn port_available(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}
fn wait_for_port(child: &mut Child, port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("Servo exited before WebDriver became ready: {status}");
        }
        if TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            Duration::from_millis(100),
        )
        .is_ok()
        {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "Servo WebDriver did not listen within {} ms",
            timeout.as_millis()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_stream(
    id: String,
    webdriver_id: String,
    port: u16,
    profile: MediaProfile,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    thread::Builder::new()
        .name(format!("browser-media-{id}"))
        .spawn(move || {
            if let Err(error) = run_stream(&id, &webdriver_id, port, &profile, &stop) {
                eprintln!("canvas-browser-host: stream '{id}' stopped: {error:#}");
            }
        })
        .context("spawn browser media thread")?;
    Ok(())
}

fn run_stream(
    id: &str,
    webdriver_id: &str,
    port: u16,
    profile: &MediaProfile,
    stop: &AtomicBool,
) -> Result<()> {
    let mut encoder = spawn_encoder(profile)?;
    let mut input = encoder.stdin.take().context("FFmpeg stdin unavailable")?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let announce_stop = Arc::new(AtomicBool::new(false));
    let announce_flag = announce_stop.clone();
    let stream = video_stream(id, profile)?;
    let bus_config = profile.bus.clone();
    let destination = profile.destination.clone();
    let ttl = profile.ttl_ms;
    let announcer = thread::Builder::new()
        .name(format!("browser-announce-{id}"))
        .spawn(move || {
            while !announce_flag.load(Ordering::Acquire) {
                let result = Client::connect(bus_config.clone()).and_then(|bus| {
                    while !announce_flag.load(Ordering::Acquire) {
                        announce(&bus, destination.as_deref(), &stream, ttl)?;
                        thread::sleep(Duration::from_millis(ttl / 3));
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    eprintln!("canvas-browser-host: announce reconnect: {error:#}");
                    thread::sleep(Duration::from_secs(1));
                }
            }
        })?;
    let interval = Duration::from_secs_f64(1.0 / f64::from(profile.fps));
    let url = format!("http://127.0.0.1:{port}/session/{webdriver_id}/screenshot");
    let mut capture_failures = 0_u64;
    let mut stream_result = Ok(());
    while !stop.load(Ordering::Acquire) {
        let started = Instant::now();
        let png = match capture_screenshot(&client, &url) {
            Ok(png) => {
                if capture_failures > 0 {
                    eprintln!(
                        "canvas-browser-host: stream '{id}' recovered after {capture_failures} dropped frame(s)"
                    );
                    capture_failures = 0;
                }
                png
            }
            Err(error) => {
                capture_failures += 1;
                if capture_failures == 1 || capture_failures.is_multiple_of(12) {
                    eprintln!(
                        "canvas-browser-host: stream '{id}' dropped frame {capture_failures}: {error:#}"
                    );
                }
                thread::sleep(interval.saturating_sub(started.elapsed()));
                continue;
            }
        };
        if let Err(error) = input.write_all(&png) {
            stream_result = Err(error).context("write screenshot to encoder");
            break;
        }
        thread::sleep(interval.saturating_sub(started.elapsed()));
    }
    announce_stop.store(true, Ordering::Release);
    drop(input);
    let _ = encoder.kill();
    let _ = encoder.wait();
    let _ = announcer.join();
    stream_result
}

fn capture_screenshot(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    let response: serde_json::Value = client.get(url).send()?.error_for_status()?.json()?;
    let encoded = response
        .get("value")
        .and_then(serde_json::Value::as_str)
        .context("WebDriver screenshot lacks value")?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("decode WebDriver screenshot")
}

fn spawn_encoder(profile: &MediaProfile) -> Result<Child> {
    let scale = format!(
        "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2",
        profile.width, profile.height, profile.width, profile.height
    );
    let mut command = Command::new(&profile.ffmpeg_binary);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "image2pipe",
        "-framerate",
        &profile.fps.to_string(),
        "-vcodec",
        "png",
        "-i",
        "pipe:0",
        "-an",
        "-vf",
        &scale,
    ]);
    match profile.encoder {
        HardwareEncoder::AppleVideoToolbox => {
            command.args(["-c:v", "h264_videotoolbox", "-realtime", "1", "-b:v", "3M"]);
        }
        HardwareEncoder::NvidiaNvenc => {
            command.args([
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p1",
                "-tune",
                "ull",
                "-b:v",
                "3M",
            ]);
        }
        HardwareEncoder::Vaapi => {
            command.args([
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-c:v",
                "h264_vaapi",
                "-b:v",
                "3M",
            ]);
        }
    }
    command
        .args([
            "-g",
            &profile.fps.to_string(),
            "-bf",
            "0",
            "-f",
            "mpegts",
            &profile.stream_endpoint,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {}", profile.ffmpeg_binary.display()))
}

fn video_stream(id: &str, profile: &MediaProfile) -> Result<VideoStream> {
    let stream = VideoStream {
        schema_version: MEDIA_SCHEMA_VERSION,
        stream_id: format!("browser-{id}"),
        display_name: format!("Browser {id}"),
        zone: profile.zone.clone(),
        transport: profile.transport(),
        endpoint: profile.advertised_endpoint.clone(),
        codec: VideoCodec::H264,
        width: profile.width,
        height: profile.height,
        frame_rate_millihz: profile.fps * 1_000,
        announced_at_unix_ms: 1,
        valid_until_unix_ms: 2,
    };
    stream.validate().map_err(anyhow::Error::msg)?;
    Ok(stream)
}

fn announce(
    client: &Client,
    destination: Option<&str>,
    template: &VideoStream,
    ttl_ms: u64,
) -> Result<()> {
    let now = now_ms();
    let mut stream = template.clone();
    stream.announced_at_unix_ms = now;
    stream.valid_until_unix_ms = now + ttl_ms;
    let body = serde_json::to_value(stream)?
        .as_object()
        .cloned()
        .context("video stream is not an object")?;
    let envelope = client.envelope(
        destination,
        Message::new(VIDEO_STREAM_CONTRACT, serde_json::Value::Object(body)),
    )?;
    client.send(&envelope, &[])
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
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, HostConfig) {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("servo");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let credential = dir.path().join("credential");
        std::fs::write(&credential, b"0123456789abcdef0123456789abcdef\n").unwrap();
        let config = HostConfig {
            listen: "127.0.0.1:7788".parse().unwrap(),
            credential_file: credential,
            state_dir: dir.path().join("state"),
            servo_binary: binary,
            max_sessions: 2,
            webdriver_ports: PortRange {
                start: 17000,
                end: 17001,
            },
            startup_timeout_ms: 1_000,
            media_profiles: BTreeMap::new(),
        };
        (dir, config)
    }

    #[test]
    fn config_and_auth_fail_closed() {
        let (_dir, config) = fixture();
        config.validate().unwrap();
        let pool = BrowserPool::new(config).unwrap();
        assert!(pool.authenticate(Some(b"Bearer 0123456789abcdef0123456789abcdef")));
        assert!(!pool.authenticate(None));
        assert!(!pool.authenticate(Some(b"Bearer short")));
    }

    #[test]
    fn requests_reject_paths_origins_and_unknown_profiles() {
        let (_dir, config) = fixture();
        let mut request = CreateSession {
            session_id: "../bad".into(),
            initial_url: None,
            width: 1280,
            height: 720,
            media_profile: None,
        };
        assert!(request.validate(&config).is_err());
        request.session_id = "agent-one".into();
        request.initial_url = Some("http://example.com".into());
        assert!(request.validate(&config).is_err());
        request.initial_url = Some("https://example.com".into());
        request.media_profile = Some("wall".into());
        assert!(request.validate(&config).is_err());
    }

    #[test]
    fn example_surface_is_a_valid_native_live_video_document() {
        let raw = include_str!("../../../config/demo/browser-surface.json");
        let document: canvas::WorkspaceDocument = serde_json::from_str(raw).unwrap();
        let catalog =
            canvas_core::Catalog::parse(include_str!("../../../config/projects.json")).unwrap();
        document.validate(&catalog).unwrap();
        assert_eq!(document.surfaces[0].content.kind_tag(), "live_video");
    }
}
