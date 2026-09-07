//! Deterministic compilation from resolved presentation intent to Canvas state.
//!
//! Callers resolve natural language, project identity, and authorized resources
//! before constructing [`DesiredWorkspaceState`]. This module performs no search,
//! model inference, I/O, or socket operations.

use crate::{
    Background, DataBinding, Layout, PatchOperation, SCHEMA_VERSION, Surface, ThemeRef,
    WorkspaceDocument, WorkspacePatch,
};
use dock_core::Catalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ReconcileMode {
    /// Add or update requested state while preserving everything unmentioned.
    #[default]
    Preserve,
    /// Make surfaces, bindings, and layout exactly match the desired state.
    Exact,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum DesiredFocus {
    #[default]
    Preserve,
    Clear,
    Surface {
        surface_id: String,
    },
}

/// Resolved, authorized presentation intent. Every ID is exact by this point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DesiredWorkspaceState {
    pub schema_version: u32,
    pub workspace_id: String,
    pub project_id: String,
    #[serde(default)]
    pub mode: ReconcileMode,
    #[serde(default)]
    pub background: Option<Background>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default)]
    pub bindings: Vec<DataBinding>,
    #[serde(default)]
    pub layout: Option<Layout>,
    #[serde(default)]
    pub focus: DesiredFocus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum StateChange {
    Project { from: String, to: String },
    SurfaceUpserted { surface_id: String },
    SurfaceRemoved { surface_id: String },
    BindingUpserted { binding_id: String },
    BindingRemoved { binding_id: String },
    Layout,
    Theme,
    Background,
    Focus,
}

/// Dry-run result. The caller may inspect `changes` before applying `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Reconciliation {
    pub changes: Vec<StateChange>,
    pub action: ReconcileAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum ReconcileAction {
    NoChange { document_revision: u64 },
    ApplyPatch { patch: WorkspacePatch },
    ApplyDocument { document: WorkspaceDocument },
}

pub fn reconcile(
    current: &WorkspaceDocument,
    desired: &DesiredWorkspaceState,
    catalog: &Catalog,
) -> crate::Result<Reconciliation> {
    current.validate(catalog)?;
    require(
        desired.schema_version == SCHEMA_VERSION,
        "Unsupported desired workspace schema_version",
    )?;
    require(
        desired.workspace_id == current.id,
        "Desired state targets another workspace",
    )?;
    catalog
        .project(&desired.project_id)
        .map_err(|error| crate::Error(error.to_string()))?;
    require_unique(
        desired.surfaces.iter().map(|surface| surface.id.as_str()),
        "desired surface",
    )?;
    require_unique(
        desired.bindings.iter().map(|binding| binding.id.as_str()),
        "desired binding",
    )?;
    if desired.mode == ReconcileMode::Exact {
        require(
            desired.layout.is_some(),
            "Exact desired state requires an explicit layout",
        )?;
    }
    if desired.project_id != current.project_id {
        require(
            desired.mode == ReconcileMode::Exact,
            "Changing projects requires exact desired state",
        )?;
        return replace_project(current, desired, catalog);
    }

    let target = build_same_project_target(current, desired, catalog)?;
    let (operations, changes) = diff(current, &target);
    if operations.is_empty() {
        return Ok(Reconciliation {
            changes,
            action: ReconcileAction::NoChange {
                document_revision: current.revision,
            },
        });
    }
    let patch = WorkspacePatch {
        schema_version: SCHEMA_VERSION,
        workspace_id: current.id.clone(),
        base_revision: current.revision,
        revision: current.revision + 1,
        operations,
    };
    let applied = current.apply_patch(&patch, catalog)?;
    require(
        applied == target,
        "Reconciler produced a patch that does not converge to its target",
    )?;
    Ok(Reconciliation {
        changes,
        action: ReconcileAction::ApplyPatch { patch },
    })
}

