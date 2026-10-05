//! Host-owned asynchronous transport for bounded Lua module intents.
//! A widget VM never receives credentials or an unrestricted bus session.
use crate::lua_module::ModuleAction;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use unibus_sdk::{Event, Publish, Session};

#[derive(Default)]
pub struct ModuleBus {
    connecting: Option<Receiver<Result<Session, String>>>,
    session: Option<Session>,
    pub error: Option<String>,
    dropped: u64,
    disconnected_reported: bool,
    pending: BTreeMap<String, (String, Instant)>,
}

impl ModuleBus {
    /// Attach a session chosen by the trusted host, not by a widget script.
    pub fn from_session(session: Session) -> Self {
        Self {
            session: Some(session),
            ..Self::default()
        }
    }
    /// Explicit host configuration, independent from Canvas-edge's identity.
    /// No connection is attempted from a frame or Lua invocation.
    pub fn from_env() -> Self {
        let Some(path) = std::env::var_os("CANVAS_MODULE_BUS_CONFIG") else {
            return Self::default();
        };
        let (sender, receiver) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("canvas-module-connect".into())
            .spawn(move || {
                let result = (|| -> anyhow::Result<Session> {
                    anyhow::ensure!(
                        std::fs::metadata(&path)?.len() <= 64 * 1024,
                        "module bus config exceeds 64 KiB"
                    );
                    let config = serde_json::from_slice(&std::fs::read(path)?)?;
                    Session::connect(config)
                })()
                .map_err(|error| error.to_string());
                let _ = sender.send(result);
            });
        match spawn {
            Ok(_) => Self {
                connecting: Some(receiver),
                ..Self::default()
            },
            Err(error) => Self {
                error: Some(error.to_string()),
                ..Self::default()
            },
        }
    }
    pub fn connected(&self) -> bool {
        self.dropped == 0
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.status().connected)
    }
    pub fn publish(&mut self, action: ModuleAction) -> Result<String, String> {
        if !self.connected() {
            return Err(
                "module bus is disconnected or degraded; resynchronization required".into(),
            );
        }
        if self.pending.len() >= 64 {
            return Err("too many module commands awaiting receipts".into());
        }
        let destination = action.destination.clone();
        let request_id = self
            .session
            .as_ref()
            .ok_or("module bus is not connected")?
            .publish(Publish {
                destination: Some(action.destination),
                kind: action.kind,
                body: action.body,
            })
            .map_err(|error| error.to_string())?;
        self.pending
            .insert(request_id.clone(), (destination, Instant::now()));
        Ok(request_id)
    }
    /// Only the addressed controller can acknowledge a pending request.
    pub fn receipt(&mut self, event: &Event) -> Result<String, String> {
        if event.kind != "canvas.module.action.result.v1" {
            return Err("unexpected module receipt kind".into());
        }
        let id = event
            .body
            .get("request_id")
            .and_then(|value| value.as_str())
            .ok_or("receipt lacks request_id")?;
        let (destination, _) = self
            .pending
            .get(id)
            .ok_or("receipt does not match a pending module command")?;
        if destination != &event.producer_id {
            return Err("module receipt came from the wrong controller".into());
        }
        let ok = event
            .body
            .get("ok")
            .and_then(|value| value.as_bool())
            .ok_or("receipt lacks ok")?;
        self.pending.remove(id);
        if ok {
            Ok(format!("Controller acknowledged module command · {id}"))
        } else {
            Err(format!("Controller rejected module command · {id}"))
        }
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let before = self.pending.len();
        self.pending
            .retain(|_, (_, queued)| queued.elapsed() < Duration::from_secs(10));
        if self.pending.len() < before {
            self.error =
                Some("module command receipt timed out; outcome unknown, not replayed".into());
        }
        if let Some(receiver) = &self.connecting {
            match receiver.try_recv() {
                Ok(Ok(session)) => {
                    self.session = Some(session);
                    self.connecting = None;
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.connecting = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("module bus connector stopped".into());
                    self.connecting = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let status = session.status();
        if !status.connected && !self.disconnected_reported {
            self.error = Some("module bus disconnected; commands are not replayed".into());
            self.disconnected_reported = true;
        }
        if status.dropped_events > self.dropped {
            self.error = Some(format!(
                "module bus dropped {} events; state needs resynchronization",
                status.dropped_events - self.dropped
            ));
            self.dropped = status.dropped_events;
        }
        (0..16).map_while(|_| session.poll()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt(producer: &str) -> Event {
        Event {
            producer_id: producer.into(),
            request_id: "reply-1".into(),
            issued_at_unix_ms: 1,
            kind: "canvas.module.action.result.v1".into(),
            body: serde_json::json!({"request_id":"command-1","ok":true}),
        }
    }
    #[test]
    fn only_correlated_controller_receipts_are_accepted() {
        let mut bus = ModuleBus::default();
        bus.pending
            .insert("command-1".into(), ("controller".into(), Instant::now()));
        assert!(bus.receipt(&receipt("other-controller")).is_err());
        assert_eq!(bus.pending.len(), 1);
        assert!(bus.receipt(&receipt("controller")).is_ok());
        assert!(bus.receipt(&receipt("controller")).is_err());
    }
    #[test]
    fn missing_receipts_expire_without_replay() {
        let mut bus = ModuleBus::default();
        bus.pending.insert(
            "command-1".into(),
            (
                "controller".into(),
                Instant::now() - Duration::from_secs(11),
            ),
        );
        assert!(bus.poll().is_empty());
        assert!(bus.pending.is_empty());
        assert!(bus.error.as_ref().unwrap().contains("outcome unknown"));
    }
}
