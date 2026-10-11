//! Bounded local ingress CLI using Canvas's shared framing and socket owner.
use std::io::Read;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    std::io::stdin().take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("widget ingress exceeds 64KiB".into());
    }
    let event: serde_json::Value = serde_json::from_slice(&bytes)?;
    if event["schema_version"] != 1 || event["contract"] != "canvas.widget.snapshot.v1" {
        return Err("widget ingress contract rejected".into());
    }
    for chunk in canvas_feeds::chunk_envelopes(&event, 2048)? {
        canvas_feeds::send_raw(&chunk)?;
    }
    Ok(())
}
