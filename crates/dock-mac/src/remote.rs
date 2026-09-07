//! Bounded SSH client for the fixed project desktop/session API.
use serde_json::Value;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
/// SSH joins remote arguments through a shell. Quote every argument once; never
/// insert user prompt text into executable shell syntax.
pub fn rpc(
    project: &str,
    action: &str,
    args: &[String],
    input: Option<String>,
) -> Result<Value, String> {
    if !super::session_identifier(project) {
        return Err("Invalid project identifier".into());
    }
    if action.starts_with("session-") && super::edgerunner::binding(project)?.is_some() {
        return Err("This project references an Edgerunner-owned session. The local proof-session adapter is disabled for it; use edgerunner bound-status until owner attachment is connected.".into());
    }
    if !allowed_action(action) {
        return Err("Unsupported Dock remote operation".into());
    }
    if input.as_ref().is_some_and(|s| s.len() > 65536) {
        return Err("Prompt exceeds 64 KiB".into());
    }
    let command = std::iter::once(".local/bin/dock-linux")
        .chain([action, project])
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ");
    let mut ssh = Command::new("ssh");
    if let Some(home) = std::env::var_os("HOME") {
        let socket = std::path::PathBuf::from(home)
            .join("Library/Application Support/Dock/desktop-ssh.sock");
        if socket.exists() {
            ssh.arg("-S").arg(socket);
        }
    }
    let mut child = ssh
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "agora-one",
            &command,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdin = child.stdin.take().unwrap();
    let input_thread = std::thread::spawn(move || {
        let mut stdin = stdin;
        if let Some(body) = input {
            stdin.write_all(body.as_bytes())?;
        }
        Ok::<_, std::io::Error>(())
    });
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    const MAX: u64 = 24 * 1024 * 1024;
    let out_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        stdout.take(MAX + 1).read_to_end(&mut v).map(|_| v)
    });
    let err_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        stderr.take(65537).read_to_end(&mut v).map(|_| v)
    });
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Agora One did not answer within 45 seconds; inspect status before retrying a prompt".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = out_thread
        .join()
        .map_err(|_| "Remote output reader failed")?
        .map_err(|e| e.to_string())?;
    let stderr = err_thread
        .join()
        .map_err(|_| "Remote error reader failed")?
        .map_err(|e| e.to_string())?;
    input_thread
        .join()
        .map_err(|_| "Prompt writer failed")?
        .map_err(|e| format!("Could not send prompt: {e}"))?;
    if stdout.len() as u64 > MAX {
        return Err("Remote response exceeds 24 MiB".into());
    }
    let value: Value = serde_json::from_slice(&stdout).map_err(|e| {
        format!(
            "Invalid Agora One response: {e}; {}",
            String::from_utf8_lossy(&stderr).trim()
        )
    })?;
    envelope(project, status.success(), value)
}

