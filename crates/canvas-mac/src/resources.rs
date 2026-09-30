//! A live resource index owned by Alfalfa; no agent/session prerequisites.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub company_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub project: String,
    pub project_name: String,
    pub id: String,
    pub title: String,
    pub kind: String,
    pub uri: String,
    pub note: Option<String>,
    pub home: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub purpose: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Index {
    pub companies: Vec<Company>,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Company {
    pub id: String,
    pub name: String,
    pub role: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub company_id: Option<String>,
    pub name: String,
    pub projects: usize,
}

fn companies(value: Value) -> Result<Vec<Company>, String> {
    let companies: Vec<Company> =
        serde_json::from_value(value).map_err(|_| "Invalid owner company memberships")?;
    let mut ids = std::collections::HashSet::new();
    if companies
        .iter()
        .any(|c| c.id.is_empty() || c.name.trim().is_empty() || !ids.insert(c.id.clone()))
    {
        return Err("Owner returned ambiguous company memberships".into());
    }
    Ok(companies)
}
/// Folder identity always comes from the canonical company ID. Equal names do
/// not merge companies; an absent company is the separate Unassigned folder.
pub fn folders(index: &Index, query: &str, include_inactive: bool) -> Vec<Folder> {
    let mut result = Vec::new();
    for (id, name) in index
        .companies
        .iter()
        .map(|c| (Some(c.id.clone()), c.name.clone()))
        .chain(std::iter::once((None, "Unassigned".into())))
    {
        let projects = project_matches(index, "", None, include_inactive)
            .into_iter()
            .filter(|i| index.entries[*i].company_id == id)
            .count();
        let searchable = matching(index, query).into_iter().any(|i| {
            index.entries[i].company_id == id
                && (include_inactive || !inactive(&index.entries[i].status))
        });
        let name_matches = query
            .split_whitespace()
            .all(|word| name.to_lowercase().contains(&word.to_lowercase()));
        if projects > 0 && (name_matches || searchable) {
            result.push(Folder {
                company_id: id,
                name,
                projects,
            });
        }
    }
    result.sort_by(|a, b| {
        a.company_id
            .is_none()
            .cmp(&b.company_id.is_none())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.company_id.cmp(&b.company_id))
    });
    result
}
fn inactive(status: &str) -> bool {
    matches!(
        status,
        "paused" | "archived" | "retired" | "deprecated" | "end_of_life"
    )
}
pub fn folder_matches(
    index: &Index,
    query: &str,
    company_id: Option<&str>,
    project: Option<&str>,
    include_inactive: bool,
) -> Vec<usize> {
    if company_id.is_some_and(|id| !index.companies.iter().any(|c| c.id == id)) {
        return vec![];
    }
    project_matches(index, query, project, include_inactive)
        .into_iter()
        .filter(|i| index.entries[*i].company_id.as_deref() == company_id)
        .collect()
}

fn project_home(id: &str) -> Result<String, String> {
    if !super::session_identifier(id) {
        return Err("Invalid project identity".into());
    }
    let port = std::env::var("DOCK_ALFALFA_PORT")
        .unwrap_or_else(|_| "8791".into())
        .parse::<u16>()
        .map_err(|_| "Invalid Alfalfa port")?;
    if port == 0 {
        return Err("Invalid Alfalfa port".into());
    }
    Ok(format!("http://127.0.0.1:{port}/projects/{id}"))
}