fn replace_project(
    current: &WorkspaceDocument,
    desired: &DesiredWorkspaceState,
    catalog: &Catalog,
) -> crate::Result<Reconciliation> {
    let mut document = WorkspaceDocument {
        schema_version: SCHEMA_VERSION,
        id: current.id.clone(),
        project_id: desired.project_id.clone(),
        revision: current.revision + 1,
        theme: ThemeRef::Project,
        background: desired.background.clone().unwrap_or_default(),
        surfaces: sorted_surfaces(&desired.surfaces),
        layout: desired
            .layout
            .clone()
            .expect("exact mode was validated above"),
        bindings: sorted_bindings(&desired.bindings),
        focused_surface_id: None,
    };
    apply_focus(&mut document, &desired.focus);
    document.validate(catalog)?;
    let mut changes = vec![StateChange::Project {
        from: current.project_id.clone(),
        to: document.project_id.clone(),
    }];
    changes.extend(
        document
            .surfaces
            .iter()
            .map(|surface| StateChange::SurfaceUpserted {
                surface_id: surface.id.clone(),
            }),
    );
    changes.extend(
        document
            .bindings
            .iter()
            .map(|binding| StateChange::BindingUpserted {
                binding_id: binding.id.clone(),
            }),
    );
    changes.push(StateChange::Layout);
    changes.push(StateChange::Theme);
    changes.push(StateChange::Background);
    if !matches!(desired.focus, DesiredFocus::Preserve) {
        changes.push(StateChange::Focus);
    }
    Ok(Reconciliation {
        changes,
        action: ReconcileAction::ApplyDocument { document },
    })
}

fn build_same_project_target(
    current: &WorkspaceDocument,
    desired: &DesiredWorkspaceState,
    catalog: &Catalog,
) -> crate::Result<WorkspaceDocument> {
    let mut target = current.clone();
    match desired.mode {
        ReconcileMode::Preserve => {
            for surface in sorted_surfaces(&desired.surfaces) {
                upsert(&mut target.surfaces, surface, |value| &value.id);
            }
            for binding in sorted_bindings(&desired.bindings) {
                upsert(&mut target.bindings, binding, |value| &value.id);
            }
        }
        ReconcileMode::Exact => {
            target.surfaces = sorted_surfaces(&desired.surfaces);
            target.bindings = sorted_bindings(&desired.bindings);
        }
    }
    if let Some(layout) = &desired.layout {
        target.layout.clone_from(layout);
    }
    if let Some(background) = &desired.background {
        target.background.clone_from(background);
    }
    apply_focus(&mut target, &desired.focus);
    if target != *current {
        target.revision = current.revision + 1;
    }
    target.validate(catalog)?;
    Ok(target)
}

fn apply_focus(document: &mut WorkspaceDocument, focus: &DesiredFocus) {
    match focus {
        DesiredFocus::Preserve => {}
        DesiredFocus::Clear => document.focused_surface_id = None,
        DesiredFocus::Surface { surface_id } => {
            document.focused_surface_id = Some(surface_id.clone());
        }
    }
}

