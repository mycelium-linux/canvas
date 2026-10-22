//! One-shot, read-only status adapter. Caller owns delivery and polling cadence.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket = std::env::args()
        .nth(1)
        .ok_or("usage: canvas-agent-status APP_SERVER_SOCKET")?;
    println!(
        "{}",
        canvas_feeds::agent_status::observe(std::path::Path::new(&socket))?
    );
    Ok(())
}
