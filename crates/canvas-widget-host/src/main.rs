//! Canvas-owned bounded Lua composition and presentation delivery, not provider IO.
#[cfg(feature = "luau")]
use mlua::LuaSerdeExt;
#[cfg(feature = "luau")]
use std::io::{BufRead, Read, Write};
#[cfg(not(feature = "luau"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("widget_projection requires bounded Luau runtime".into())
}
#[cfg(feature = "luau")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "stream-tv") {
        if args.len() != 6 {
            return Err(
                "usage: canvas-widget-host stream-tv BINDER TVCTL ENDPOINT WORKSPACE SURFACE"
                    .into(),
            );
        }
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = Vec::new();
            let count = stdin
                .by_ref()
                .take(64 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                return Ok(());
            }
            if count > 64 * 1024 || !line.ends_with(b"\n") {
                return Err("oversized/incomplete observation".into());
            }
            let result = compose(&args[1], &line)?;
            let mut frame = result["widget"].clone();
            frame
                .as_object_mut()
                .ok_or("invalid widget frame")?
                .remove("kind");
            let request = serde_json::json!({"schema_version":1,"request_id":format!("widget-{}",chrono::Utc::now().timestamp_millis()),"command":{"command":"surface_snapshot","workspace_id":args[4],"surface_id":args[5],"frame":frame,"data":result["data"]}});
            let mut child = std::process::Command::new(&args[2])
                .arg(&args[3])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            child
                .stdin
                .take()
                .ok_or("missing TV input")?
                .write_all(&serde_json::to_vec(&request)?)?;
            let mut stdout = child.stdout.take().ok_or("missing TV reply")?;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = stdout.by_ref().take(64 * 1024 + 1).read_to_end(&mut bytes);
                let _ = tx.send(read.map(|_| bytes));
            });
            let response = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(bytes) => bytes?,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("TV delivery deadline exceeded".into());
                }
            };
            let status = child.wait()?;
            if response.len() > 64 * 1024 || !status.success() {
                return Err("TV delivery failed".into());
            }
            let reply: serde_json::Value = serde_json::from_slice(&response)?;
            // The legacy TV owner receipt is {ok, revision, data}, without a
            // request ID. Each trusted local CLI invocation owns one connection.
            if reply["ok"] != true || !reply["revision"].is_u64() {
                return Err("TV rejected widget delivery".into());
            }
            println!("delivered {} {}", args[4], args[5]);
            std::io::stdout().flush()?;
        }
    }
    let path = args
        .first()
        .ok_or("usage: canvas-widget-host BINDER.lua < OBSERVATION.json")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 64 * 1024 {
        return Err("observation exceeds 64 KiB".into());
    }
    println!("{}", compose(path, &input)?);
    Ok(())
}
#[cfg(feature = "luau")]
fn compose(path: &str, input: &[u8]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let script = std::fs::read(path)?;
    if script.len() > 64 * 1024 {
        return Err("binder exceeds 64 KiB".into());
    }
    compose_script(&script, input)
}
#[cfg(feature = "luau")]
fn compose_script(
    script: &[u8],
    input: &[u8],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_slice(input)?;
    let lua = mlua::Lua::new();
    lua.set_memory_limit(1024 * 1024)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    lua.set_interrupt(move |_| {
        if std::time::Instant::now() >= deadline {
            Err(mlua::Error::RuntimeError(
                "widget composition deadline exceeded".into(),
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    let module: mlua::Table = lua.load(script).eval()?;
    let render: mlua::Function = module.get("render")?;
    let now = chrono::Utc::now().timestamp_millis();
    let output: mlua::Value = render.call((lua.to_value(&value)?, now))?;
    let mut result: serde_json::Value = lua.from_value(output)?;
    if let Some(stamp) = result["data"]["observed_at_unix_ms"].as_i64() {
        result["data"]
            .as_object_mut()
            .ok_or("missing metadata")?
            .remove("observed_at_unix_ms");
        result["data"]["observed_at"] = chrono::DateTime::from_timestamp_millis(stamp)
            .ok_or("invalid observation time")?
            .to_rfc3339()
            .into();
    }
    let bytes = serde_json::to_vec(&result)?;
    if bytes.len() > 64 * 1024 {
        return Err("widget output exceeds 64 KiB".into());
    }
    Ok(result)
}

#[cfg(all(test, feature = "luau"))]
mod tests {
    use super::*;
    #[test]
    fn preserves_observation_clock_not_render_time() {
        let result=compose_script(br#"return {render=function(value,now) return {widget={kind='status_list',title='Evidence'},data={observed_at_unix_ms=value.observed_at_unix_ms}} end}"#,br#"{"observed_at_unix_ms":1000}"#).unwrap();
        assert_eq!(result["data"]["observed_at"], "1970-01-01T00:00:01+00:00");
        assert!(result["data"].get("observed_at_unix_ms").is_none());
    }
    #[test]
    fn nonterminating_binder_is_bounded() {
        let start = std::time::Instant::now();
        let result = compose_script(b"return {render=function() while true do end end}", b"{}");
        assert!(result.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    #[test]
    fn oversized_output_is_rejected() {
        assert!(compose_script(
            b"return {render=function() return {widget={body=string.rep('x',70000)}} end}",
            b"{}"
        )
        .is_err());
    }
}