fn entries(id: &str, context: &Value) -> Result<Vec<Entry>, String> {
    let project = &context["definition"]["project"];
    if project["id"].as_str() != Some(id) {
        return Err("Resource context belongs to another project".into());
    }
    let name = project["name"].as_str().ok_or("Project has no name")?;
    let home = project_home(id)?;
    let mut result = vec![Entry {
        company_id: project["company_id"].as_str().map(str::to_owned),
        workspace_id: context["definition"]["workspace_id"]
            .as_str()
            .map(str::to_owned),
        project: id.into(),
        project_name: name.into(),
        id: "@home".into(),
        title: format!("{name} project home"),
        kind: "project".into(),
        uri: home.clone(),
        note: None,
        home: home.clone(),
        status: project["status"].as_str().unwrap_or("active").into(),
        purpose: project["purpose"].as_str().unwrap_or("").into(),
    }];
    for resource in context["definition"]["resource"]
        .as_array()
        .ok_or("Missing project resources")?
    {
        let Some(resource_id) = resource["id"].as_str() else {
            continue;
        };
        let Some(title) = resource["title"].as_str() else {
            continue;
        };
        let note = resource["context_path"].as_str().and_then(|path| {
            context["sources"]
                .as_array()?
                .iter()
                .find(|source| source["path"] == path)?["content"]
                .as_str()
                .map(str::to_owned)
        });
        result.push(Entry {
            company_id: project["company_id"].as_str().map(str::to_owned),
            workspace_id: context["definition"]["workspace_id"]
                .as_str()
                .map(str::to_owned),
            project: id.into(),
            project_name: name.into(),
            id: resource_id.into(),
            title: title.into(),
            kind: resource["kind"].as_str().unwrap_or("resource").into(),
            uri: resource["uri"].as_str().unwrap_or("").into(),
            note,
            home: home.clone(),
            status: project["status"].as_str().unwrap_or("active").into(),
            purpose: String::new(),
        });
    }
    Ok(result)
}

/// Read one exact Alfalfa project through the credential owner and expose its
/// resources to Dock's presentation adapters. Callers decide where to present
/// them; this function never opens a target or returns unrelated projects.
pub fn project_entries(id: &str) -> Result<Vec<Entry>, String> {
    entries(id, &super::alfalfa::context(id)?)
}

pub fn load() -> Result<Index, String> {
    let projects = super::alfalfa::list()?;
    let projects = projects.as_array().ok_or("Invalid Alfalfa catalog")?;
    let mut index = Index::default();
    match super::alfalfa::companies().and_then(companies) {
        Ok(companies) => index.companies = companies,
        Err(error) if error == super::alfalfa::AUTHORIZATION_DENIED => return Err(error),
        Err(_) => index.warnings.push(
            "Company folders are unavailable; showing Unassigned workspaces. Command-R to retry."
                .into(),
        ),
    }
    // Bound owner requests and load four independent contexts concurrently.
    if projects.len() > 128 {
        index
            .warnings
            .push("Showing the first 128 projects.".into());
    }
    for batch in projects.iter().take(128).collect::<Vec<_>>().chunks(4) {
        std::thread::scope(|scope| {
            let reads: Vec<_> = batch
                .iter()
                .map(|project| {
                    scope.spawn(move || {
                        let id = project["project"]["id"]
                            .as_str()
                            .ok_or("Missing Alfalfa project identity")?;
                        entries(id, &super::alfalfa::context(id)?)
                    })
                })
                .collect();
            for read in reads {
                match read.join() {
                    Ok(Ok(entries)) => index.entries.extend(entries),
                    Ok(Err(error)) => index.warnings.push(error),
                    Err(_) => index
                        .warnings
                        .push("A project context could not be loaded.".into()),
                }
            }
        });
    }
    if index
        .warnings
        .iter()
        .any(|e| e == super::alfalfa::AUTHORIZATION_DENIED)
    {
        return Err(super::alfalfa::AUTHORIZATION_DENIED.into());
    }
    if index.entries.iter().any(|e| {
        e.company_id
            .as_ref()
            .is_some_and(|id| !index.companies.iter().any(|c| c.id == *id))
    }) {
        index
            .warnings
            .push("Some project companies are not in the current verified membership list.".into());
    }
    match super::alfalfa::activity() {
        Ok(activity) => add_observations(&mut index, &activity)?,
        Err(error) if error == super::alfalfa::AUTHORIZATION_DENIED => return Err(error),
        Err(_) => index
            .warnings
            .push("My agents is unavailable; showing registered projects.".into()),
    }
    Ok(index)
}

