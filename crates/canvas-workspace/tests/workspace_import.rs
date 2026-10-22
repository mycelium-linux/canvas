use canvas_workspace::{
    WorkspaceDocument,
    endpoint::{Command, Endpoint, Request},
};

#[test]
fn incremental_import_is_revision_bound_retained_and_preserves_menu() {
    let mut endpoint = Endpoint::new("firetv-omni");
    let install: Request = serde_json::from_str(include_str!(
        "../../../config/deploy/firetv/workspaces.json"
    ))
    .unwrap();
    endpoint.execute(install).unwrap();
    let before = endpoint.snapshot();
    let document: WorkspaceDocument = serde_json::from_str(include_str!(
        "../../../config/workspaces/shared/canvas-sync.json"
    ))
    .unwrap();
    let import = |revision| Request {
        schema_version: 1,
        request_id: "import-live".into(),
        expected_revision: revision,
        command: Command::WorkspaceImport {
            document: Box::new(document.clone()),
            activate: false,
        },
    };
    assert!(endpoint.execute(import(None)).is_err());
    assert_eq!(endpoint.snapshot(), before);
    let revision = endpoint.snapshot()["revision"].as_u64().unwrap();
    endpoint.execute(import(Some(revision))).unwrap();
    assert_eq!(
        endpoint.snapshot()["catalog"]["selected_project"],
        before["catalog"]["selected_project"]
    );
    assert_eq!(
        endpoint.snapshot()["catalog"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        before["catalog"]["projects"].as_array().unwrap().len() + 1
    );
    let mut restored =
        Endpoint::restore("firetv-omni", &endpoint.checkpoint().to_string()).unwrap();
    let read = Request {
        schema_version: 1,
        request_id: "read-live".into(),
        expected_revision: None,
        command: Command::WorkspaceRead {
            name: "canvas-sync".into(),
        },
    };
    assert_eq!(
        restored.execute(read).unwrap(),
        serde_json::to_value(&document).unwrap()
    );
    let saved = endpoint.checkpoint();
    assert!(endpoint.execute(import(Some(revision))).is_err());
    assert_eq!(endpoint.checkpoint(), saved);
}
