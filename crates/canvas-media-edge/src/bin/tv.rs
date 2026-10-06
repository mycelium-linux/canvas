//! Decode an operator-selected LAN television endpoint into Canvas's native frame plane.
use anyhow::{ensure, Context, Result};
use canvas_media_edge::{
    gstreamer_raw_plan, publish_frame_size, write_raw_frame, DecodePolicy, HardwareDecoder,
    RawVideoPlan,
};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use unibus_media::{CameraStream, TransportRole, VideoCodec, VideoTransport, MEDIA_SCHEMA_VERSION};

// Macroblock-aligned in both dimensions for Pi decoder/converter negotiation.
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 576;
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
    ensure!(
        path.starts_with("auto/v") || path.starts_with("tuner0/v") || path.starts_with("tuner1/v"),
        "expected an HDHomeRun channel path"
    );
    ensure!(
        !path.contains(['?', '#']) && !path.contains(".."),
        "invalid channel path"
    );
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        arguments.len() == 2,
        "usage: canvas-tv <source-id> <http://LAN:5004/auto/vCHANNEL | udp://LAN:PORT>"
    );
    let source = &arguments[0];
    if arguments[1].starts_with("udp://") {
        return run_hardware(source, &arguments[1]);
    }
    validate_endpoint(&arguments[1])?;
    // One geometry owns the sidecar, decoder output, frame length and Canvas texture.
    publish_frame_size(source, WIDTH, HEIGHT)?;
    let filter = format!("yadif=mode=send_frame:deint=interlaced,scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=decrease,pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=24");
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
            .arg(&arguments[1])
            .args([
                "-map", "0:v:0", "-an", "-vf", &filter, "-pix_fmt", "rgba", "-f", "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start FFmpeg television decoder")?,
    );
    let mut output = decoder.0.stdout.take().context("decoder stdout missing")?;
    let mut frame = vec![0; FRAME_BYTES];
    loop {
        output
            .read_exact(&mut frame)
            .context("television decoder stopped before a complete frame")?;
        // No unbounded frame queue and no SD-card writes: the shared publisher
        // atomically replaces a single frame in XDG_RUNTIME_DIR.
        write_raw_frame(source, FRAME_BYTES, &frame)?;
    }
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
