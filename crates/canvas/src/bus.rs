//! Shared unibus client for the Canvas edge and central binaries: connect,
//! register, heartbeat, and frame I/O. Both sides speak one envelope; only
//! the role differs (`canvas-edge` consumes and executes, `canvas-central`
//! addresses and awaits receipts).

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use canvas_core::unibus::{Envelope, FrameError, Message, framing, kinds};
use serde::Deserialize;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusConfig {
    pub installation_id: String,
    pub producer_id: String,
    pub hub_endpoint: String,
    pub token_file: PathBuf,
    pub zones: Vec<String>,
    /// Capabilities to claim at registration. Defaults to the v1 tag
    /// `display` plus the namespaced `canvas.display`; point-to-point
    /// senders like the central may claim a namespaced tag of their own.
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub screen_anchors: Vec<String>,
    /// Wire schema presented to the hub. Deployed v1 hubs reject schema 2,
    /// so the default is v1; set 2 once the hub runs the unibus build.
    #[serde(default = "default_schema")]
    pub schema_version: u32,
}

fn default_schema() -> u32 {
    canvas_core::unibus::SCHEMA_VERSION_V1
}

impl BusConfig {
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read bus config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes).with_context(|| "parse bus config")?;
        ensure!(!config.zones.is_empty(), "bus config must serve a zone");
        Ok(config)
    }

    pub fn token(&self) -> Result<String> {
        Ok(std::fs::read_to_string(&self.token_file)
            .with_context(|| format!("read hub token {}", self.token_file.display()))?
            .trim()
            .to_owned())
    }
}

/// A registered bus connection: one reader and one mutex-guarded writer
/// shared by every sender thread (heartbeats and command sends can never
/// interleave partial frames).
pub struct Bus {
    config: BusConfig,
    reader: Mutex<TcpStream>,
    writer: Arc<Mutex<TcpStream>>,
    stop: Arc<AtomicBool>,
}

