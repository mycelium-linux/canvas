//! Transport-independent presentation endpoint. Transport authenticates callers;
//! this state machine owns admission, revisions and the target's own catalog.
use crate::{WorkspaceDocument, WorkspacePatch, session::Workspaces};
use canvas_core::{Actor, Catalog};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    pub command: Command,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Inspect,
    ReadDocument,
    Install {
        catalog: Catalog,
        documents: Vec<WorkspaceDocument>,
        selected: String,
    },
    WorkspaceSwitch {
        name: String,
    },
    WorkspaceRead {
        name: String,
    },
    ApplyPatch {
        patch: WorkspacePatch,
    },
}

#[derive(Clone)]
pub struct Endpoint {
    owner: String,
    revision: u64,
    session: Option<Workspaces>,
}

impl Endpoint {
    pub fn new(owner: &str) -> Self {
        Self {
            owner: owner.into(),
            revision: 1,
            session: None,
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "owner":self.owner, "revision":self.revision,
            "catalog":self.session.as_ref().map(Workspaces::selector_projection),
            "palette":self.session.as_ref().and_then(|session| session.palette().ok().flatten()),
            "document":self.session.as_ref().and_then(Workspaces::document)
        })
    }

    pub fn checkpoint(&self) -> serde_json::Value {
        serde_json::json!({"schema_version":1,"owner":self.owner,"revision":self.revision,
            "projection":self.session.as_ref().map(Workspaces::retained_projection)})
    }

    pub fn restore(owner: &str, raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_FRAME {
            return Err("Checkpoint exceeds limit".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Projection {
            catalog: Catalog,
            documents: Vec<WorkspaceDocument>,
            selected: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Checkpoint {
            schema_version: u32,
            owner: String,
            revision: u64,
            projection: Option<Projection>,
        }
        let checkpoint: Checkpoint = serde_json::from_str(raw).map_err(|_| "Invalid checkpoint")?;
        if checkpoint.schema_version != 1 || checkpoint.owner != owner || checkpoint.revision == 0 {
            return Err("Invalid checkpoint provenance/version".into());
        }
        let mut endpoint = Self::new(owner);
        if let Some(projection) = checkpoint.projection {
            if checkpoint.revision < 2 {
                return Err("Invalid checkpoint revision".into());
            }
            endpoint.execute(Request {
                schema_version: 1,
                request_id: "restore".into(),
                expected_revision: Some(1),
                command: Command::Install {
                    catalog: projection.catalog,
                    documents: projection.documents,
                    selected: projection.selected,
                },
            })?;
        }
        endpoint.revision = checkpoint.revision;
        Ok(endpoint)
    }

    pub fn execute(&mut self, request: Request) -> Result<serde_json::Value, String> {
        if request.schema_version != 1
            || request.request_id.is_empty()
            || request.request_id.len() > 64
        {
            return Err("Invalid presentation request".into());
        }
        let writes = !matches!(
            request.command,
            Command::Inspect | Command::ReadDocument | Command::WorkspaceRead { .. }
        );
        if writes && request.expected_revision != Some(self.revision) {
            return Err("Stale or missing endpoint revision; inspect this target first".into());
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or("Endpoint revision exhausted")?;
        match request.command {
            Command::Inspect => {}
            Command::ReadDocument => {
                return serde_json::to_value(self.session.as_ref().and_then(Workspaces::document))
                    .map_err(|error| error.to_string());
            }
            Command::WorkspaceRead { name } => {
                let session = self.session.as_ref().ok_or("Endpoint has no catalog")?;
                return serde_json::to_value(session.document_for(&name))
                    .map_err(|error| error.to_string());
            }
            Command::Install {
                catalog,
                documents,
                selected,
            } => {
                if documents.len() > 64 {
                    return Err("Too many documents".into());
                }
                let mut next = Workspaces::from_catalog(
                    &serde_json::to_string(&catalog).map_err(|error| error.to_string())?,
                    &selected,
                )?;
                let mut ids = std::collections::BTreeSet::new();
                for document in documents {
                    if !ids.insert(document.project_id.clone()) {
                        return Err("Duplicate project document".into());
                    }
                    next.install_document(
                        &serde_json::to_string(&document).map_err(|error| error.to_string())?,
                    )?;
                }
                if next.document().is_none() {
                    return Err("Selected project has no document".into());
                }
                self.session = Some(next);
            }
            Command::WorkspaceSwitch { name } => {
                let session = self.session.as_mut().ok_or("Endpoint has no catalog")?;
                if session.document_for(&name).is_none() {
                    return Err("Project has no retained document".into());
                }
                session.select(Actor::Human, &name)?;
            }
            Command::ApplyPatch { patch } => {
                self.session
                    .as_mut()
                    .ok_or("Endpoint has no catalog")?
                    .apply_document_patch(&patch)?;
            }
        }
        if writes {
            self.revision = next_revision;
        }
        Ok(self.snapshot())
    }

    pub fn dispatch(&mut self, raw: &str) -> String {
        let result = if raw.len() > MAX_FRAME {
            Err("Presentation request exceeds limit".into())
        } else {
            serde_json::from_str::<Request>(raw)
                .map_err(|_| "Invalid presentation command".into())
                .and_then(|request| self.execute(request))
        };
        match result {
            Ok(data) => {
                serde_json::json!({"ok":true,"revision":self.revision,"data":data}).to_string()
            }
            Err(error) => {
                serde_json::json!({"ok":false,"revision":self.revision,"error":error}).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_tv_install_switch_patch_and_restart_use_one_session() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let raw = include_str!("../../../config/deploy/firetv/workspaces.json");
        let request: Request = serde_json::from_str(raw).unwrap();
        endpoint.execute(request).unwrap();
        assert_eq!(endpoint.snapshot()["catalog"]["selected_project"], "television");
        let before = endpoint.checkpoint();
        let stale = Request { schema_version:1,request_id:"stale".into(),expected_revision:Some(1),
            command:Command::WorkspaceSwitch { name:"clock".into() } };
        assert!(endpoint.execute(stale).is_err());
        assert_eq!(endpoint.checkpoint(), before);
        endpoint.execute(Request { schema_version:1,request_id:"switch".into(),expected_revision:Some(2),
            command:Command::WorkspaceSwitch { name:"clock".into() } }).unwrap();
        let saved = endpoint.checkpoint().to_string();
        let restored = Endpoint::restore("firetv-omni", &saved).unwrap();
        assert_eq!(restored.snapshot(), endpoint.snapshot());
        assert_eq!(restored.snapshot()["catalog"]["selected_project"], "clock");
        assert!(Endpoint::restore("neo", &saved).is_err());
        assert!(Endpoint::restore("firetv-omni", "{}").is_err());
    }
    #[test]
    fn stale_mutations_and_unsupported_commands_leave_target_unchanged() {
        let mut endpoint = Endpoint::new("tv");
        assert!(
            endpoint
                .execute(Request {
                    schema_version: 1,
                    request_id: "test".into(),
                    expected_revision: None,
                    command: Command::WorkspaceSwitch {
                        name: "studio".into()
                    }
                })
                .is_err()
        );
        assert_eq!(endpoint.snapshot()["revision"], 1);
        assert!(
            !serde_json::from_str::<serde_json::Value>(&endpoint.dispatch(
                r#"{"schema_version":1,"request_id":"test","command":{"command":"shutdown"}}"#
            ))
            .unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(endpoint.snapshot()["owner"], "tv");
    }
}