fn diff(
    current: &WorkspaceDocument,
    target: &WorkspaceDocument,
) -> (Vec<PatchOperation>, Vec<StateChange>) {
    let mut operations = Vec::new();
    let mut changes = Vec::new();
    let target_binding_ids = target
        .bindings
        .iter()
        .map(|binding| binding.id.as_str())
        .collect::<BTreeSet<_>>();
    let target_surface_ids = target
        .surfaces
        .iter()
        .map(|surface| surface.id.as_str())
        .collect::<BTreeSet<_>>();

    let mut removed_bindings = current
        .bindings
        .iter()
        .filter(|binding| !target_binding_ids.contains(binding.id.as_str()))
        .map(|binding| binding.id.clone())
        .collect::<Vec<_>>();
    removed_bindings.sort();
    for binding_id in removed_bindings {
        operations.push(PatchOperation::RemoveBinding {
            binding_id: binding_id.clone(),
        });
        changes.push(StateChange::BindingRemoved { binding_id });
    }

    let mut removed_surfaces = current
        .surfaces
        .iter()
        .filter(|surface| !target_surface_ids.contains(surface.id.as_str()))
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    removed_surfaces.sort();
    for surface_id in removed_surfaces {
        operations.push(PatchOperation::RemoveSurface {
            surface_id: surface_id.clone(),
        });
        changes.push(StateChange::SurfaceRemoved { surface_id });
    }

    for surface in &target.surfaces {
        if current
            .surfaces
            .iter()
            .find(|candidate| candidate.id == surface.id)
            != Some(surface)
        {
            operations.push(PatchOperation::UpsertSurface {
                surface: surface.clone(),
            });
            changes.push(StateChange::SurfaceUpserted {
                surface_id: surface.id.clone(),
            });
        }
    }
    for binding in &target.bindings {
        if current
            .bindings
            .iter()
            .find(|candidate| candidate.id == binding.id)
            != Some(binding)
        {
            operations.push(PatchOperation::UpsertBinding {
                binding: binding.clone(),
            });
            changes.push(StateChange::BindingUpserted {
                binding_id: binding.id.clone(),
            });
        }
    }
    if current.layout != target.layout {
        operations.push(PatchOperation::SetLayout {
            layout: target.layout.clone(),
        });
        changes.push(StateChange::Layout);
    }
    if current.theme != target.theme {
        operations.push(PatchOperation::SetTheme {
            theme: target.theme,
        });
        changes.push(StateChange::Theme);
    }
    if current.background != target.background {
        operations.push(PatchOperation::SetBackground {
            background: target.background.clone(),
        });
        changes.push(StateChange::Background);
    }
    if current.focused_surface_id != target.focused_surface_id {
        operations.push(PatchOperation::SetFocus {
            surface_id: target.focused_surface_id.clone(),
        });
        changes.push(StateChange::Focus);
    }
    (operations, changes)
}

fn sorted_surfaces(surfaces: &[Surface]) -> Vec<Surface> {
    let mut sorted = surfaces.to_vec();
    sorted.sort_by(|left, right| left.id.cmp(&right.id));
    sorted
}

fn sorted_bindings(bindings: &[DataBinding]) -> Vec<DataBinding> {
    let mut sorted = bindings.to_vec();
    sorted.sort_by(|left, right| left.id.cmp(&right.id));
    sorted
}

fn upsert<T, F>(values: &mut Vec<T>, value: T, id: F)
where
    F: Fn(&T) -> &String,
{
    if let Some(index) = values
        .iter()
        .position(|candidate| id(candidate) == id(&value))
    {
        values[index] = value;
    } else {
        values.push(value);
    }
}

fn require_unique<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> crate::Result<()> {
    let mut unique = BTreeSet::new();
    for value in values {
        require(
            unique.insert(value),
            format!("Duplicate {label} id: {value}"),
        )?;
    }
    Ok(())
}

