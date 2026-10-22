use canvas::{
    control::Command,
    shell_launcher::{Catalog, Choice, Entry, Input, LEASE_MS, Launcher, Scope, Target},
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
    assert_eq!(l.buttons.len(), 3);
    assert!(l.buttons.iter().any(|button| button.label == "Studio"));
    // Native buttons own row titles; the retained scene must not draw a second
    // copy behind them (native buttons can be translucent).
    assert!(
        !scene
            .nodes
            .iter()
            .any(|n| matches!(&n.content, canvas::NodeContent::Text{text,..} if text == "Studio"))
    );
    l.render(theme, 100 + LEASE_MS).unwrap();
    assert!(l.scene.as_ref().unwrap().nodes.iter().any(
        |n| matches!(&n.content,canvas::NodeContent::Text{text,..} if text.contains("STALE"))
    ));
}

#[test]
fn shell_views_separate_workspaces_canvas_windows_and_native_apps() {
    let mut l = Launcher::new().unwrap();
    let mut c = catalog();
    c.entries.insert(
        0,
        Entry {
            id: "app:/Applications/AAAA.app".into(),
            title: "AAAA".into(),
            detail: "APPLICATION · native".into(),
            available: true,
            choice: Choice::Application {
                path: "/Applications/AAAA.app".into(),
            },
        },
    );
    c.entries.push(Entry {
        id: "surface:clock".into(),
        title: "Clock".into(),
        detail: "WINDOW".into(),
        available: true,
        choice: Choice::Surface { id: "clock".into() },
    });
    let full_count = c.entries.len();
    l.accept(l.generation, Ok(c), 100);
    l.canvas_surface_ids = Some(["surf".into()].into_iter().collect());
    l.set_scope(Scope::Workspaces);
    assert_eq!(
        l.matches()
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["workspace:studio"]
    );
    l.query = "AAAA".into();
    assert!(l.matches().is_empty());
    l.set_scope(Scope::Canvas);
    assert_eq!(
        l.matches()
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["surface:surf"]
    );
    assert!(l.action(101).is_none()); // Unavailable Canvas surface remains unavailable.
    l.set_scope(Scope::Native);
    assert_eq!(
        l.matches()
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["app:/Applications/AAAA.app"]
    );
    assert!(
        matches!(l.action(101), Some((_, Command::ShellActivate { entry_id, catalog_revision })) if entry_id == "app:/Applications/AAAA.app" && catalog_revision == "a".repeat(64))
    );
    assert_eq!(l.catalog.as_ref().unwrap().entries.len(), full_count);
    l.set_scope(Scope::Canvas);
    l.targets.push(Target {
        id: "remote".into(),
        label: "Remote screen".into(),
        mycelium_peer: Some("remote".into()),
        unix_user: None,
    });
    l.target = 1;
    assert_eq!(l.matches().len(), 2); // Never filter a remote catalog with local surface IDs.
}

#[test]
fn changing_shell_view_rejects_old_pointer_and_catalog_results() {
    let mut l = Launcher::new().unwrap();
    l.accept(l.generation, Ok(catalog()), 100);
    let generation = l.generation;
    l.loading = true; // Do not invalidate a pending catalog request.
    l.set_scope(Scope::Workspaces);
    assert!(l.loading);
    assert_eq!(l.scope, Scope::All);
    assert_eq!(l.generation, generation);
    l.handle(&key(iced::keyboard::key::Named::F2));
    assert_eq!(l.scope, Scope::All);
    l.accept(generation, Ok(catalog()), 100);
    l.set_scope(Scope::Workspaces);
    assert!(!l.loading);
    assert!(!l.select_entry(generation, &"a".repeat(64), "workspace:studio", 101));
    l.accept(generation, Err("late refresh".into()), 101);
    assert!(l.catalog.is_some());
    assert!(l.select_entry(l.generation, &"a".repeat(64), "workspace:studio", 101));
    assert!(!l.select_entry(l.generation, &"a".repeat(64), "surface:surf", 101));
    l.busy = true;
    l.set_scope(Scope::Native);
    assert_eq!(l.scope, Scope::Workspaces);
    l.busy = false;
    l.handle(&key(iced::keyboard::key::Named::F2));
    assert_eq!(l.scope, Scope::Workspaces);
    l.set_scope(Scope::Canvas);
    assert!(matches!(
        l.handle(&key(iced::keyboard::key::Named::F2)),
        Input::Consumed
    ));
    assert_eq!(l.scope, Scope::Native);
    l.handle(&key(iced::keyboard::key::Named::F2));
    assert_eq!(l.scope, Scope::Canvas);
}

#[cfg(feature = "lua-scenes")]
#[test]
fn scoped_launcher_labels_match_the_view_and_cycle_has_no_native_app_control() {
    let mut l = Launcher::new().unwrap();
    l.accept(l.generation, Ok(catalog()), 100);
    for (scope, heading) in [
        (Scope::Workspaces, "Workspaces"),
        (Scope::Canvas, "Canvas apps"),
        (Scope::Native, "Native apps"),
    ] {
        l.set_scope(scope);
        l.render(
            canvas_core::ProjectTheme {
                mode: canvas_core::ThemeMode::Dark,
                accent: canvas_core::Accent::Brand,
            }
            .into(),
            101,
        )
        .unwrap();
        assert!(
            l.scene.as_ref().unwrap().nodes.iter().any(
                |n| matches!(&n.content,canvas::NodeContent::Text{text,..} if text == heading)
            )
        );
        if scope == Scope::Workspaces {
            assert!(
                !l.buttons
                    .iter()
                    .any(|b| b.id == "canvas-apps" || b.id == "native-apps")
            );
        } else {
            assert!(l.buttons.iter().any(|b| b.id
                == if scope == Scope::Native {
                    "canvas-apps"
                } else {
                    "native-apps"
                }));
        }
    }
    l.scope = Scope::All;
    l.window_cycle = true;
    l.render(
        canvas_core::ProjectTheme {
            mode: canvas_core::ThemeMode::Dark,
            accent: canvas_core::Accent::Brand,
        }
        .into(),
        101,
    )
    .unwrap();
    assert!(l.buttons.iter().all(|b| b.id.starts_with("row")));
}
