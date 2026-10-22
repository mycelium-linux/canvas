//! The Canvas MCP adapter.
//!
//! With no arguments it speaks stdio, which is what a client that can spawn the
//! process wants. `http PORT` serves the same tools on loopback for FPL Desktop
//! Bridge, which cannot spawn anything: it advertises a capability only after
//! probing an address, so the HTTP mode is what makes Canvas reachable from the
//! cloud rather than only from this machine.
fn main() -> Result<(), String> {
    #[cfg(unix)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [] => canvas::mcp::serve(),
            ["http", port] => {
                let port = port
                    .parse()
                    .map_err(|_| format!("Not a port number: {port}"))?;
                canvas::mcp::serve_http(port)
            }
            _ => Err("usage: canvas-mcp [http PORT]".into()),
        }
    }
    #[cfg(not(unix))]
    {
        Err("Canvas MCP requires the Unix owner-local control transport on this platform".into())
    }
}