fn allowed_action(action: &str) -> bool {
    matches!(
        action,
        "session-ensure"
            | "session-start"
            | "session-status"
            | "session-prompt"
            | "session-resume"
            | "session-abort"
            | "session-attach"
            | "desktop-status"
            | "desktop-capture"
            | "desktop-text"
            | "desktop-key"
            | "desktop-click"
            | "desktop-control"
            | "desktop-recall"
            | "desktop-browser"
            | "desktop-browser-url"
            | "desktop-web-url"
            | "desktop-web-status"
    )
}
pub fn envelope(project: &str, success: bool, value: Value) -> Result<Value, String> {
    if value["schema_version"] != 1 {
        return Err("Unsupported Agora One response version".into());
    }
    if !success || value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Agora One rejected the operation")
            .into());
    }
    let result = value
        .get("result")
        .filter(|r| r.is_object())
        .ok_or("Missing remote result")?;
    if let Some(actual) = result.get("project").and_then(|v| v.as_str()) {
        if actual != project {
            return Err("Agora One returned another project's result".into());
        }
    }
    Ok(result.clone())
}
pub(crate) const VIEWER: &str = "/Applications/TigerVNC.app/Contents/MacOS/vncviewer";
fn endpoint(index: usize) -> Result<String, String> {
    if !(1..=2).contains(&index) {
        return Err("Dock currently supports one or two project outputs".into());
    }
    Ok(format!("127.0.0.1::{}", 15900 + index))
}
fn rfb_ready(port: u16) -> bool {
    use std::io::Read;
    use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    TcpStream::connect_timeout(&address.into(), Duration::from_millis(250)).is_ok_and(
        |mut stream| {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut greeting = [0; 4];
            stream.read_exact(&mut greeting).is_ok() && &greeting == b"RFB "
        },
    )
}
fn desktop_tunnel(count: usize) -> Result<std::path::PathBuf, String> {
    if !(1..=2).contains(&count) {
        return Err("Dock currently supports one or two project outputs".into());
    }
    let support = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
        .join("Library/Application Support/Dock");
    std::fs::create_dir_all(&support).map_err(|e| e.to_string())?;
    let socket = support.join("desktop-ssh.sock");
    let checked = super::bounded_output(
        Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-S"])
            .arg(&socket)
            .args(["-O", "check", "agora-one"]),
        8,
    )?;
    if !checked.status.success() {
        let mut command = Command::new("ssh");
        command
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=2",
                "-M",
                "-S",
            ])
            .arg(&socket)
            .args(["-f", "-N"]);
        for index in 1..=count {
            command.arg("-L").arg(format!(
                "127.0.0.1:{}:127.0.0.1:{}",
                15900 + index,
                5900 + index
            ));
        }
        command.arg("agora-one");
        let tunnel = super::bounded_output(&mut command, 15)?;
        if !tunnel.status.success() {
            return Err(format!(
                "Could not open desktop SSH tunnel: {}",
                String::from_utf8_lossy(&tunnel.stderr).trim()
            ));
        }
    }
    for index in 1..=count {
        let local_port = 15900 + index as u16;
        if !rfb_ready(local_port) {
            let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{}", 5900 + index);
            let added = super::bounded_output(
                Command::new("ssh")
                    .args(["-o", "BatchMode=yes", "-S"])
                    .arg(&socket)
                    .args(["-O", "forward", "-L", &forward, "agora-one"]),
                8,
            )?;
            if !added.status.success() {
                return Err(format!(
                    "Could not add desktop output {index} to the SSH tunnel: {}",
                    String::from_utf8_lossy(&added.stderr).trim()
                ));
            }
        }
        if !rfb_ready(local_port) {
            return Err(format!("Desktop output {index} did not present RFB"));
        }
    }
    Ok(support)
}
fn viewer_command(support: &std::path::Path, index: usize) -> Result<Command, String> {
    let cert = support.join("desktop-vnc-cert.pem");
    if !cert.exists() {
        let fetched = super::bounded_output(
            Command::new("ssh").args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "agora-one",
                "cat",
                ".config/dock/wayvnc/tls-cert.pem",
            ]),
            15,
        )?;
        if !fetched.status.success() {
            return Err("Could not retrieve the desktop certificate over SSH".into());
        }
        std::fs::write(&cert, fetched.stdout).map_err(|e| e.to_string())?;
    }
    let password = super::bounded_output(
        Command::new("ssh").args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "agora-one",
            "cat",
            ".config/dock/wayvnc/password",
        ]),
        15,
    )?;
    if !password.status.success() {
        return Err("Could not retrieve desktop credentials over SSH".into());
    }
    let password =
        String::from_utf8(password.stdout).map_err(|_| "Invalid desktop credential encoding")?;
    if password.trim().is_empty() {
        return Err("Desktop credential is empty".into());
    }
    let mut command = Command::new(VIEWER);
    command
        .args(viewer_options())
        .args(["-X509CA"])
        .arg(cert)
        .arg(endpoint(index)?)
        .env("VNC_USERNAME", "dock")
        .env("VNC_PASSWORD", password.trim());
    Ok(command)
}
/// One fixed, lossless desktop surface. Multi-output composition is a server
/// capability; choosing several Mac monitors alone never creates remote outputs.
fn viewer_options() -> Vec<&'static str> {
    vec![
        "-SecurityTypes",
        "X509Plain",
        "-Shared",
        "-RemoteResize=0",
        "-AutoSelect=0",
        "-PreferredEncoding",
        "ZRLE",
        "-FullColor=1",
        "-NoJPEG",
        "-FullscreenSystemKeys=0",
        "-ShortcutModifiers",
        "Ctrl,Shift",
    ]
}
pub(crate) fn project_viewer_commands(project: &str, count: usize) -> Result<Vec<Command>, String> {
    let status = rpc(project, "desktop-status", &[], None)?;
    let active_outputs = status["outputs"]
        .as_array()
        .ok_or("Agora One omitted its output inventory")?
        .iter()
        .filter(|output| output["active"] == true)
        .count();
    if active_outputs < count {
        return Err(format!(
            "Agora One has {active_outputs} active outputs; {count} streaming displays were selected"
        ));
    }
    rpc(project, "desktop-recall", &[], None)?;
    let support = desktop_tunnel(count)?;
    (1..=count)
        .map(|index| viewer_command(&support, index))
        .collect()
}
/// Open or focus the independently running project desktop. A reservation uses
/// its own Child handle instead, so cleanup cannot terminate a borrowed viewer.
pub fn open_desktop(project: &str) -> Result<Value, String> {
    let recalled = rpc(project, "desktop-recall", &[], None)?;
    let output_index = recalled_output_index(&recalled)?;
    let support = desktop_tunnel(output_index)?;
    let endpoint = format!("127.0.0.1::{}", 15900 + output_index);
    let running = super::bounded_output(Command::new("ps").args(["-axo", "pid=,command="]), 8)?;
    if running.status.success() {
        for line in String::from_utf8_lossy(&running.stdout).lines() {
            let line = line.trim();
            if let Some((pid, command)) = line.split_once(char::is_whitespace) {
                if command
                    .trim_start()
                    .starts_with("/Applications/TigerVNC.app/Contents/MacOS/vncviewer ")
                    && command.contains(&endpoint)
                {
                    let pid: u32 = pid.parse().map_err(|_| "Invalid viewer process ID")?;
                    let activated=super::bounded_output(Command::new("osascript").args(["-l","JavaScript","-e", "ObjC.import('AppKit'); function run(args) { const app=$.NSRunningApplication.runningApplicationWithProcessIdentifier(Number(args[0])); if (!app || ObjC.unwrap(app.executableURL.path)!='/Applications/TigerVNC.app/Contents/MacOS/vncviewer') throw Error('Viewer changed'); if (!app.activateWithOptions(2)) throw Error('Cannot activate viewer'); }", &pid.to_string()]),8)?;
                    if !activated.status.success() {
                        return Err("The existing desktop viewer could not be activated".into());
                    }
                    return Ok(
                        serde_json::json!({"project":project,"output_index":output_index,"viewer_pid":pid,"reused":true,"recalled":recalled}),
                    );
                }
            }
        }
    }
    let log_path = support.join(format!("desktop-viewer-{output_index}.log"));
    use std::os::unix::fs::OpenOptionsExt;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let error_log = log.try_clone().map_err(|e| e.to_string())?;
    let mut child = viewer_command(&support, output_index)?
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log))
        .spawn()
        .map_err(|e| format!("Could not open TigerVNC: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        return Err(format!("Desktop viewer exited; see {}", log_path.display()));
    }
    Ok(
        serde_json::json!({"project":project,"output_index":output_index,"viewer_pid":child.id(),"recalled":recalled,"viewer_log":log_path}),
    )
}