fn repository_key(uri: &str) -> Option<String> {
    let uri = if !uri.contains("://") && uri.contains('@') {
        format!("ssh://{}", uri.replacen(':', "/", 1))
    } else {
        uri.into()
    };
    let url = url::Url::parse(&uri).ok()?;
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    if url.scheme() == "file" {
        let path = url.to_file_path().ok()?.canonicalize().ok()?;
        let digest = format!("{:x}", Sha256::digest(path.to_str()?.as_bytes()));
        return Some(format!(
            "local/{}/{}",
            path.file_name()?.to_str()?,
            &digest[..12]
        ));
    }
    if !matches!(url.scheme(), "http" | "https" | "ssh" | "git") {
        return None;
    }
    Some(format!(
        "{}/{}",
        url.host_str()?,
        url.path().trim_matches('/').trim_end_matches(".git")
    ))
}

/// Observations are private, replaceable workspace suggestions, never writes to
/// the shared project catalog. Exact repository identity prevents name collisions.
fn add_observations(index: &mut Index, activity: &Value) -> Result<(), String> {
    let Some(snapshot) = activity.get("snapshot").filter(|s| !s.is_null()) else {
        return Ok(());
    };
    if snapshot["schema"] != "alfalfa.activity/1" {
        index
            .warnings
            .push("My agents returned an unsupported observation format.".into());
        return Ok(());
    }
    let stale = activity["stale"].as_bool().unwrap_or(true);
    if stale {
        index
            .warnings
            .push("My agents observations are stale.".into());
    }
    let known: std::collections::HashSet<_> = index
        .entries
        .iter()
        .filter(|e| e.kind == "repo")
        .filter_map(|e| repository_key(&e.uri))
        .collect();
    let home = project_home("activity")?.replace("/projects/activity", "/activity");
    let mut seen = std::collections::HashSet::new();
    for repo in snapshot["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
    {
        let (Some(key), Some(name)) = (repo["key"].as_str(), repo["name"].as_str()) else {
            continue;
        };
        if known.contains(key) || !seen.insert(key) {
            continue;
        }
        let id = format!("observed-{:x}", Sha256::digest(key.as_bytes()));
        index.entries.push(Entry {
            company_id:None, workspace_id:None, project: id, project_name: name.into(), id: "@home".into(), title: name.into(),
            kind: "project".into(), uri: home.clone(), note: None, home: home.clone(),
            status: "observed".into(),
            purpose: format!("{} · {} agent sessions{}\nDiscovered through My agents; lifecycle has not been assigned.", key,
                repo["agents"].as_array().map_or(0, Vec::len), if stale { " (stale observation)" } else { "" }),
        });
    }
    Ok(())
}

pub fn project_matches(
    index: &Index,
    query: &str,
    project: Option<&str>,
    include_inactive: bool,
) -> Vec<usize> {
    matching(index, query)
        .into_iter()
        .filter(|i| {
            let e = &index.entries[*i];
            match project {
                Some(id) => e.project == id,
                None => {
                    e.kind == "project"
                        && (include_inactive
                            || !matches!(
                                e.status.as_str(),
                                "paused" | "archived" | "retired" | "deprecated" | "end_of_life"
                            ))
                }
            }
        })
        .collect()
}

pub fn matching(index: &Index, query: &str) -> Vec<usize> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    index
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| {
            let search = format!(
                "{} {} {} {} {}s {} {}",
                entry.project,
                entry.project_name,
                entry.title,
                entry.kind,
                entry.kind,
                entry.uri,
                entry.status
            )
            .to_lowercase();
            words.iter().all(|word| search.contains(word)).then_some(i)
        })
        .collect()
}

