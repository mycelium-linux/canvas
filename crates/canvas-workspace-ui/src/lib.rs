//! Host-embeddable work inbox. Rendering never performs provider I/O.
use coherence_native::{
    Tokens,
    egui::widgets::{self, Columns, DenseRow, Tone},
};
use egui::{Align, Key, Layout, RichText, ScrollArea, TextEdit};
use serde::{Deserialize, Serialize};

pub mod demo;
pub mod mail;
pub mod shell;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub mod web;

/// Owner-derived work status. Claims remain distinct from execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Claimed,
    Active,
    Review,
    Blocked,
    Deferred,
    Done,
    #[default]
    Unknown,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Claimed => "Claimed",
            Self::Active => "In progress",
            Self::Review => "In review",
            Self::Blocked => "Blocked",
            Self::Deferred => "Deferred",
            Self::Done => "Done",
            Self::Unknown => "Unknown",
        }
    }
    fn tone(self) -> Tone {
        match self {
            Self::Active => Tone::Info,
            Self::Claimed | Self::Review => Tone::Warning,
            Self::Blocked => Tone::Danger,
            Self::Done => Tone::Success,
            _ => Tone::Neutral,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coherence_native::{Density, Mode};

    fn snapshot(count: usize) -> Snapshot {
        Snapshot {revision:1,items:(0..count).map(|index|Item {id:format!("owner:{index:06}"),title:format!("Work item {index} with a very long title that must remain in its own row even when the available viewport is narrow"),status:Status::Open,priority:2,project:"Canvas".into(),owner:"You".into(),updated:"Oct 19".into(),description:"The full context belongs beside the selected work item.".into(),source:Some("owner/docs/design.md".into()),evidence:vec!["commit: 1234".into()],dependencies:vec![]}).collect(),fresh:true,source:"Marbles".into(),observed:"12:00 UTC".into(),notice:None}
    }
    fn frame(
        context: &egui::Context,
        inbox: &mut Inbox,
        snapshot: &Snapshot,
        width: f32,
        events: Vec<egui::Event>,
        active: bool,
    ) -> Report {
        let tokens = Tokens::new(Mode::Dark, Density::Compact);
        context.set_style(coherence_native::egui::style(&tokens).expect("appearance"));
        let mut report = Report::default();
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 800.0),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    report = inbox.show(
                        ui,
                        &tokens,
                        snapshot,
                        Options {
                            keyboard_active: active,
                            ..Default::default()
                        },
                    )
                });
            },
        );
        report
    }
    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn renders_only_visible_rows_from_large_work_sets() {
        let context = egui::Context::default();
        let mut inbox = Inbox::default();
        let snapshot = snapshot(10_000);
        let report = frame(&context, &mut inbox, &snapshot, 1600.0, vec![], true);
        assert_eq!(report.matched, 10_000);
        assert!(
            report.rows_drawn > 5 && report.rows_drawn < 60,
            "{} rows drawn",
            report.rows_drawn
        );
        for pair in report.row_rects.windows(2) {
            assert!(
                pair[0].bottom() <= pair[1].top() + 0.1,
                "overlapping work rows"
            );
        }
    }
    #[test]
    fn selection_survives_reordering_and_new_owner_generations() {
        let context = egui::Context::default();
        let mut inbox = Inbox::default();
        let mut snapshot = snapshot(8);
        frame(&context, &mut inbox, &snapshot, 1400.0, vec![], true);
        frame(
            &context,
            &mut inbox,
            &snapshot,
            1400.0,
            vec![key(Key::ArrowDown)],
            true,
        );
        let selected = inbox.selected.clone();
        assert_eq!(selected.as_deref(), Some("owner:000001"));
        snapshot.items.reverse();
        snapshot.revision += 1;
        frame(&context, &mut inbox, &snapshot, 1400.0, vec![], true);
        assert_eq!(inbox.selected, selected);
    }
    #[test]
    fn host_focus_and_search_editing_prevent_shortcuts_from_stealing_input() {
        let context = egui::Context::default();
        let mut inbox = Inbox::default();
        let snapshot = snapshot(8);
        frame(&context, &mut inbox, &snapshot, 1400.0, vec![], true);
        let selected = inbox.selected.clone();
        frame(
            &context,
            &mut inbox,
            &snapshot,
            1400.0,
            vec![key(Key::J)],
            false,
        );
        assert_eq!(inbox.selected, selected);
        frame(
            &context,
            &mut inbox,
            &snapshot,
            1400.0,
            vec![key(Key::Slash)],
            true,
        );
        frame(
            &context,
            &mut inbox,
            &snapshot,
            1400.0,
            vec![key(Key::J)],
            true,
        );
        assert_eq!(inbox.selected, selected);
    }
    #[test]
    fn narrow_layout_and_empty_search_keep_a_bounded_work_surface() {
        let context = egui::Context::default();
        let mut inbox = Inbox::default();
        let snapshot = snapshot(8);
        let report = frame(&context, &mut inbox, &snapshot, 480.0, vec![], true);
        assert_eq!(report.matched, 8);
        assert!(report.row_rects.iter().all(|r| r.width() <= 480.0));
        inbox.query = "this cannot match".into();
        let report = frame(&context, &mut inbox, &snapshot, 480.0, vec![], true);
        assert_eq!(report.rows_drawn, 0);
        assert!(inbox.selected.is_none());
    }
    #[test]
    fn duplicate_identity_is_rejected_and_stale_views_emit_no_domain_mutations() {
        let mut snapshot = snapshot(2);
        snapshot.items[1].id = snapshot.items[0].id.clone();
        assert!(snapshot.validate().is_err());
        snapshot.items[1].id = "another-owner:1".into();
        snapshot.fresh = false;
        snapshot.validate().expect("valid stale observation");
        let context = egui::Context::default();
        let mut inbox = Inbox::default();
        let report = frame(
            &context,
            &mut inbox,
            &snapshot,
            1400.0,
            vec![key(Key::E)],
            true,
        );
        assert!(report.intents.is_empty());
    }
}

