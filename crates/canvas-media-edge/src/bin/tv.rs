//! Decode an operator-selected LAN television endpoint into Canvas's native frame plane.
use anyhow::{ensure, Context, Result};
use canvas_media_edge::{publish_frame_size, write_raw_frame};
use std::io::Read;
use std::process::{Child, Command, Stdio};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
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
        "usage: canvas-tv <source-id> <http://LAN:5004/auto/vCHANNEL>"
    );
    let source = &arguments[0];
    validate_endpoint(&arguments[1])?;
    // One geometry owns the sidecar, decoder output, frame length and Canvas texture.
    publish_frame_size(source, WIDTH, HEIGHT)?;
    let mut decoder = Decoder(Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "warning", "-rw_timeout", "15000000", "-i"])
        .arg(&arguments[1])
        .args(["-map", "0:v:0", "-an", "-vf", "yadif=mode=send_frame:deint=interlaced,scale=960:540:force_original_aspect_ratio=decrease,pad=960:540:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=24", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit())
        .spawn().context("start FFmpeg television decoder")?);
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
        assert_eq!(FRAME_BYTES, 2_073_600);
        assert_eq!((WIDTH * 4) % 256, 0);
    }
}