/// User-activated targets only. Custom schemes and relative paths are delegated
/// to the project page, rather than executed as local app commands.
pub fn target(entry: &Entry) -> String {
    if let Some(view) = owner_view_url(entry) {
        return view;
    }
    match url::Url::parse(&entry.uri) {
        Ok(url)
            if matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            url.into()
        }
        Ok(url)
            if url.scheme() == "file"
                && url.to_file_path().is_ok_and(|p| {
                    p.is_absolute()
                        && (p.is_dir()
                            && !p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                                matches!(
                                    e.to_ascii_lowercase().as_str(),
                                    "app" | "bundle" | "workflow" | "plugin" | "scptd"
                                )
                            })
                            || p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                                matches!(
                                    e.to_ascii_lowercase().as_str(),
                                    "md" | "txt"
                                        | "pdf"
                                        | "csv"
                                        | "json"
                                        | "png"
                                        | "jpg"
                                        | "jpeg"
                                        | "docx"
                                        | "xlsx"
                                        | "pptx"
                                )
                            }))
                }) =>
        {
            url.into()
        }
        _ => entry.home.clone(),
    }
}

/// Resolve engineering resources through the web view owned by their native
/// tool. The Alfalfa URI keeps durable owner identity; the configured base is
/// only where this Canvas host can currently reach that owner.
pub fn owner_view_url(entry: &Entry) -> Option<String> {
    let transmog = std::env::var("CANVAS_TRANSMOG_VIEWER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:4173".into());
    let lob =
        std::env::var("CANVAS_LOB_VIEWER_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    owner_view_url_with_bases(entry, &transmog, &lob)
}

fn owner_view_url_with_bases(entry: &Entry, transmog: &str, lob: &str) -> Option<String> {
    // `url` correctly normalizes dot segments, but an owner locator is an
    // identity, not a navigation request. Reject spelling that would change
    // identity during parsing (including percent-encoded spellings).
    if entry.uri.contains('%')
        || entry
            .uri
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
    {
        return None;
    }
    let locator = url::Url::parse(&entry.uri).ok()?;
    if !locator.username().is_empty()
        || locator.password().is_some()
        || locator.query().is_some()
        || locator.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = locator
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    if segments.len() != 1 || !super::session_identifier(segments[0]) {
        return None;
    }
    let id = segments[0];
    match (entry.kind.as_str(), locator.scheme(), locator.host_str()) {
        ("model", "transmog", Some("projects")) => {
            let mut base = safe_viewer_base(transmog)?;
            let encoded: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
            base.set_fragment(Some(&format!("/p/{encoded}")));
            Some(base.into())
        }
        ("circuit", "lob", Some("circuits")) => {
            let mut base = safe_viewer_base(lob)?;
            base.path_segments_mut()
                .ok()?
                .pop_if_empty()
                .extend(["c", id]);
            Some(base.into())
        }
        _ => None,
    }
}

fn safe_viewer_base(value: &str) -> Option<url::Url> {
    let mut url = url::Url::parse(value).ok()?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if !((url.scheme() == "https" && url.host_str().is_some())
        || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

pub fn open(entry: &Entry) -> Result<(), String> {
    let output = super::bounded_output(
        std::process::Command::new("/usr/bin/open")
            .arg("--")
            .arg(target(entry))
            .stdin(std::process::Stdio::null()),
        8,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err("The selected resource could not be opened.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::json!({"definition":{"project":{"id":"business","name":"Studio business"},"resource":[{"id":"notes","title":"Weekly notes","kind":"note","uri":"notes.md","context_path":"notes.md"},{"id":"sales","title":"Sales dashboard","kind":"artifact","uri":"https://example.test/sales"}]},"sources":[{"path":"notes.md","content":"A real owner note"},{"path":"private.md","content":"Not included"}]})
    }

    fn engineering(kind: &str, uri: &str) -> Entry {
        Entry {
            company_id: None,
            workspace_id: None,
            project: "vehicle".into(),
            project_name: "Vehicle".into(),
            id: "design".into(),
            title: "Design".into(),
            kind: kind.into(),
            uri: uri.into(),
            note: None,
            home: "https://alfalfa.example/projects/vehicle".into(),
            status: "active".into(),
            purpose: String::new(),
        }
    }

    #[test]
    fn engineering_resources_resolve_to_their_owner_web_views() {
        let model = engineering("model", "transmog://projects/front-suspension");
        assert_eq!(
            owner_view_url_with_bases(
                &model,
                "https://transmog.example/viewer",
                "https://lob.example"
            )
            .as_deref(),
            Some("https://transmog.example/viewer#/p/front-suspension")
        );
        let circuit = engineering("circuit", "lob://circuits/controller-board");
        assert_eq!(
            owner_view_url_with_bases(
                &circuit,
                "https://transmog.example",
                "https://lob.example/viewer/"
            )
            .as_deref(),
            Some("https://lob.example/viewer/c/controller-board")
        );
    }

    #[test]
    fn owner_view_resolution_fails_closed() {
        for (kind, locator) in [
            ("model", "transmog://projects/a/b"),
            ("model", "transmog://projects/../secret"),
            ("model", "transmog://other/design"),
            ("circuit", "lob://circuits/board?token=secret"),
            ("file", "transmog://projects/design"),
        ] {
            let entry = engineering(kind, locator);
            assert!(
                owner_view_url_with_bases(
                    &entry,
                    "https://transmog.example",
                    "https://lob.example"
                )
                .is_none(),
                "{locator} must not resolve"
            );
        }
        let model = engineering("model", "transmog://projects/design");
        assert!(
            owner_view_url_with_bases(&model, "http://transmog.example", "https://lob.example")
                .is_none(),
            "plain HTTP is allowed only on loopback"
        );
    }
    #[test]
    fn canvas_workspace_identity_is_distinct_from_company_identity() {
        let mut context = fixture();
        context["definition"]["project"]["company_id"] = Value::Null;
        context["definition"]["workspace_id"] = "fpl".into();
        let entries = entries("business", &context).unwrap();
        assert_eq!(entries[0].company_id, None);
        assert_eq!(entries[0].workspace_id.as_deref(), Some("fpl"));
        assert!(entries
            .iter()
            .all(|entry| entry.workspace_id.as_deref() == Some("fpl")));
    }
    #[test]
    fn company_ids_group_equal_names_without_guessing_unassigned_ownership() {
        let memberships=companies(serde_json::json!([{"id":"one","name":"Studio","role":"owner"},{"id":"two","name":"Studio","role":"member"}])).unwrap();
        let mut context = fixture();
        context["definition"]["project"]["company_id"] = "one".into();
        let mut index = Index {
            companies: memberships,
            entries: entries("business", &context).unwrap(),
            warnings: vec![],
        };
        context["definition"]["project"]["id"] = "other".into();
        context["definition"]["project"]["company_id"] = "two".into();
        index.entries.extend(entries("other", &context).unwrap());
        context["definition"]["project"]["id"] = "legacy".into();
        context["definition"]["project"]["company_id"] = Value::Null;
        context["definition"]["project"]["owner"] = "Studio".into();
        index.entries.extend(entries("legacy", &context).unwrap());
        let groups = folders(&index, "", false);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].company_id.as_deref(), Some("one"));
        assert_eq!(groups[1].company_id.as_deref(), Some("two"));
        assert_eq!(groups[2].company_id, None);
        assert_eq!(
            folder_matches(&index, "", Some("one"), None, false),
            vec![0]
        );
        assert_eq!(
            folder_matches(&index, "sales", Some("two"), Some("other"), false),
            vec![5]
        );
        assert!(folder_matches(&index, "", Some("one"), Some("other"), false).is_empty());
        index.companies.clear();
        assert_eq!(folders(&index, "", false).len(), 1);
        assert!(folder_matches(&index, "", Some("one"), None, true).is_empty());
        assert_eq!(index.entries[0].company_id.as_deref(), Some("one"));
    }
    #[test]
    fn company_folders_honor_lifecycle_and_reject_ambiguous_memberships() {
        assert!(companies(serde_json::json!([{"id":"same","name":"A","role":"owner"},{"id":"same","name":"B","role":"member"}])).is_err());
        let mut index = Index {
            entries: entries("business", &fixture()).unwrap(),
            ..Index::default()
        };
        for entry in &mut index.entries {
            entry.status = "archived".into();
        }
        assert!(folders(&index, "", false).is_empty());
        assert_eq!(folders(&index, "", true)[0].projects, 1);
        assert_eq!(folders(&index, "sales", true).len(), 1);
    }
    #[test]
    fn project_view_scopes_resources_and_hides_inactive_without_deleting_them() {
        let mut index = Index {
            entries: entries("business", &fixture()).unwrap(),
            companies: vec![],
            warnings: vec![],
        };
        assert_eq!(project_matches(&index, "", None, false), vec![0]);
        assert_eq!(
            project_matches(&index, "sales", Some("business"), false),
            vec![2]
        );
        for status in ["paused", "archived", "retired"] {
            index.entries[0].status = status.into();
            assert!(project_matches(&index, "", None, false).is_empty());
            assert_eq!(project_matches(&index, "", None, true), vec![0]);
        }
    }
    #[test]
    fn observed_repositories_merge_exactly_and_do_not_reactivate_retired_projects() {
        let mut context = fixture();
        context["definition"]["project"]["status"] = "archived".into();
        context["definition"]["resource"][0]["kind"] = "repo".into();
        context["definition"]["resource"][0]["uri"] = "git@github.com:owner/repo.git".into();
        let mut index = Index {
            entries: entries("business", &context).unwrap(),
            companies: vec![],
            warnings: vec![],
        };
        let activity = serde_json::json!({"stale":false,"snapshot":{"schema":"alfalfa.activity/1","repositories":[
            {"key":"github.com/owner/repo","name":"repo","agents":[]},
            {"key":"github.com/other/repo","name":"repo","agents":[]},
            {"key":"github.com/other/repo","name":"repo","agents":[]}
        ]}});
        add_observations(&mut index, &activity).unwrap();
        assert_eq!(index.entries.len(), 4);
        assert_eq!(index.entries[0].status, "archived");
        assert_eq!(project_matches(&index, "", None, false), vec![3]);
        assert_eq!(index.entries[3].status, "observed");
        assert!(index.entries[3].uri.ends_with("/activity"));
    }
    #[test]
    fn local_repository_identity_matches_collector_hash_without_name_guessing() {
        let path = std::env::current_dir().unwrap();
        let canonical = path.canonicalize().unwrap();
        let digest = format!(
            "{:x}",
            Sha256::digest(canonical.to_str().unwrap().as_bytes())
        );
        assert_eq!(
            repository_key(url::Url::from_directory_path(&path).unwrap().as_str()).unwrap(),
            format!(
                "local/{}/{}",
                canonical.file_name().unwrap().to_str().unwrap(),
                &digest[..12]
            )
        );
        assert!(repository_key("https://github.com/owner/repo?identity=other").is_none());
    }
    #[test]
    fn search_combines_project_and_resource_and_notes_keep_owner_source() {
        let index = Index {
            entries: entries("business", &fixture()).unwrap(),
            companies: vec![],
            warnings: vec![],
        };
        assert_eq!(matching(&index, "STUDIO sales"), vec![2]);
        assert_eq!(index.entries[1].note.as_deref(), Some("A real owner note"));
        assert!(index.entries[2].note.is_none());
        assert!(entries("different", &fixture()).is_err());
    }
    #[test]
    fn unsafe_or_custom_targets_open_owner_page_instead() {
        let mut entry = entries("business", &fixture()).unwrap().remove(1);
        for uri in [
            "javascript:alert(1)",
            "https://secret@example.test",
            "beads:workspace",
            "--args",
            "../script.sh",
            "file://remote-server/share",
            "file:///tmp/run.command",
            "file:///tmp/untrusted.app",
        ] {
            entry.uri = uri.into();
            assert_eq!(target(&entry), entry.home);
        }
        entry.uri = "https://example.test/dashboard?q=one".into();
        assert_eq!(target(&entry), entry.uri);
    }
    #[test]
    fn application_bundles_are_not_treated_as_document_folders() {
        let path = std::env::temp_dir().join(format!(
            "dock-resource-{}-{}.app",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let mut entry = entries("business", &fixture()).unwrap().remove(1);
        entry.uri = url::Url::from_directory_path(&path).unwrap().into();
        let actual = target(&entry);
        std::fs::remove_dir(&path).unwrap();
        assert_eq!(actual, entry.home);
    }
}
