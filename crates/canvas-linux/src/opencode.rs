//! OpenCode owns inference and conversation persistence. Dock calls its supported API.
use crate::{command, unit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Serialize, Deserialize)]
struct Session {
    version: u32,
    project: String,
    directory: String,
    port: u16,
    session_id: String,
}
fn root() -> Result<PathBuf, String> {
    Ok(PathBuf::from(env::var("HOME").map_err(|_| "HOME missing")?)
        .join(".local/state/dock/opencode"))
}
fn state_path(project: &str) -> Result<PathBuf, String> {
    unit(project)?;
    Ok(root()?.join(format!("{project}.json")))
}
fn service(project: &str) -> String {
    format!("dock-opencode-{project}.service")
}
fn tmux(project: &str) -> String {
    format!("dock-{project}")
}
fn binary() -> Result<PathBuf, String> {
    let bin =
        PathBuf::from(env::var("HOME").map_err(|_| "HOME missing")?).join(".opencode/bin/opencode");
    if !bin.is_file() {
        return Err("OpenCode missing; install official CLI into ~/.opencode/bin/opencode".into());
    }
    Ok(bin)
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}
fn save(session: &Session) -> Result<(), String> {
    private_write(
        &state_path(&session.project)?,
        &serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?,
    )
}
fn load(project: &str) -> Result<Session, String> {
    let s: Session = serde_json::from_slice(&fs::read(state_path(project)?).map_err(|e| {
        format!("No registered OpenCode session for {project}: {e}; run session-ensure")
    })?)
    .map_err(|e| e.to_string())?;
    if s.version != 1 || s.project != project || s.port == 0 {
        return Err("invalid OpenCode session registration".into());
    }
    Ok(s)
}
fn password(project: &str) -> Result<String, String> {
    let p = root()?.join(format!("{project}.env"));
    let text = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let pass = text
        .strip_prefix("OPENCODE_SERVER_PASSWORD=")
        .ok_or("invalid server auth file")?
        .trim();
    if pass.len() != 64 || !pass.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid server auth file".into());
    }
    Ok(pass.into())
}
fn api(
    s: &Session,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client
        .request(method, format!("http://127.0.0.1:{}{path}", s.port))
        .basic_auth("opencode", Some(password(&s.project)?))
        .query(&[("directory", &s.directory)]);
    if let Some(body) = body {
        request = request.json(&body)
    }
    let response = request
        .send()
        .map_err(|e| format!("OpenCode request failed: {e}"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("OpenCode response exceeds 4 MiB".into());
    }
    if !status.is_success() {
        return Err(format!(
            "OpenCode API {path}: HTTP {status}: {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    if bytes.is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_slice(&bytes).map_err(|e| format!("OpenCode JSON: {e}"))
    }
}
fn owned_service(project: &str) -> Result<(), String> {
    let name = service(project);
    let description = command(
        "systemctl",
        &["--user", "show", &name, "--property=Description", "--value"],
    )?;
    if description != "Dock OpenCode backend" {
        return Err(format!("{name} lacks Dock ownership marker"));
    }
    command("systemctl", &["--user", "is-active", &name])?;
    Ok(())
}
fn require_session(s: &Session) -> Result<(), String> {
    if s.session_id.is_empty()
        || !s
            .session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid or uninitialized OpenCode session id".into());
    }
    owned_service(&s.project)
}
fn ensure(project: &str, directory: &Path, port: Option<u16>) -> Result<Value, String> {
    use std::os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    };
    unit(project)?;
    let dir = directory
        .canonicalize()
        .map_err(|e| format!("project directory: {e}"))?;
    if !dir.is_dir() {
        return Err("project directory is not a directory".into());
    }
    let root = root()?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)
        .map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("OpenCode state directory must be owned by current user with mode0700".into());
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(format!("{project}.lock")))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("cannot lock session state".into());
    }
    let path = state_path(project)?;
    let mut s = if path.exists() {
        let existing = load(project)?;
        if Path::new(&existing.directory) != dir || port.is_some_and(|p| p != existing.port) {
            return Err(
                "project session already registered with different directory or port".into(),
            );
        }
        existing
    } else {
        let mut random = [0u8; 32];
        fs::File::open("/dev/urandom")
            .map_err(|e| e.to_string())?
            .read_exact(&mut random)
            .map_err(|e| e.to_string())?;
        let secret: String = random.iter().map(|b| format!("{b:02x}")).collect();
        private_write(
            &root.join(format!("{project}.env")),
            format!("OPENCODE_SERVER_PASSWORD={secret}\n").as_bytes(),
        )?;
        let port = match port {
            Some(0) => return Err("port must be nonzero".into()),
            Some(p) => p,
            None => TcpListener::bind("127.0.0.1:0")
                .map_err(|e| e.to_string())?
                .local_addr()
                .map_err(|e| e.to_string())?
                .port(),
        };
        let s = Session {
            version: 1,
            project: project.into(),
            directory: dir.to_str().ok_or("directory must be UTF8")?.into(),
            port,
            session_id: String::new(),
        };
        save(&s)?;
        s
    };
    if command("systemctl", &["--user", "is-active", &service(project)]).is_err() {
        let bin = binary()?;
        let envfile = format!(
            "--property=EnvironmentFile={}",
            root.join(format!("{project}.env")).display()
        );
        command(
            "systemd-run",
            &[
                "--user",
                "--collect",
                "--service-type=exec",
                "--description=Dock OpenCode backend",
                "--property=KillMode=control-group",
                "--property=Restart=on-failure",
                &envfile,
                "--unit",
                &service(project),
                "--working-directory",
                &s.directory,
                "--",
                bin.to_str().ok_or("binary path invalid")?,
                "serve",
                "--hostname",
                "127.0.0.1",
                "--port",
                &s.port.to_string(),
            ],
        )?;
    }
    owned_service(project)?;
    let mut healthy = false;
    for _ in 0..30 {
        if api(&s, reqwest::Method::GET, "/global/health", None).is_ok() {
            healthy = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200))
    }
    if !healthy {
        return Err("OpenCode health check failed; inspect user service journal".into());
    }
    if s.session_id.is_empty() {
        let created = api(
            &s,
            reqwest::Method::POST,
            "/session",
            Some(json!({"title":format!("Dock {project}")})),
        )?;
        s.session_id = created["id"]
            .as_str()
            .ok_or("OpenCode did not return a session id")?
            .into();
        save(&s)?;
    }
    ensure_terminal(&s)?;
    Ok(
        json!({"project":project,"session_id":s.session_id,"directory":s.directory,"port":s.port,"tmux_session":tmux(project),"persistent":true}),
    )
}
fn ensure_terminal(s: &Session) -> Result<(), String> {
    let name = tmux(&s.project);
    if command("tmux", &["-L", "dock-opencode", "has-session", "-t", &name]).is_ok() {
        return Ok(());
    }
    // tmux receives arguments directly; no prompt or project data is interpreted by a shell.
    let status = std::process::Command::new("tmux")
        .args([
            "-L",
            "dock-opencode",
            "new-session",
            "-d",
            "-s",
            &name,
            "-c",
            &s.directory,
            "-x",
            "120",
            "-y",
            "36",
        ])
        .arg(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["session-tui", &s.project])
        .output()
        .map_err(|e| e.to_string())?;
    if !status.status.success() {
        return Err(format!(
            "tmux terminal failed: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    Ok(())
}
fn status(project: &str) -> Result<Value, String> {
    let s = load(project)?;
    require_session(&s)?;
    let session = api(
        &s,
        reqwest::Method::GET,
        &format!("/session/{}", s.session_id),
        None,
    )?;
    let states = api(&s, reqwest::Method::GET, "/session/status", None)?;
    let messages = api(
        &s,
        reqwest::Method::GET,
        &format!("/session/{}/message?limit=20", s.session_id),
        None,
    )?;
    let status = states
        .get(&s.session_id)
        .cloned()
        .unwrap_or(json!({"type":"idle"}));
    Ok(
        json!({"project":project,"session_id":s.session_id,"directory":s.directory,"port":s.port,"session":session,"status":status,"messages":messages,"providers":{"note":"Model/provider availability is managed by OpenCode; use its TUI model selector"},"terminal":{"tmux_session":tmux(project),"alive":command("tmux", &["-L","dock-opencode","has-session","-t",&tmux(project)]).is_ok()}}),
    )
}
pub fn abort(project: &str) -> Result<Value, String> {
    if !state_path(project)?.exists() {
        return Ok(json!({"project":project,"aborted":false,"reason":"no registered session"}));
    }
    let s = load(project)?;
    require_session(&s)?;
    let result = api(
        &s,
        reqwest::Method::POST,
        &format!("/session/{}/abort", s.session_id),
        Some(json!({})),
    )?;
    if result != true {
        return Err(format!("OpenCode abort unconfirmed: {result}"));
    }
    Ok(json!({"project":project,"session_id":s.session_id,"aborted":true}))
}
fn prompt_body(text: &str, model: Option<&str>) -> Result<Value, String> {
    if text.trim().is_empty() || text.len() > 65536 {
        return Err("prompt must contain 1–65536 bytes".into());
    }
    let mut body = json!({"parts":[{"type":"text","text":text}]});
    if let Some(model) = model {
        let (p, m) = model
            .split_once('/')
            .ok_or("model must be provider/model")?;
        if p.is_empty() || m.is_empty() {
            return Err("model must be provider/model".into());
        }
        body["model"] = json!({"providerID":p,"modelID":m})
    }
    Ok(body)
}
fn prompt(project: &str, text: &str, model: Option<&str>) -> Result<Value, String> {
    let body = prompt_body(text, model)?;
    let s = load(project)?;
    require_session(&s)?;
    crate::control::with_agent(project, || {
        api(
            &s,
            reqwest::Method::POST,
            &format!("/session/{}/prompt_async", s.session_id),
            Some(body),
        )?;
        Ok(
            json!({"project":project,"session_id":s.session_id,"accepted":true,"note":"poll session-status for inference result or provider errors"}),
        )
    })
}
pub fn directory(project: &str) -> Result<PathBuf, String> {
    unit(project)?;
    let home = PathBuf::from(env::var("HOME").map_err(|_| "HOME missing")?);
    if state_path(project)?.exists() {
        return Ok(PathBuf::from(load(project)?.directory));
    }
    let config = home.join(".config/dock/project-directories.json");
    if config.exists() {
        let mapping: std::collections::HashMap<String, String> =
            serde_json::from_slice(&fs::read(config).map_err(|e| e.to_string())?)
                .map_err(|e| format!("project directory configuration: {e}"))?;
        if let Some(path) = mapping.get(project) {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("configured project directory must be absolute".into());
            }
            return Ok(path);
        }
    }
    match project {
        "dock" => Ok(home.join("src/dock")),
        "studio" => Ok(home.join("src/dock-studio")),
        _ => {
            let path = state_path(project)?;
            if path.exists() {
                Ok(PathBuf::from(load(project)?.directory))
            } else {
                Err(
                    "project directory not registered; use session-start PROJECT DIRECTORY first"
                        .into(),
                )
            }
        }
    }
}
pub fn execute(args: &[String]) -> Result<Value, String> {
    let action = args.first().map(String::as_str).unwrap_or("");
    let project = args.get(1).ok_or("session action requires PROJECT")?;
    unit(project)?;
    match action{
 "session-ensure" if args.len()==2=>ensure(project,&directory(project)?,None),
 "session-start" if (3..=4).contains(&args.len())=>ensure(project,Path::new(&args[2]),args.get(3).map(|p|p.parse::<u16>().map_err(|_|"invalid port".to_string())).transpose()?),
 "session-status" if args.len()==2=>status(project),
 "session-abort" if args.len()==2=>abort(project),
 "session-prompt"|"session-resume" if (2..=3).contains(&args.len())=>{let mut text=String::new();std::io::stdin().take(65537).read_to_string(&mut text).map_err(|e|e.to_string())?;if action=="session-resume"&&text.trim().is_empty(){text="Continue the interrupted task from the existing conversation; inspect current state before acting.".into()}prompt(project,&text,args.get(2).map(String::as_str))},
 "session-tui" if args.len()==2=>{let s=load(project)?;require_session(&s)?;let exit=std::process::Command::new(binary()?).env("OPENCODE_SERVER_PASSWORD",password(project)?).args(["attach",&format!("http://127.0.0.1:{}",s.port),"--dir",&s.directory,"--session",&s.session_id]).status().map_err(|e|e.to_string())?;if !exit.success(){return Err(format!("OpenCode terminal exited {exit}"))}Ok(json!({"project":project,"terminal_closed":true}))},
 "session-attach" if args.len()==2=>{let s=load(project)?;require_session(&s)?;ensure_terminal(&s)?;crate::desktop::open_session(project,&tmux(project),&s.directory)},
 _=>Err("usage: session-ensure PROJECT | session-start PROJECT DIRECTORY [PORT] | session-status/session-abort/session-attach PROJECT | session-prompt/session-resume PROJECT [PROVIDER/MODEL] (prompt on stdin)".into())}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_project_registration() {
        for p in ["../foo", "a/b", "a;id", ""] {
            assert!(state_path(p).is_err())
        }
    }
    #[test]
    fn prompt_is_bounded_and_payload_is_structured() {
        assert!(prompt_body("  ", None).is_err());
        assert!(prompt_body(&"x".repeat(65537), None).is_err());
        for model in ["missing-slash", "/model", "provider/"] {
            assert!(prompt_body("hi", Some(model)).is_err());
        }
        let text = "quote \" and newline\n$(id)";
        let body = prompt_body(text, Some("opencode/big-pickle")).unwrap();
        assert_eq!(body["parts"][0]["text"], text);
        assert_eq!(body["model"]["providerID"], "opencode");
    }
    #[test]
    fn service_namespace() {
        assert_eq!(service("dock"), "dock-opencode-dock.service");
        assert_eq!(tmux("studio"), "dock-studio")
    }
}