fn recalled_output_index(recalled: &Value) -> Result<usize, String> {
    let output = recalled
        .pointer("/active_workspace/output")
        .and_then(Value::as_str)
        .ok_or("Recalled workspace omitted its output")?;
    let index = output
        .strip_prefix("HEADLESS-")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| (1..=2).contains(index))
        .ok_or("Recalled workspace is not on a supported Dock output")?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_options_preserve_mac_shortcuts_and_lossless_pixels() {
        let options = viewer_options();
        assert!(options.contains(&"-FullscreenSystemKeys=0"));
        assert!(options
            .windows(2)
            .any(|p| p == ["-ShortcutModifiers", "Ctrl,Shift"]));
        assert!(options
            .windows(2)
            .any(|p| p == ["-PreferredEncoding", "ZRLE"]));
        assert!(options.contains(&"-NoJPEG") && options.contains(&"-FullColor=1"));
        assert!(!options.iter().any(|p| p.contains("Password")));
        assert_eq!(endpoint(1).unwrap(), "127.0.0.1::15901");
        assert_eq!(endpoint(2).unwrap(), "127.0.0.1::15902");
        assert!(endpoint(3).is_err());
    }

    use super::*;
    #[test]
    fn shell_quote_preserves_prompt_metacharacters() {
        assert_eq!(quote("a'$(id); b"), "'a'\\''$(id); b'");
        assert!(allowed_action("desktop-web-url"));
        assert!(allowed_action("desktop-web-status"));
        assert!(!allowed_action("shell"));
    }
    #[test]
    fn recalled_workspace_selects_its_matching_viewer_output() {
        assert_eq!(
            recalled_output_index(&serde_json::json!({"active_workspace":{"output":"HEADLESS-2"}}))
                .unwrap(),
            2
        );
        for output in ["HEADLESS-0", "HEADLESS-3", "DP-1"] {
            assert!(recalled_output_index(
                &serde_json::json!({"active_workspace":{"output":output}})
            )
            .is_err());
        }
    }
    #[test]
    fn envelope_rejects_other_project_and_wrong_version() {
        assert!(envelope(
            "dock",
            true,
            serde_json::json!({"schema_version":1,"ok":true,"result":{"project":"studio"}})
        )
        .is_err());
        assert!(envelope(
            "dock",
            true,
            serde_json::json!({"schema_version":2,"ok":true,"result":{}})
        )
        .is_err());
    }
}