/// A read projection; owner/account scoping is part of the opaque stable ID.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub priority: u8,
    pub project: String,
    pub owner: String,
    pub updated: String,
    pub description: String,
    pub source: Option<String>,
    pub evidence: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Changes whenever any retained content or freshness changes.
    pub revision: u64,
    pub items: Vec<Item>,
    pub fresh: bool,
    pub source: String,
    pub observed: String,
    pub notice: Option<String>,
}
impl Snapshot {
    /// Validate untrusted owner projections before retaining or rendering them.
    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > 100_000 {
            return Err("Work projection exceeds 100000 rows".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for item in &self.items {
            if item.id.is_empty()
                || !ids.insert(&item.id)
                || item.id.len() > 512
                || item.title.len() > 4096
                || item.description.len() > 64 * 1024
                || item.priority > 4
                || item.project.len() > 256
                || item.owner.len() > 256
                || item.updated.len() > 128
                || item.evidence.len() > 64
                || item.dependencies.len() > 256
                || item.source.as_ref().is_some_and(|s| s.len() > 4096)
            {
                return Err("Invalid or duplicate work projection identity/content".into());
            }
            if item
                .evidence
                .iter()
                .chain(&item.dependencies)
                .any(|s| s.len() > 4096)
            {
                return Err("Work projection reference exceeds limit".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    Active,
    All,
    Done,
}

/// Presentation state only. No claim, draft, task or business state is stored here.
#[derive(Default)]
pub struct Inbox {
    pub selected: Option<String>,
    pub query: String,
    pub filter: Filter,
    pub details_open: bool,
    scroll_to_selection: bool,
    focus_search: bool,
    view: ViewIndex,
}

#[derive(Default)]
struct ViewIndex {
    key: Option<(u64, usize, String, Filter)>,
    indices: Vec<usize>,
}

/// The host explicitly scopes keyboard ownership to this embedded pane.
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub keyboard_active: bool,
    /// The embedding receiver, using its own clock, has lost live observations.
    pub observation_expired: bool,
}

/// Intents require owner resolution/authorization by the embedding host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    OpenSource { item: String, reference: String },
}

#[derive(Default, Debug)]
pub struct Report {
    pub total: usize,
    pub matched: usize,
    pub rows_drawn: usize,
    pub row_rects: Vec<egui::Rect>,
    pub intents: Vec<Intent>,
}

impl Inbox {
    fn visible<'a>(&mut self, snapshot: &'a Snapshot) -> Vec<&'a Item> {
        let key = (
            snapshot.revision,
            snapshot.items.len(),
            self.query.clone(),
            self.filter,
        );
        if self.view.key.as_ref() == Some(&key) {
            return self
                .view
                .indices
                .iter()
                .map(|&index| &snapshot.items[index])
                .collect();
        }
        let query = self.query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        let mut indices: Vec<_> = snapshot
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                let status = match self.filter {
                    Filter::Active => i.status != Status::Done,
                    Filter::All => true,
                    Filter::Done => i.status == Status::Done,
                };
                status
                    && (words.is_empty() || {
                        let text =
                            format!("{} {} {} {}", i.title, i.project, i.owner, i.status.label())
                                .to_lowercase();
                        words.iter().all(|word| text.contains(word))
                    })
            })
            .map(|(index, _)| index)
            .collect();
        indices.sort_by(|&a, &b| {
            snapshot.items[a]
                .priority
                .cmp(&snapshot.items[b].priority)
                .then(snapshot.items[a].id.cmp(&snapshot.items[b].id))
        });
        self.view = ViewIndex {
            key: Some(key),
            indices,
        };
        self.view
            .indices
            .iter()
            .map(|&index| &snapshot.items[index])
            .collect()
    }

    fn reconcile(&mut self, items: &[&Item]) {
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| items.iter().any(|i| &i.id == id))
        {
            self.selected = items.first().map(|i| i.id.clone());
        }
    }

    fn navigate(&mut self, items: &[&Item], delta: isize) {
        if items.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| items.iter().position(|i| &i.id == id))
            .unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(items.len() - 1);
        self.selected = Some(items[next].id.clone());
        self.scroll_to_selection = true;
    }

    /// Draw within a host-owned Ui. The host must supply validated snapshots and
    /// apply Coherence appearance when theme changes, not on every row/frame.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        snapshot: &Snapshot,
        options: Options,
    ) -> Report {
        let fresh = snapshot.fresh && !options.observation_expired;
        let mut report = Report {
            total: snapshot.items.len(),
            ..Default::default()
        };
        let items = self.visible(snapshot);
        self.reconcile(&items);
        let search_id = ui.id().with("work-search");
        let editing = ui.memory(|m| m.has_focus(search_id));
        let focused = options.keyboard_active && ui.ctx().input(|i| i.focused);
        if focused && !editing {
            let down = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                    || i.consume_key(egui::Modifiers::NONE, Key::J)
            });
            let up = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                    || i.consume_key(egui::Modifiers::NONE, Key::K)
            });
            if down {
                self.navigate(&items, 1);
            }
            if up {
                self.navigate(&items, -1);
            }
            if ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, Key::ArrowRight)
            }) {
                self.details_open = true;
            }
            if ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Escape)
                    || i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft)
            }) {
                self.details_open = false;
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Slash)) {
                self.focus_search = true;
            }
        }
        if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
            self.focus_search = true;
        }

        let compact = ui.available_width() < 760.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Work inbox")
                    .size(tokens.metrics.title)
                    .strong(),
            );
            ui.add_space(tokens.metrics.space[3]);
            if !compact {
                self.filters(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.search(ui, tokens, search_id, 220.0)
                });
            }
        });
        if compact {
            ui.horizontal(|ui| self.filters(ui));
            self.search(ui, tokens, search_id, ui.available_width());
        }
        ui.add_space(tokens.metrics.space[2]);
        let items = self.visible(snapshot);
        self.reconcile(&items);
        report.matched = items.len();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} items", items.len()))
                    .small()
                    .weak(),
            );
            ui.label(
                RichText::new(if fresh { "Current" } else { "Stale" })
                    .small()
                    .color(if fresh { Tone::Success } else { Tone::Warning }.color(tokens)),
            );
            ui.label(RichText::new(&snapshot.source).small().weak());
        });
        if let Some(notice) = &snapshot.notice {
            ui.colored_label(Tone::Warning.color(tokens), notice);
        }
        if options.observation_expired {
            ui.colored_label(
                Tone::Warning.color(tokens),
                "Updates paused. Showing last observed work.",
            );
        }
        ui.add_space(tokens.metrics.space[2]);
        let wide = ui.available_width() >= 980.0;
        let pane_width = if wide {
            (ui.available_width() * 0.35).clamp(320.0, 520.0)
        } else {
            0.0
        };
        let list_width = if wide {
            ui.available_width() - pane_width - tokens.metrics.space[3]
        } else {
            ui.available_width()
        };
        let footer_height = tokens.metrics.control_height;
        let body_height = (ui.available_height() - footer_height).max(100.0);
        ui.horizontal_top(|ui| {
            if wide || !self.details_open {
                ui.allocate_ui_with_layout(
                    egui::vec2(list_width, body_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        self.list(ui, tokens, &items, &mut report);
                    },
                );
            }
            if wide {
                ui.add_space(tokens.metrics.space[2]);
                ui.separator();
                ui.add_space(tokens.metrics.space[2]);
            }
            if wide || self.details_open {
                let width = if wide {
                    pane_width
                } else {
                    ui.available_width()
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(width, body_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        if !wide && ui.button("‹ Back to work").clicked() {
                            self.details_open = false;
                        }
                        let selected = items.iter().find(|i| Some(&i.id) == self.selected.as_ref());
                        self.detail(ui, tokens, selected.copied(), &mut report);
                    },
                );
            }
        });
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new(format!(
                    "{}   ·   {}   ·   Read-only",
                    snapshot.source, snapshot.observed
                ))
                .size(tokens.metrics.small)
                .weak(),
            );
        });
        report
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        for (filter, label) in [
            (Filter::Active, "Active"),
            (Filter::All, "All work"),
            (Filter::Done, "Done"),
        ] {
            if ui.selectable_label(self.filter == filter, label).clicked() {
                self.filter = filter;
            }
        }
    }

    fn search(&mut self, ui: &mut egui::Ui, tokens: &Tokens, id: egui::Id, width: f32) {
        let response = ui.add_sized(
            [width, tokens.metrics.control_height],
            TextEdit::singleline(&mut self.query)
                .hint_text("Search work  /")
                .id(id)
                .char_limit(256),
        );
        if self.focus_search {
            response.request_focus();
            self.focus_search = false;
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, tokens: &Tokens, items: &[&Item], report: &mut Report) {
        if items.is_empty() {
            ui.add_space(tokens.metrics.space[4]);
            ui.label(if self.query.is_empty() {
                "No work in this view."
            } else {
                "No matching work. Try another search."
            });
            return;
        }
        let columns = Columns::new(
            ui.available_width() - ui.spacing().scroll.bar_width - tokens.metrics.space[2],
            tokens,
        );
        widgets::dense_header(
            ui,
            tokens,
            columns,
            ["Task", "Status", "P", "Project", "Owner", "Updated"],
        );
        let height = widgets::dense_row_height(tokens);
        let mut scroll = ScrollArea::vertical()
            .id_salt("work-list")
            .auto_shrink([false, false]);
        if self.scroll_to_selection {
            if let Some(index) = items
                .iter()
                .position(|i| Some(&i.id) == self.selected.as_ref())
            {
                scroll = scroll
                    .vertical_scroll_offset(index as f32 * (height + ui.spacing().item_spacing.y));
            }
            self.scroll_to_selection = false;
        }
        scroll.show_rows(ui, height, items.len(), |ui, range| {
            for index in range {
                let item = items[index];
                let priority = format!("{}", item.priority);
                let row = DenseRow {
                    id: &item.id,
                    title: &item.title,
                    status: item.status.label(),
                    tone: item.status.tone(),
                    priority: &priority,
                    category: &item.project,
                    owner: &item.owner,
                    updated: &item.updated,
                };
                let response = ui
                    .push_id(&item.id, |ui| {
                        widgets::dense_row(
                            ui,
                            tokens,
                            columns,
                            &row,
                            self.selected.as_deref() == Some(&item.id),
                        )
                    })
                    .inner;
                if response.clicked() {
                    self.selected = Some(item.id.clone());
                    self.details_open = true;
                }
                report.row_rects.push(response.rect);
                report.rows_drawn += 1;
            }
        });
    }

    fn detail(&self, ui: &mut egui::Ui, tokens: &Tokens, item: Option<&Item>, report: &mut Report) {
        let Some(item) = item else {
            ui.label("Select work to see its context.");
            return;
        };
        ScrollArea::vertical()
            .id_salt("work-context")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(RichText::new(&item.project).small().weak());
                ui.add_space(tokens.metrics.space[2]);
                ui.label(
                    RichText::new(&item.title)
                        .size(tokens.metrics.large)
                        .strong(),
                );
                ui.add_space(tokens.metrics.space[3]);
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(item.status.tone().color(tokens), item.status.label());
                    ui.label(format!("Priority {}", item.priority));
                    ui.label(&item.owner);
                });
                ui.add_space(tokens.metrics.space[3]);
                let (description, notes) = item
                    .description
                    .split_once('\n')
                    .unwrap_or((&item.description, ""));
                ui.label(if description.is_empty() {
                    "No description supplied by the owner."
                } else {
                    description
                });
                if !notes.trim().is_empty() {
                    ui.add_space(tokens.metrics.space[3]);
                    ui.collapsing("Activity and notes", |ui| {
                        ui.label(notes.trim());
                    });
                }
                if !item.dependencies.is_empty() {
                    ui.add_space(tokens.metrics.space[3]);
                    ui.strong("Dependencies");
                    for dependency in &item.dependencies {
                        ui.label(dependency);
                    }
                }
                if !item.evidence.is_empty() {
                    ui.add_space(tokens.metrics.space[3]);
                    ui.strong("Evidence");
                    for evidence in &item.evidence {
                        ui.label(evidence);
                    }
                }
                if let Some(source) = &item.source {
                    ui.add_space(tokens.metrics.space[3]);
                    ui.strong("Source");
                    ui.label(source);
                    if ui.button("Open source").clicked() {
                        report.intents.push(Intent::OpenSource {
                            item: item.id.clone(),
                            reference: source.clone(),
                        });
                    }
                }
                ui.add_space(tokens.metrics.space[4]);
                ui.label(RichText::new(&item.id).small().weak());
            });
    }
}