impl Bus {
    /// Connect, register, and start the heartbeat.
    pub fn connect(config: BusConfig) -> Result<Self> {
        let addr: std::net::SocketAddr = config
            .hub_endpoint
            .parse()
            .context("hub_endpoint must be ip:port")?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .with_context(|| format!("connect hub at {addr}"))?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let token = config.token()?;

        let claimed: Vec<String> = if config.capabilities.is_empty() {
            vec!["display".into(), "canvas.display".into()]
        } else {
            config.capabilities.clone()
        };
        let mut register = Envelope::new(
            &config.installation_id,
            &config.producer_id,
            None,
            &token,
            Message::new(
                kinds::V1_REGISTER,
                serde_json::json!({
                    "capabilities": claimed,
                    "zones": config.zones,
                    "screen_anchors": config.screen_anchors,
                }),
            ),
        );
        register.schema_version = config.schema_version;

        let writer = Arc::new(Mutex::new(
            stream.try_clone().context("clone stream for writer")?,
        ));
        framing::write_frame(&mut *writer.lock().unwrap(), &register, &[])
            .context("send register")?;

        let mut reader = stream.try_clone().context("clone stream for reader")?;
        let (reply, _) = framing::read_frame(&mut reader).context("read register reply")?;
        ensure!(
            reply.message.body["success"].as_bool() == Some(true),
            "hub rejected registration: {:?}",
            reply.message.body
        );

        let stop = Arc::new(AtomicBool::new(false));
        {
            let installation = config.installation_id.clone();
            let producer = config.producer_id.clone();
            let schema = config.schema_version;
            let writer = writer.clone();
            let stop = stop.clone();
            thread::Builder::new()
                .name("canvas-bus-heartbeat".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        thread::sleep(HEARTBEAT_INTERVAL);
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let mut beat = Envelope::new(
                            &installation,
                            &producer,
                            None,
                            &token,
                            Message::new(kinds::V1_HEARTBEAT, serde_json::Value::Null),
                        );
                        beat.schema_version = schema;
                        let Ok(mut guard) = writer.lock() else {
                            stop.store(true, Ordering::Release);
                            break;
                        };
                        if framing::write_frame(&mut *guard, &beat, &[]).is_err() {
                            stop.store(true, Ordering::Release);
                            break;
                        }
                    }
                })
                .context("start heartbeat thread")?;
        }

        Ok(Self {
            config,
            reader: Mutex::new(reader),
            writer,
            stop,
        })
    }

    pub fn config(&self) -> &BusConfig {
        &self.config
    }

    pub fn send(&self, envelope: &Envelope, payload: &[u8]) -> Result<()> {
        let mut envelope = envelope.clone();
        envelope.schema_version = self.config.schema_version;
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("bus writer poisoned"))?;
        framing::write_frame(&mut *guard, &envelope, payload).context("write frame to hub")
    }

    /// Envelope addressed at one screen anchor.
    pub fn for_screen(&self, anchor: &str, message: Message) -> Result<Envelope> {
        let mut envelope = Envelope::for_screen(
            &self.config.installation_id,
            &self.config.producer_id,
            &self.config.token()?,
            anchor,
            message,
        );
        envelope.schema_version = self.config.schema_version;
        Ok(envelope)
    }

    /// Envelope addressed to a zone (destinations name zones for routed
    /// kinds and producers for reply-routed kinds).
    pub fn to_destination(&self, destination: &str, message: Message) -> Result<Envelope> {
        let mut envelope = Envelope::new(
            &self.config.installation_id,
            &self.config.producer_id,
            Some(destination),
            &self.config.token()?,
            message,
        );
        envelope.schema_version = self.config.schema_version;
        Ok(envelope)
    }

    /// Blocking read of one frame; read-timeout expiry is an idle poll.
    pub fn try_read(&self) -> Result<Option<(Envelope, Vec<u8>)>> {
        ensure!(
            !self.stop.load(Ordering::Acquire),
            "bus heartbeat writer stopped"
        );
        let mut guard = self
            .reader
            .lock()
            .map_err(|_| anyhow::anyhow!("bus reader poisoned"))?;
        match framing::read_frame(&mut *guard) {
            Ok(frame) => Ok(Some(frame)),
            Err(FrameError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn bus_config_defaults_and_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.json");
        std::fs::write(
            &path,
            r#"{"installation_id":"home","producer_id":"c","hub_endpoint":"127.0.0.1:1","token_file":"t","zones":["default"]}"#,
        )
        .unwrap();
        let config = BusConfig::read(&path).unwrap();
        assert_eq!(
            config.schema_version,
            canvas_core::unibus::SCHEMA_VERSION_V1
        );
        assert!(config.capabilities.is_empty());

        std::fs::write(
            &path,
            r#"{"installation_id":"home","producer_id":"c","hub_endpoint":"127.0.0.1:1","token_file":"t","zones":[],"hub_admin":true}"#,
        )
        .unwrap();
        assert!(BusConfig::read(&path).is_err());
    }

    #[test]
    fn stopped_heartbeat_makes_the_reader_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        let bus = Bus {
            config: BusConfig {
                installation_id: "home".into(),
                producer_id: "canvas-edge".into(),
                hub_endpoint: listener.local_addr().unwrap().to_string(),
                token_file: PathBuf::from("unused"),
                zones: vec!["default".into()],
                capabilities: vec!["canvas.display".into()],
                screen_anchors: vec!["screen-test".into()],
                schema_version: canvas_core::unibus::SCHEMA_VERSION_V2,
            },
            reader: Mutex::new(client.try_clone().unwrap()),
            writer: Arc::new(Mutex::new(client)),
            stop: Arc::new(AtomicBool::new(true)),
        };

        assert_eq!(
            bus.try_read().unwrap_err().to_string(),
            "bus heartbeat writer stopped"
        );
    }
}
