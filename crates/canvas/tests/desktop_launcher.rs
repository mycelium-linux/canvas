use canvas::{
    control::Command,
    shell_launcher::{Catalog, Choice, Entry, Input, LEASE_MS, Launcher, Target},
};
fn catalog() -> Catalog {
    Catalog {
        revision: "a".repeat(64),
        entries: vec![
            Entry {
                id: "workspace:studio".into(),
                title: "Studio".into(),
                detail: "WORKSPACE".into(),
                available: true,
                choice: Choice::Workspace {
                    id: "studio".into(),
                },
            },
            Entry {
                id: "surface:surf".into(),
                title: "Surf browser".into(),
                detail: "WINDOW · disconnected".into(),
                available: false,
                choice: Choice::Surface { id: "surf".into() },
            },
        ],
    }
}
fn key(named: iced::keyboard::key::Named) -> iced::keyboard::Event {
    use iced::keyboard::{Event, Key, key};
    Event::KeyPressed {
        key: Key::Named(named),
        modified_key: Key::Named(named),
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    }
}
#[test]
fn target_switch_clears_catalog_and_rejects_late_results() {
    let mut launcher = Launcher::new().unwrap();
    launcher.targets = vec![
        Target {
            id: "neo".into(),
            label: "Neo".into(),
            mycelium_peer: None,
            unix_user: None,
        },
        Target {
            id: "tv".into(),
            label: "TV".into(),
            mycelium_peer: Some("linux-home-pi".into()),
            unix_user: Some("ajm".into()),
        },
    ];
    let generation = launcher.generation;
    launcher.accept(generation, Ok(catalog()), 100);
    assert!(launcher.action(101).is_some());
    assert!(matches!(
        launcher.handle(&key(iced::keyboard::key::Named::Tab)),
        Input::Reload
    ));
    assert!(launcher.catalog.is_none());
    assert!(launcher.action(101).is_none());
    launcher.accept(generation, Ok(catalog()), 102);
    assert!(launcher.catalog.is_none());
    launcher.accept(launcher.generation, Ok(catalog()), 103);
    let (target, command) = launcher.action(104).unwrap();
    assert_eq!(target.id, "tv");
    assert!(
        matches!(command,Command::ShellActivate{catalog_revision,..} if catalog_revision=="a".repeat(64))
    );
}
#[test]
fn unavailable_stale_busy_and_empty_choices_cannot_execute() {
    let mut l = Launcher::new().unwrap();
    l.accept(l.generation, Ok(catalog()), 100);
    l.query = "browser".into();
    assert_eq!(l.matches().len(), 1);
    assert!(l.action(101).is_none());
    l.query = "studio".into();
    assert!(l.action(101).is_some());
    assert!(l.action(100 + LEASE_MS).is_none());
    l.busy = true;
    assert!(l.action(101).is_none());
    l.busy = false;
    l.query = "no such choice".into();
    assert!(l.action(101).is_none());
}

#[test]
fn pointer_choice_rejects_changed_catalog_generation_and_unavailable_rows() {
    let mut l = Launcher::new().unwrap();
    let generation = l.generation;
    let revision = "a".repeat(64);
    l.accept(generation, Ok(catalog()), 100);
    assert!(!l.select_entry(generation + 1, &revision, "workspace:studio", 101));
    assert!(!l.select_entry(generation, "wrong", "workspace:studio", 101));
    assert!(!l.select_entry(generation, &revision, "surface:surf", 101));
    assert!(!l.select_entry(generation, &revision, "unknown", 101));
    assert!(!l.select_entry(generation, &revision, "workspace:studio", 100 + LEASE_MS));
    assert!(l.select_entry(generation, &revision, "workspace:studio", 101));
    assert!(
        matches!(l.action(101), Some((_, Command::ShellActivate { entry_id, .. })) if entry_id == "workspace:studio")
    );
    l.busy = true;
    assert!(!l.select_entry(generation, &revision, "workspace:studio", 101));
}
#[test]
fn reopening_never_reuses_a_pending_generation() {
    assert_ne!(
        Launcher::new().unwrap().generation,
        Launcher::new().unwrap().generation
    );
}
#[cfg(feature = "lua-scenes")]
#[test]
fn lua_shell_uses_themed_primitives_and_never_issues_actions() {
    use canvas_core::{Accent, ProjectTheme, ThemeMode};
    let mut l = Launcher::new().unwrap();
    l.accept(l.generation, Ok(catalog()), 100);
    let theme: canvas::lua_module::ModuleThemeTokens = ProjectTheme {
        mode: ThemeMode::Dark,
        accent: Accent::Info,
    }
    .into();
    l.render(theme, 101).unwrap();
    let scene = l.scene.as_ref().unwrap();
    assert!(!scene.nodes.is_empty());
    assert_eq!(l.buttons.len(), 2);
    assert!(l.buttons.iter().any(|button| button.label == "Studio"));
    // Native buttons own row titles; the retained scene must not draw a second
    // copy behind them (native buttons can be translucent).
    assert!(!scene.nodes.iter().any(
        |n| matches!(&n.content, canvas::NodeContent::Text{text,..} if text == "Studio")
    ));
    l.render(theme, 100 + LEASE_MS).unwrap();
    assert!(l.scene.as_ref().unwrap().nodes.iter().any(
        |n| matches!(&n.content,canvas::NodeContent::Text{text,..} if text.contains("STALE"))
    ));
}