fn require(ok: bool, message: impl Into<String>) -> crate::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(crate::Error(message.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataSource, Placement};
    use crate::{InputPort, InputTarget, NormalizedRect, SurfaceContent};

    fn catalog() -> Catalog {
        Catalog::parse(
            r#"{
              "schema_version": 1,
              "projects": [
                {
                  "id": "dock",
                  "name": "Dock",
                  "theme": {"source":"coherence","mode":"dark","accent":"info"},
                  "resources": [
                    {"id":"docs","title":"Docs","host":"mac","kind":"mac_app","target":"com.apple.Safari"},
                    {"id":"invoice","title":"Invoice","host":"mac","kind":"mac_app","target":"com.apple.Preview"},
                    {"id":"shell","title":"Shell","host":"agora_one","kind":"terminal_session","target":"dock"}
                  ]
                },
                {
                  "id": "ohm",
                  "name": "Ohm",
                  "theme": {"source":"coherence","mode":"light","accent":"success"},
                  "resources": [
                    {"id":"board","title":"Ohm Board","host":"mac","kind":"mac_app","target":"com.apple.Safari"}
                  ]
                }
              ]
            }"#,
        )
        .unwrap()
    }

    fn resource_surface(id: &str, resource_id: &str) -> Surface {
        Surface {
            id: id.into(),
            title: id.into(),
            content: SurfaceContent::ProjectResource {
                resource_id: resource_id.into(),
            },
            inputs: Vec::new(),
        }
    }

    fn current() -> WorkspaceDocument {
        WorkspaceDocument {
            schema_version: SCHEMA_VERSION,
            id: "primary".into(),
            project_id: "dock".into(),
            revision: 7,
            theme: ThemeRef::Project,
            background: crate::Background::default(),
            surfaces: vec![
                resource_surface("docs", "docs"),
                Surface {
                    id: "shell".into(),
                    title: "shell".into(),
                    content: SurfaceContent::Terminal {
                        resource_id: "shell".into(),
                    },
                    inputs: vec![InputPort {
                        id: "status".into(),
                        contract: "text/plain".into(),
                    }],
                },
            ],
            layout: Layout::Canvas {
                placements: vec![
                    Placement {
                        surface_id: "docs".into(),
                        bounds: NormalizedRect {
                            x: 0.0,
                            y: 0.0,
                            width: 0.7,
                            height: 1.0,
                        },
                        layer: 0,
                    },
                    Placement {
                        surface_id: "shell".into(),
                        bounds: NormalizedRect {
                            x: 0.7,
                            y: 0.0,
                            width: 0.3,
                            height: 1.0,
                        },
                        layer: 1,
                    },
                ],
            },
            bindings: vec![DataBinding {
                id: "shell-status".into(),
                source: DataSource {
                    owner: "opencode".into(),
                    stream: "status".into(),
                    contract: "text/plain".into(),
                },
                target: InputTarget {
                    surface_id: "shell".into(),
                    input_id: "status".into(),
                },
            }],
            focused_surface_id: Some("docs".into()),
        }
    }

    fn desired() -> DesiredWorkspaceState {
        DesiredWorkspaceState {
            schema_version: SCHEMA_VERSION,
            workspace_id: "primary".into(),
            project_id: "dock".into(),
            mode: ReconcileMode::Preserve,
            background: None,
            surfaces: Vec::new(),
            bindings: Vec::new(),
            layout: None,
            focus: DesiredFocus::Preserve,
        }
    }

    #[test]
    fn empty_preserving_intent_converges_to_no_change() {
        let plan = reconcile(&current(), &desired(), &catalog()).unwrap();
        assert_eq!(plan.changes, Vec::new());
        assert_eq!(
            plan.action,
            ReconcileAction::NoChange {
                document_revision: 7
            }
        );
    }

    #[test]
    fn background_intent_is_a_minimal_idempotent_state_change() {
        let current = current();
        let mut desired = desired();
        desired.background = Some(Background::Solid {
            color: [4, 8, 16, 255],
        });
        let plan = reconcile(&current, &desired, &catalog()).unwrap();
        assert_eq!(plan.changes, vec![StateChange::Background]);
        let ReconcileAction::ApplyPatch { patch } = plan.action else {
            panic!("expected a patch");
        };
        assert_eq!(patch.operations.len(), 1);
        assert!(matches!(
            patch.operations[0],
            PatchOperation::SetBackground { .. }
        ));
        let applied = current.apply_patch(&patch, &catalog()).unwrap();
        assert_eq!(
            reconcile(&applied, &desired, &catalog()).unwrap().action,
            ReconcileAction::NoChange {
                document_revision: applied.revision
            }
        );
    }

    #[test]
    fn resource_reveal_is_a_minimal_patch_and_preserves_human_surfaces() {
        let current = current();
        let mut desired = desired();
        desired.surfaces = vec![resource_surface("invoice", "invoice")];
        desired.focus = DesiredFocus::Surface {
            surface_id: "invoice".into(),
        };
        let plan = reconcile(&current, &desired, &catalog()).unwrap();
        let ReconcileAction::ApplyPatch { patch } = plan.action else {
            panic!("expected a patch");
        };
        assert_eq!(patch.operations.len(), 2);
        assert!(matches!(
            patch.operations[0],
            PatchOperation::UpsertSurface { .. }
        ));
        assert!(matches!(
            patch.operations[1],
            PatchOperation::SetFocus { .. }
        ));
        let applied = current.apply_patch(&patch, &catalog()).unwrap();
        assert!(applied.surfaces.iter().any(|surface| surface.id == "docs"));
        assert!(applied.surfaces.iter().any(|surface| surface.id == "shell"));
        assert!(
            applied
                .surfaces
                .iter()
                .any(|surface| surface.id == "invoice")
        );
    }

    #[test]
    fn upserts_are_stably_ordered_by_exact_id() {
        let mut desired = desired();
        desired.surfaces = vec![
            resource_surface("z-last", "invoice"),
            resource_surface("a-first", "invoice"),
        ];
        let plan = reconcile(&current(), &desired, &catalog()).unwrap();
        let ReconcileAction::ApplyPatch { patch } = plan.action else {
            panic!("expected a patch");
        };
        let ids = patch
            .operations
            .iter()
            .filter_map(|operation| match operation {
                PatchOperation::UpsertSurface { surface } => Some(surface.id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, ["a-first", "z-last"]);
    }

    #[test]
    fn exact_mode_removes_unmentioned_state_and_self_validates() {
        let current = current();
        let mut desired = desired();
        desired.mode = ReconcileMode::Exact;
        desired.surfaces = vec![resource_surface("invoice", "invoice")];
        desired.layout = Some(Layout::Surface {
            surface_id: "invoice".into(),
        });
        desired.focus = DesiredFocus::Surface {
            surface_id: "invoice".into(),
        };
        let plan = reconcile(&current, &desired, &catalog()).unwrap();
        let ReconcileAction::ApplyPatch { patch } = plan.action else {
            panic!("expected a patch");
        };
        let applied = current.apply_patch(&patch, &catalog()).unwrap();
        assert_eq!(
            applied.surfaces,
            vec![resource_surface("invoice", "invoice")]
        );
        assert!(applied.bindings.is_empty());
        assert_eq!(applied.focused_surface_id.as_deref(), Some("invoice"));
    }

    #[test]
    fn project_recall_requires_exact_state_and_returns_a_document() {
        let mut preserving = desired();
        preserving.project_id = "ohm".into();
        assert!(
            reconcile(&current(), &preserving, &catalog())
                .unwrap_err()
                .to_string()
                .contains("requires exact")
        );

        preserving.mode = ReconcileMode::Exact;
        preserving.surfaces = vec![resource_surface("board", "board")];
        preserving.layout = Some(Layout::Surface {
            surface_id: "board".into(),
        });
        preserving.focus = DesiredFocus::Surface {
            surface_id: "board".into(),
        };
        let plan = reconcile(&current(), &preserving, &catalog()).unwrap();
        let ReconcileAction::ApplyDocument { document } = plan.action else {
            panic!("expected a document replacement");
        };
        assert_eq!(document.project_id, "ohm");
        assert_eq!(document.revision, 8);
        document.validate(&catalog()).unwrap();
    }

    #[test]
    fn missing_or_duplicate_resolved_resources_fail_loudly() {
        let mut missing = desired();
        missing.surfaces = vec![resource_surface("invoice", "not-real")];
        assert!(reconcile(&current(), &missing, &catalog()).is_err());

        let mut duplicate = desired();
        duplicate.surfaces = vec![
            resource_surface("invoice", "invoice"),
            resource_surface("invoice", "invoice"),
        ];
        assert!(
            reconcile(&current(), &duplicate, &catalog())
                .unwrap_err()
                .to_string()
                .contains("Duplicate desired surface")
        );
    }
}
