//! Explicit, ADB-authorized transport to the TV's presentation endpoint.
use canvas_workspace::endpoint::{Command as PresentationCommand, MAX_FRAME, Request};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::Command,
};

fn adb(serial: &str, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("adb")
        .args(["-s", serial])
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(output.stdout)
}

struct Forward {
    serial: String,
    local: String,
}
impl Drop for Forward {
    fn drop(&mut self) {
        let _ = adb(&self.serial, &["forward", "--remove", &self.local]);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let serial = std::env::args()
        .nth(1)
        .ok_or("usage: canvas-tvctl ADB_SERIAL < request.json")?;
    if std::env::args().count() != 2 {
        return Err("Expected exactly one explicit ADB target".into());
    }
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_FRAME as u64 + 1)
        .read_to_string(&mut raw)?;
    if raw.len() > MAX_FRAME {
        return Err("Request exceeds 4 MiB".into());
    }
    let text_mode = !raw.trim_start().starts_with('{');
    let mut request: Request = if text_mode {
        let words: Vec<&str> = raw.split_whitespace().collect();
        let command =
            match words.as_slice() {
                ["inspect"] | ["workspace", "list"] => PresentationCommand::Inspect,
                ["doc"] => PresentationCommand::ReadDocument,
                ["workspace", "switch", name] => PresentationCommand::WorkspaceSwitch {
                    name: (*name).into(),
                },
                ["workspace", "read", name] => PresentationCommand::WorkspaceRead {
                    name: (*name).into(),
                },
                ["workspace", "install", path] => {
                    let file = std::fs::File::open(path)?;
                    let mut input = String::new();
                    file.take(MAX_FRAME as u64 + 1).read_to_string(&mut input)?;
                    if input.len() > MAX_FRAME {
                        return Err("Install file exceeds limit".into());
                    }
                    serde_json::from_str::<Request>(&input)?.command
                }
                _ => return Err(
                    "Use inspect, doc, workspace list/read/switch/install, or presentation JSON"
                        .into(),
                ),
            };
        Request {
            schema_version: 1,
            request_id: "tvctl".into(),
            expected_revision: None,
            command,
        }
    } else {
        serde_json::from_str(&raw)?
    };
    let token = adb(
        &serial,
        &[
            "shell",
            "run-as",
            "dev.fpl.canvas.tv",
            "cat",
            "files/control-token",
        ],
    )?;
    if token.len() != 64 || !token.iter().all(u8::is_ascii_hexdigit) {
        return Err(
            "Private TV credential unavailable; start the authorized development app".into(),
        );
    }
    let forwarded = adb(&serial, &["forward", "tcp:0", &format!("tcp:{}", canvas_android::CONTROL_PORT)])?;
    let port: u16 = String::from_utf8(forwarded)?.trim().parse()?;
    let guard = Forward {
        serial,
        local: format!("tcp:{port}"),
    };
    if text_mode
        && matches!(
            request.command,
            PresentationCommand::Install { .. } | PresentationCommand::WorkspaceSwitch { .. }
        )
    {
        let observed = exchange(
            port,
            &token,
            &Request {
                schema_version: 1,
                request_id: "tvctl-observe".into(),
                expected_revision: None,
                command: PresentationCommand::Inspect,
            },
        )?;
        request.expected_revision = observed["revision"].as_u64();
    }
    let receipt = exchange(port, &token, &request)?;
    println!("{}", serde_json::to_string(&receipt)?);
    drop(guard);
    if receipt["ok"] != true {
        return Err("TV rejected presentation request".into());
    }
    Ok(())
}

fn exchange(
    port: u16,
    token: &[u8],
    request: &Request,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_secs(3),
    )?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.write_all(token)?;
    stream.write_all(b"\n")?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let mut bytes = Vec::new();
    BufReader::new(stream.take(MAX_FRAME as u64 + 1)).read_until(b'\n', &mut bytes)?;
    if bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err("Incomplete or oversized TV receipt".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
