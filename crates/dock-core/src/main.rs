use dock_core::{Action, Actor, Catalog, Error, Result, StateStore};
use serde_json::{Value, json};
use std::{env, path::PathBuf};

fn run() -> Result<Value> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args == ["--help"] {
        return Ok(
            json!({"usage":"dockctl [--catalog PATH] [--state PATH] [--actor human|agent] projects|status|select ID|take-control ID|release-control ID|theme ID|desktop-theme ID|export-desktop DIRECTORY",
            "scope":"Local workspace policy only. No desktop or remote process action is implied."}),
        );
    }
    let mut catalog_path = None;
    let mut state_path = env::var_os("DOCK_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".dock/state.json"));
    let mut actor = Actor::Agent; // Tool invocations default to the least authority.
    while args.first().is_some_and(|s| s.starts_with("--")) {
        let flag = args.remove(0);
        if args.is_empty() {
            return Err(Error(format!("Missing value for {flag}")));
        }
        let value = args.remove(0);
        match flag.as_str() {
            "--catalog" => catalog_path = Some(PathBuf::from(value)),
            "--state" => state_path = PathBuf::from(value),
            "--actor" => {
                actor = match value.as_str() {
                    "human" => Actor::Human,
                    "agent" => Actor::Agent,
                    _ => return Err(Error("actor must be human or agent".into())),
                }
            }
            _ => return Err(Error(format!("Unknown option {flag}"))),
        }
    }
    let catalog = match catalog_path {
        Some(path) => Catalog::load(path)?,
        None => Catalog::bundled()?,
    };
    let store = StateStore::new(state_path);
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["projects"] => Ok(json!(catalog)),
        ["status"] => Ok(json!(store.load(&catalog)?)),
        ["theme", id] => Ok(json!(catalog.project(id)?.theme.palette())),
        ["desktop-theme", id] => Ok(json!(catalog.project(id)?.theme.desktop_artifacts())),
        ["export-desktop", directory] => {
            let directory = PathBuf::from(directory);
            std::fs::create_dir_all(directory.join("themes")).map_err(|e| Error(e.to_string()))?;
            std::fs::create_dir_all(directory.join("sway/themes"))
                .map_err(|e| Error(e.to_string()))?;
            for project in &catalog.projects {
                let artifacts = project.theme.desktop_artifacts();
                std::fs::write(
                    directory.join("themes").join(format!("{}.css", project.id)),
                    &artifacts.css,
                )
                .map_err(|e| Error(e.to_string()))?;
                std::fs::write(
                    directory
                        .join("sway/themes")
                        .join(format!("{}.conf", project.id)),
                    &artifacts.sway,
                )
                .map_err(|e| Error(e.to_string()))?;
                std::fs::write(
                    directory
                        .join("themes")
                        .join(format!("{}.conf", project.id)),
                    &artifacts.hyprland,
                )
                .map_err(|e| Error(e.to_string()))?;
                std::fs::write(
                    directory
                        .join("themes")
                        .join(format!("foot-{}.ini", project.id)),
                    &artifacts.foot,
                )
                .map_err(|e| Error(e.to_string()))?;
            }
            let initial = catalog.projects[0].theme.desktop_artifacts();
            std::fs::write(directory.join("coherence.conf"), initial.hyprland)
                .map_err(|e| Error(e.to_string()))?;
            std::fs::write(directory.join("foot.ini"), initial.foot)
                .map_err(|e| Error(e.to_string()))?;
            Ok(
                json!({"directory":directory,"projects":catalog.projects.len(),"note":"Default theme uses the first catalog project; running desktops are unchanged"}),
            )
        }
        [
            command @ ("select" | "take-control" | "release-control"),
            id,
        ] => {
            let action = match *command {
                "select" => Action::SelectProject((*id).into()),
                "take-control" => Action::TakeControl((*id).into()),
                _ => Action::ReleaseControl((*id).into()),
            };
            Ok(json!(store.update(&catalog, |s| s.apply(actor, action))?))
        }
        _ => Err(Error("Invalid command; use dockctl --help".into())),
    }
}
fn main() {
    match run() {
        Ok(value) => println!("{}", json!({"schema_version":1,"ok":true,"result":value})),
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":1,"ok":false,"error":error.to_string()})
            );
            std::process::exit(1);
        }
    }
}
