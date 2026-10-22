//! QEMU display adapter for Canvas.
//!
//! QMP is used only to prove that the configured guest display exists. Pixels
//! come directly from QEMU's RFB server, are encoded by the host GPU, and ride
//! the existing Canvas media plane. No desktop/window capture is involved.

use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas_core::unibus::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use unibus_media::{
    MEDIA_SCHEMA_VERSION, TransportRole, VIDEO_STREAM_CONTRACT, VideoCodec, VideoStream,
    VideoTransport,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: BusConfig,
    destination: String,
    gst_launch: PathBuf,
    qmp_socket: PathBuf,
    vnc_host: String,
    vnc_port: u16,
    stream_id: String,
    display_name: String,
    stream_endpoint: String,
    advertised_endpoint: String,
    width: u32,
    height: u32,
    fps: u32,
    #[serde(default = "default_bitrate_kbps")]
    bitrate_kbps: u32,
    #[serde(default = "default_ttl_ms")]
    ttl_ms: u64,
}

const fn default_bitrate_kbps() -> u32 {
    8_000
}

const fn default_ttl_ms() -> u64 {
    30_000
}

impl Config {
    fn read(path: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(
            &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        ensure!(config.gst_launch.is_file(), "gst-launch not found");
        ensure!(config.qmp_socket.exists(), "QMP socket not found");
        ensure!(!config.vnc_host.is_empty(), "VNC host is required");
        ensure!((16..=7680).contains(&config.width), "invalid width");
        ensure!((16..=4320).contains(&config.height), "invalid height");
        ensure!((1..=60).contains(&config.fps), "fps must be 1..=60");
        ensure!(
            (250..=200_000).contains(&config.bitrate_kbps),
            "bitrate_kbps must be 250..=200000"
        );
        ensure!((5_000..=300_000).contains(&config.ttl_ms), "invalid ttl_ms");
        parse_udp_endpoint(&config.stream_endpoint)?;
        Ok(config)
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-qemu-stream <config.json>")?;
    let config = Config::read(Path::new(&path))?;
    verify_qmp_vnc(&config)?;
    let bus = Bus::connect(config.bus.clone())?;
    let mut pipeline = spawn_pipeline(&config)?;
    let mut next_announcement = Instant::now();
    loop {
        if Instant::now() >= next_announcement {
            announce(&bus, &config)?;
            next_announcement = Instant::now() + Duration::from_millis(config.ttl_ms / 2);
        }
        if let Some(status) = pipeline.try_wait()? {
            bail!("QEMU display pipeline exited {status}");
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// QMP discovers/control-proofs the display, but never transports pixels.
fn verify_qmp_vnc(config: &Config) -> Result<()> {
    let mut socket = UnixStream::connect(&config.qmp_socket)
        .with_context(|| format!("connect QMP at {}", config.qmp_socket.display()))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(socket.try_clone()?);
    let greeting = read_json_line(&mut reader)?;
    ensure!(greeting.get("QMP").is_some(), "invalid QMP greeting");
    qmp_execute(&mut socket, &mut reader, "qmp_capabilities", 1)?;
    let response = qmp_execute(&mut socket, &mut reader, "query-vnc", 2)?;
    let result = response
        .get("return")
        .context("QMP query-vnc response has no return value")?;
    ensure!(
        result.get("enabled").and_then(Value::as_bool) == Some(true),
        "QEMU VNC display is not enabled"
    );
    if let Some(service) = result.get("service").and_then(Value::as_str) {
        let actual: u16 = service
            .parse()
            .context("QMP returned invalid VNC service")?;
        ensure!(
            actual == config.vnc_port,
            "QMP VNC port {actual} does not match configured {}",
            config.vnc_port
        );
    }
    Ok(())
}

fn qmp_execute(
    writer: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    command: &str,
    id: u64,
) -> Result<Value> {
    serde_json::to_writer(&mut *writer, &json!({"execute": command, "id": id}))?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    loop {
        let value = read_json_line(reader)?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                bail!("QMP {command} failed: {error}");
            }
            return Ok(value);
        }
        // Asynchronous QMP events can legally arrive between command replies.
    }
}

fn read_json_line(reader: &mut impl BufRead) -> Result<Value> {
    let mut line = String::new();
    ensure!(reader.read_line(&mut line)? != 0, "QMP closed the socket");
    serde_json::from_str(&line).context("decode QMP response")
}

fn parse_udp_endpoint(endpoint: &str) -> Result<(String, u16)> {
    let authority = endpoint
        .strip_prefix("udp://")
        .context("stream_endpoint must start with udp://")?;
    let authority = authority
        .split_once('?')
        .map_or(authority, |(head, _)| head);
    let (host, port) = authority
        .rsplit_once(':')
        .context("stream_endpoint must contain host:port")?;
    ensure!(!host.is_empty(), "stream endpoint host is empty");
    Ok((host.to_owned(), port.parse().context("invalid UDP port")?))
}

fn pipeline_args(config: &Config) -> Result<Vec<String>> {
    let (target_host, target_port) = parse_udp_endpoint(&config.stream_endpoint)?;
    Ok(vec![
        "-q".into(),
        "rfbsrc".into(),
        format!("host={}", config.vnc_host),
        format!("port={}", config.vnc_port),
        "view-only=true".into(),
        "shared=true".into(),
        "incremental=true".into(),
        "do-timestamp=true".into(),
        "!".into(),
        "videoconvert".into(),
        "!".into(),
        "videoscale".into(),
        "!".into(),
        "videorate".into(),
        "!".into(),
        format!(
            "video/x-raw,format=NV12,width={},height={},framerate={}/1",
            config.width, config.height, config.fps
        ),
        "!".into(),
        "queue".into(),
        "max-size-buffers=1".into(),
        "max-size-bytes=0".into(),
        "max-size-time=0".into(),
        "leaky=downstream".into(),
        "!".into(),
        "cudaupload".into(),
        "!".into(),
        "nvh264enc".into(),
        "zerolatency=true".into(),
        "bframes=0".into(),
        "rc-lookahead=0".into(),
        format!("gop-size={}", config.fps),
        format!("bitrate={}", config.bitrate_kbps),
        "repeat-sequence-header=true".into(),
        "!".into(),
        "h264parse".into(),
        "config-interval=-1".into(),
        "!".into(),
        "mpegtsmux".into(),
        "alignment=7".into(),
        "!".into(),
        "udpsink".into(),
        format!("host={target_host}"),
        format!("port={target_port}"),
        "sync=false".into(),
        "async=false".into(),
    ])
}

fn spawn_pipeline(config: &Config) -> Result<Child> {
    Command::new(&config.gst_launch)
        .args(pipeline_args(config)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start QEMU display pipeline")
}

fn announce(bus: &Bus, config: &Config) -> Result<()> {
    let now = canvas_media_edge::now_unix_ms();
    let stream = VideoStream {
        schema_version: MEDIA_SCHEMA_VERSION,
        stream_id: config.stream_id.clone(),
        display_name: config.display_name.clone(),
        zone: config.destination.clone(),
        transport: VideoTransport::UdpMpegTs,
        transport_role: TransportRole::Listen,
        endpoint: config.advertised_endpoint.clone(),
        codec: VideoCodec::H264,
        width: config.width,
        height: config.height,
        frame_rate_millihz: config.fps * 1_000,
        announced_at_unix_ms: now,
        valid_until_unix_ms: now + config.ttl_ms,
    };
    stream.validate().map_err(anyhow::Error::msg)?;
    let envelope = bus.to_destination(
        &config.destination,
        Message::new(VIDEO_STREAM_CONTRACT, serde_json::to_value(stream)?),
    )?;
    bus.send(&envelope, &[]).context("announce QEMU display")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            bus: serde_json::from_value(json!({
                "installation_id":"home", "producer_id":"qemu", "hub_endpoint":"127.0.0.1:1",
                "token_file":"/tmp/token", "zones":["compute"], "capabilities":["canvas.media"]
            }))
            .unwrap(),
            destination: "living-room".into(),
            gst_launch: "/usr/bin/gst-launch-1.0".into(),
            qmp_socket: "/run/qemu/demo.qmp".into(),
            vnc_host: "127.0.0.1".into(),
            vnc_port: 5901,
            stream_id: "qemu-demo".into(),
            display_name: "QEMU Demo".into(),
            stream_endpoint: "udp://192.0.2.10:55300?pkt_size=1316".into(),
            advertised_endpoint: "udp://0.0.0.0:55300".into(),
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_kbps: 8_000,
            ttl_ms: 30_000,
        }
    }

    #[test]
    fn geometry_is_coupled_in_caps_and_announcement_source() {
        let config = config();
        let args = pipeline_args(&config).unwrap();
        assert!(
            args.contains(&"video/x-raw,format=NV12,width=1280,height=720,framerate=30/1".into())
        );
        assert!(!args.iter().any(|arg| arg.contains("ximagesrc")));
    }

    #[test]
    fn endpoint_is_parsed_without_a_shell() {
        assert_eq!(
            parse_udp_endpoint("udp://192.0.2.10:55300?pkt_size=1316").unwrap(),
            ("192.0.2.10".into(), 55300)
        );
        assert!(parse_udp_endpoint("$(touch /tmp/nope)").is_err());
    }
}
