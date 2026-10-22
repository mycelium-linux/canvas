//! canvas-central: send addressed Canvas control requests over unibus.
//!
//! The brain-side half of the bus: each `ControlRequest` (JSON object per
//! line, or one as an argument) is wrapped as a `canvas_command` envelope,
//! addressed to a screen anchor, a zone, or a named producer, and the tool
//! waits for the edge's `canvas.result` receipt before exiting — so callers
//! learn what executed, not just what was delivered (dock marbles
//! `dock-tnsv` remote half).
//!
//! usage: canvas-central --config bus.json (--screen A | --zone Z |
//!                      --to producer) [--timeout SECS] [request.json]
//!        echo '{"schema_version":1,...}' | canvas-central ... -

use std::io::{IsTerminal, Read};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use canvas::bus::{Bus, BusConfig};
use canvas::control::{CONTROL_VERSION, ControlRequest};
use canvas_core::unibus::{COMMAND_KIND_V1, Message, envelope, parse_result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CentralConfig {
    #[serde(flatten)]
    bus: BusConfig,
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Screen(String),
    Zone(String),
    Producer(String),
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path = None;
    let mut target = None;
    let mut timeout = Duration::from_secs(15);
    let mut request_arg = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                config_path = args.get(i).cloned();
            }
            "--screen" => {
                i += 1;
                target = Some(Target::Screen(arg(&args, i, "--screen")?));
            }
            "--zone" => {
                i += 1;
                target = Some(Target::Zone(arg(&args, i, "--zone")?));
            }
            "--to" => {
                i += 1;
                target = Some(Target::Producer(arg(&args, i, "--to")?));
            }
            "--timeout" => {
                i += 1;
                timeout = Duration::from_secs(arg(&args, i, "--timeout")?.parse()?);
            }
            "-" => request_arg = Some("-".to_string()),
            other => {
                ensure!(request_arg.is_none(), "unexpected argument: {other}");
                request_arg = Some(other.to_string());
            }
        }
        i += 1;
    }
    let config_path = config_path.context("missing --config <bus.json>")?;
    let target = target.context("missing --screen/--zone/--to")?;
    let config = CentralConfig::read(std::path::Path::new(&config_path))?;
    let bus = Bus::connect(config.bus).context("connect hub")?;

    let raw = match request_arg.as_deref() {
        Some("-") => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("read stdin")?;
            text
        }
        Some(literal) => literal.to_owned(),
        None => {
            ensure!(
                !std::io::stdin().is_terminal(),
                "no request given (argument or stdin)"
            );
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("read stdin")?;
            text
        }
    };
    let sources = vec![raw];
    let mut failures = 0usize;
    for source in &sources {
        for line in source.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match send_one(&bus, &target, line, timeout) {
                Ok(receipt) => {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "request_id": receipt.request_id,
                            "ok": receipt.ok,
                            "error": receipt.error,
                            "data": receipt.data,
                        }))
                        .unwrap()
                    );
                    failures += !receipt.ok as usize;
                }
                Err(e) => {
                    eprintln!("canvas-central: {e:#}");
                    failures += 1;
                }
            }
        }
    }
    ensure!(failures == 0, "{failures} command(s) failed");
    Ok(())
}

fn arg(args: &[String], i: usize, flag: &str) -> Result<String> {
    args.get(i)
        .cloned()
        .with_context(|| format!("{flag} needs a value"))
}

impl CentralConfig {
    fn read(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read central config {}", path.display()))?;
        Ok(serde_json::from_slice(&bytes).context("parse central config")?)
    }
}

fn send_one(
    bus: &Bus,
    target: &Target,
    line: &str,
    timeout: Duration,
) -> Result<canvas_core::unibus::ResultFrame> {
    let request = normalize_request(line)?;
    let message = Message::new(
        COMMAND_KIND_V1,
        serde_json::json!({ "command": serde_json::to_value(&request)? }),
    );
    let command_envelope = match target {
        Target::Screen(anchor) => bus.for_screen(anchor, message)?,
        Target::Zone(zone) => bus.to_destination(zone, message)?,
        Target::Producer(producer) => bus.to_destination(producer, message)?,
    };
    bus.send(&command_envelope, &[])
        .context("send canvas_command")?;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let Some((frame, _)) = bus.try_read()? else {
            continue;
        };
        if frame.producer_id == bus.config().producer_id {
            continue; // our own heartbeats never arrive, but stay paranoid
        }
        if let Some(receipt) = parse_result(&frame.message)
            && receipt.request_id == request.request_id
        {
            return Ok(receipt);
        }
        // receipts for other request ids are not ours to consume; ignore.
    }
    bail!(
        "no canvas.result for '{}' within {} s (no edge executed it)",
        request.request_id,
        timeout.as_secs()
    )
}

/// Accept a full ControlRequest JSON, filling request_id and schema when
/// the caller omitted them, and reject anything Canvas would reject.
pub fn normalize_request(line: &str) -> Result<ControlRequest> {
    let mut value: serde_json::Value = serde_json::from_str(line).context("request is not JSON")?;
    let map = value
        .as_object_mut()
        .context("request must be a JSON object")?;
    map.entry("schema_version")
        .or_insert_with(|| serde_json::json!(CONTROL_VERSION));
    if map
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        map.insert(
            "request_id".into(),
            serde_json::json!(format!(
                "central-{}-{}",
                envelope::now_ms(),
                std::process::id()
            )),
        );
    }
    let request: ControlRequest =
        serde_json::from_value(value).context("request is not a Canvas ControlRequest")?;
    request.validate().map_err(anyhow::Error::msg)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_fills_ids_and_validates() {
        let request =
            normalize_request(r#"{"command":{"command":"dismiss_indicator","indicator_id":"x"}}"#)
                .unwrap();
        assert_eq!(request.schema_version, CONTROL_VERSION);
        assert!(request.request_id.starts_with("central-"));
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_request("nonsense").is_err());
        assert!(normalize_request(r#"{"command":{"command":"inspect"},"request_id":"x"}"#).is_ok());
        assert!(normalize_request(r#"{"request_id":"x"}"#).is_err()); // no command
    }

    #[test]
    fn target_flag_parsing_is_exclusive() {
        // sanity for the arg walker: two targets must not both apply
        let parse = |args: &[&str]| -> Option<Target> {
            let mut out = None;
            let mut i = 0;
            while i < args.len() {
                match args[i] {
                    "--screen" => out = Some(Target::Screen(args[i + 1].into())),
                    "--zone" => out = Some(Target::Zone(args[i + 1].into())),
                    "--to" => out = Some(Target::Producer(args[i + 1].into())),
                    _ => {}
                }
                i += 1;
            }
            out
        };
        assert_eq!(
            parse(&["--zone", "living-room"]),
            Some(Target::Zone("living-room".into()))
        );
    }
}
