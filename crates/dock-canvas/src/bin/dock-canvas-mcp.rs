fn main() -> Result<(), String> {
    #[cfg(unix)]
    {
        dock_canvas::mcp::serve()
    }
    #[cfg(not(unix))]
    {
        Err("Canvas MCP requires the Unix owner-local control transport on this platform".into())
    }
}
