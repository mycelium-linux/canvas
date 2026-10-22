//! Decode an operator-selected LAN television endpoint into Canvas's native frame plane.
use anyhow::{Context, Result, ensure};
use canvas_media_edge::{
    DecodePolicy, HardwareDecoder, RawVideoPlan, gstreamer_raw_plan, publish_frame_size,
    write_raw_frame,
};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use unibus_media::{CameraStream, MEDIA_SCHEMA_VERSION, TransportRole, VideoCodec, VideoTransport};

// Macroblock-aligned in both dimensions for Pi decoder/converter negotiation.
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 576;
#[cfg(test)]
const FRAME_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;

struct Decoder(Child);
impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let rest = endpoint
        .strip_prefix("http://")
        .context("expected an explicit LAN HTTP stream URL")?;
    let (authority, path) = rest
        .split_once('/')
        .context("stream URL requires a channel path")?;
    ensure!(
        !authority.contains('@') && !authority.contains('?') && !authority.contains('#'),
        "credentials and URL fragments are not supported"
    );
    let host = authority.split(':').next().unwrap_or_default();
    let address: std::net::Ipv4Addr = host
        .parse()
        .context("stream host must be an explicit IPv4 address")?;
    ensure!(
        address.is_private() || address.is_loopback(),
        "only private LAN endpoints are accepted"
    );
    let plain_path = path.split('?').next().unwrap_or_default();
    let hls = plain_path.ends_with(".m3u8");
    ensure!(
        hls || path.starts_with("auto/v")
            || path.starts_with("tuner0/v")
            || path.starts_with("tuner1/v"),
        "expected a private HLS playlist or HDHomeRun channel"
    );
    ensure!(
        !path.contains('#') && !path.contains("..") && (hls || !path.contains('?')),
        "invalid media path"
    );
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 3 && arguments[0] == "describe" {
        let (width, height) = fit_geometry(arguments[1].parse()?, arguments[2].parse()?)?;
        println!("{}", serde_json::json!({"width":width,"height":height}));
        return Ok(());
    }
    if arguments.first().is_some_and(|s| s == "describe") && arguments.len() <= 2 {
        let (width, height) = if let Some(endpoint) = arguments.get(1) {
            stream_geometry(endpoint)?
        } else {
            (WIDTH, HEIGHT)
        };
        println!("{}", serde_json::json!({"width":width,"height":height}));
        return Ok(());
    }
    ensure!(
        arguments.len() == 2 || arguments.len() == 4,
        "usage: canvas-tv <source-id> <private URL> [width height]"
    );
    let source = &arguments[0];
    if arguments[1].starts_with("udp://") {
        return run_hardware(source, &arguments[1]);
    }
    let pipe = arguments[1] == "-";
    if pipe {
        ensure!(
            arguments.len() == 4,
            "pipe input requires explicit frame geometry"
        );
    } else {
        validate_endpoint(&arguments[1])?;
    }
    let (width, height) = if arguments.len() == 4 {
        (arguments[2].parse::<u32>()?, arguments[3].parse::<u32>()?)
    } else {
        (WIDTH, HEIGHT)
    };
    ensure!(
        (16..=4096).contains(&width) && (16..=4096).contains(&height),
        "invalid frame geometry"
    );
    let frame_bytes = (u64::from(width) * u64::from(height) * 4) as usize;
    ensure!(
        frame_bytes <= canvas_media_edge::MAX_FRAME_BYTES,
        "frame allocation exceeds bound"
    );
    // One geometry owns the sidecar, decoder output, frame length and Canvas texture.
    publish_frame_size(source, width, height)?;
    let filter = format!(
        "yadif=mode=send_frame:deint=interlaced,scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=24"
    );
    let mut decoder = Decoder(
        Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "warning",
                "-rw_timeout",
                "15000000",
                "-i",
            ])
            .arg(if pipe { "pipe:0" } else { &arguments[1] })
            .args([
                "-map", "0:v:0", "-an", "-vf", &filter, "-pix_fmt", "rgba", "-f", "rawvideo",
                "pipe:1",
            ])
            .stdin(if pipe {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start FFmpeg television decoder")?,
    );
    let mut output = decoder.0.stdout.take().context("decoder stdout missing")?;
    let mut frame = vec![0; frame_bytes];
    loop {
        output
            .read_exact(&mut frame)
            .context("television decoder stopped before a complete frame")?;
        // No unbounded frame queue and no SD-card writes: the shared publisher
        // atomically replaces a single frame in XDG_RUNTIME_DIR.
        write_raw_frame(source, frame_bytes, &frame)?;
    }
}

fn fit_geometry(width: u32, height: u32) -> Result<(u32, u32)> {
    ensure!(
        (16..=8192).contains(&width) && (16..=8192).contains(&height),
        "invalid source geometry"
    );
    let scale = (1024.0 / f64::from(width.max(height))).min(1.0);
    Ok((
        ((f64::from(width) * scale) as u32).max(16) & !1,
        ((f64::from(height) * scale) as u32).max(16) & !1,
    ))
}

fn stream_geometry(endpoint: &str) -> Result<(u32, u32)> {
    validate_endpoint(endpoint)?;
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-rw_timeout",
            "3000000",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
            endpoint,
        ])
        .output()
        .context("probe private media geometry")?;
    ensure!(
        output.status.success() && output.stdout.len() <= 65536,
        "private media probe failed"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let width: u32 = value["streams"][0]["width"]
        .as_u64()
        .context("missing video width")?
        .try_into()?;
    let height: u32 = value["streams"][0]["height"]
        .as_u64()
        .context("missing video height")?
        .try_into()?;
    fit_geometry(width, height)
}

fn hardware_plan(source: &str, endpoint: &str) -> Result<RawVideoPlan> {
    let authority = endpoint
        .strip_prefix("udp://")
        .context("expected UDP endpoint")?;
    let (host, port) = authority
        .rsplit_once(':')
        .context("UDP endpoint requires a port")?;
    let address: std::net::Ipv4Addr = host.parse().context("expected private IPv4 endpoint")?;
    ensure!(
        address.is_private() || address.is_loopback(),
        "expected private endpoint"
    );
    ensure!(
        port.parse::<u16>().context("invalid UDP port")? != 0,
        "UDP port cannot be zero"
    );
    gstreamer_raw_plan(
        &CameraStream {
            schema_version: MEDIA_SCHEMA_VERSION,
            camera_id: source.into(),
            display_name: "TV".into(),
            zone: "home".into(),
            transport: VideoTransport::UdpMpegTs,
            transport_role: TransportRole::Listen,
            endpoint: endpoint.into(),
            codec: VideoCodec::H264,
            width: WIDTH,
            height: HEIGHT,
            frame_rate_millihz: 30_000,
            announced_at_unix_ms: 1,
            valid_until_unix_ms: 60_001,
        },
        DecodePolicy {
            max_height: HEIGHT,
            max_fps: 30.0,
            decoder: HardwareDecoder::V4l2M2m,
        },
    )
}

fn run_hardware(source: &str, endpoint: &str) -> Result<()> {
    let plan = hardware_plan(source, endpoint)?;
    publish_frame_size(source, plan.width, plan.height)?;
    let mut decoder = Decoder(
        Command::new("gst-launch-1.0")
            .args(&plan.argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start Pi hardware television decoder")?,
    );
    let mut output = decoder.0.stdout.take().context("decoder stdout missing")?;
    let mut allocation = vec![0; plan.allocation_bytes];
    loop {
        output
            .read_exact(&mut allocation)
            .context("hardware television decoder stopped")?;
        let frame = plan.visible_frame(&allocation)?;
        write_raw_frame(source, plan.frame_bytes()?, &frame)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_is_private_and_explicit() {
        assert!(validate_endpoint("http://192.168.2.74:5004/auto/v4.1").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:18089/tardy-public.m3u8?generation=2").is_ok());
        for url in [
            "http://example.com/auto/v4.1",
            "http://8.8.8.8/auto/v4.1",
            "http://secret@192.168.2.74/auto/v4.1",
            "http://192.168.2.74/../auto/v4.1",
            "http://192.168.2.74/auto/v4.1?url=other",
        ] {
            assert!(validate_endpoint(url).is_err(), "{url}");
        }
    }
    #[test]
    fn geometry_matches_tightly_packed_rgba() {
        assert_eq!(FRAME_BYTES, 2_359_296);
        assert_eq!((WIDTH * 4) % 256, 0);
    }
    #[test]
    fn portrait_and_landscape_preserve_aspect_and_bound_pixels() {
        assert_eq!(fit_geometry(1080, 1920).unwrap(), (576, 1024));
        assert_eq!(fit_geometry(1920, 1080).unwrap(), (1024, 576));
        assert_eq!(fit_geometry(360, 640).unwrap(), (360, 640));
        assert!(fit_geometry(0, 100).is_err());
    }
    #[test]
    fn hardware_uses_shared_geometry_and_bounded_queue() {
        let plan = hardware_plan("komo4", "udp://192.168.2.74:55304").unwrap();
        assert_eq!((plan.width, plan.height), (WIDTH, HEIGHT));
        assert_eq!(plan.allocation_bytes, FRAME_BYTES);
        assert!(plan.argv.iter().any(|arg| arg == "v4l2h264dec"));
        assert!(plan.argv.iter().any(|arg| arg == "v4l2convert"));
        assert!(plan.argv.iter().any(|arg| arg == "max-size-buffers=1"));
        for endpoint in [
            "udp://8.8.8.8:55304",
            "udp://192.168.2.74:0",
            "udp://192.168.2.74:55304?inject=true",
        ] {
            assert!(hardware_plan("komo4", endpoint).is_err());
        }
    }
}
