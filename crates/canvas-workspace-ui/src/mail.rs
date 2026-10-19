//! Mail presentation and typed owner intents. This module never talks to a provider.
use coherence_native::{Tokens, egui::widgets::Tone};
use egui::{Key, RichText, ScrollArea, Sense};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyId {
    pub account: String,
    pub thread: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub sender: String,
    pub address: String,
    pub to: String,
    pub time: String,
    pub body: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: KeyId,
    pub sender: String,
    pub subject: String,
    pub snippet: String,
    pub time: String,
    pub unread: bool,
    pub starred: bool,
    pub archived: bool,
    pub snoozed: bool,
    pub label: String,
    pub messages: Vec<Message>,
    pub attachments: Vec<Attachment>,
    pub context: Vec<String>,
}
pub fn validate(threads: &[Thread]) -> Result<(), String> {
    if threads.len() > 100_000 {
        return Err("Mailbox projection exceeds bound".into());
    }
    let mut ids = HashSet::new();
    for t in threads {
        if t.id.account.is_empty()
            || t.id.thread.is_empty()
            || !ids.insert(&t.id)
            || t.id.account.len() > 320
            || t.id.thread.len() > 512
            || t.subject.len() > 4096
            || t.snippet.len() > 4096
            || t.messages.len() > 128
            || t.attachments.len() > 128
            || t.messages.iter().any(|m| m.body.len() > 256 * 1024)
        {
            return Err("Invalid mailbox projection".into());
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    Inbox,
    Unread,
    Starred,
    Snoozed,
    Drafts,
    All,
}
impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Unread => "Unread",
            Self::Starred => "Starred",
            Self::Snoozed => "Snoozed",
            Self::Drafts => "Draft previews",
            Self::All => "All mail",
        }
    }
}
#[derive(Clone, Debug)]
pub enum Intent {
    ComposePreview {
        account: String,
        to: String,
        subject: String,
        body: String,
    },
    Archive(KeyId),
    Star(KeyId),
    Unread(KeyId),
    Snooze(KeyId),
    Open(KeyId),
    SavePreview {
        key: KeyId,
        body: String,
    },
    Undo,
}
#[derive(Default)]
pub struct Mail {
    pub compose_open: bool,
    compose_account: String,
    compose_to: String,
    compose_subject: String,
    compose_body: String,
    pub selected: Option<KeyId>,
    pub account: Option<String>,
    pub filter: Filter,
    pub query: String,
    pub reader_open: bool,
    pub palette: bool,
    pub help: bool,
    pub notice: Option<String>,
    editors: HashMap<KeyId, String>,
    focus_reply: bool,
    focus_search: bool,
    scroll_selected: bool,
}
#[derive(Default)]
pub struct Report {
    pub matched: usize,
    pub drawn: usize,
    pub intents: Vec<Intent>,
    pub rows: Vec<(KeyId, egui::Rect)>,
}
pub fn account_color(tokens: &Tokens, account: &str) -> egui::Color32 {
    let tone = if account.ends_with("@fpl.dev") {
        Tone::Info
    } else if account.ends_with("@wagar.cc") {
        Tone::Success
    } else {
        Tone::Warning
    };
    tone.color(tokens)
}
impl Mail {
    fn matches(&self, t: &Thread, drafts: &HashMap<KeyId, String>) -> bool {
        self.account.as_ref().is_none_or(|a| a == &t.id.account)
            && match self.filter {
                Filter::Inbox => !t.archived && !t.snoozed,
                Filter::Unread => t.unread && !t.archived && !t.snoozed,
                Filter::Starred => t.starred,
                Filter::Snoozed => t.snoozed,
                Filter::Drafts => drafts.contains_key(&t.id),
                Filter::All => true,
            }
            && (self.query.is_empty()
                || format!("{} {} {} {}", t.sender, t.subject, t.snippet, t.id.account)
                    .to_lowercase()
                    .contains(&self.query.to_lowercase()))
    }
    pub fn visible<'a>(
        &self,
        threads: &'a [Thread],
        drafts: &HashMap<KeyId, String>,
    ) -> Vec<&'a Thread> {
        threads.iter().filter(|t| self.matches(t, drafts)).collect()
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        threads: &[Thread],
        drafts: &HashMap<KeyId, String>,
        keyboard_active: bool,
    ) -> Report {
        let mut report = Report::default();
        let visible = self.visible(threads, drafts);
        report.matched = visible.len();
        if self
            .selected
            .as_ref()
            .is_none_or(|id| !visible.iter().any(|t| &t.id == id))
        {
            self.selected = visible.first().map(|t| t.id.clone());
        }
        let ctx = ui.ctx().clone();
        if keyboard_active && ctx.input(|i| i.focused) {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
                self.palette = !self.palette;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.palette = false;
                self.help = false;
                self.reader_open = false;
            }
            if !ctx.wants_keyboard_input() && !self.palette && !self.help && !self.compose_open {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::C)) {
                    self.compose_open = true;
                }
                let delta = if ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, Key::J)
                        || i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                }) {
                    1
                } else if ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, Key::K)
                        || i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                }) {
                    -1
                } else {
                    0
                };
                if delta != 0 && !visible.is_empty() {
                    let index = visible
                        .iter()
                        .position(|t| Some(&t.id) == self.selected.as_ref())
                        .unwrap_or(0);
                    let next =
                        (index as isize + delta).clamp(0, visible.len() as isize - 1) as usize;
                    self.selected = Some(visible[next].id.clone());
                    self.scroll_selected = true;
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Slash)) {
                    self.focus_search = true;
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    self.reader_open = true;
                }
                if let Some(key) = self.selected.clone() {
                    for (button, intent) in [
                        (Key::E, Intent::Archive(key.clone())),
                        (Key::S, Intent::Star(key.clone())),
                        (Key::U, Intent::Unread(key.clone())),
                        (Key::H, Intent::Snooze(key.clone())),
                    ] {
                        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, button)) {
                            report.intents.push(intent);
                        }
                    }
                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::R)) {
                        self.reader_open = true;
                        self.focus_reply = true;
                    }
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Z)) {
                    report.intents.push(Intent::Undo);
                }
            }
        }
        ui.horizontal(|ui| {
            ui.heading(self.filter.label());
            ui.label(RichText::new(visible.len().to_string()).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Commands  ⌘K").clicked() {
                    self.palette = true;
                }
                if ui.button("Shortcuts").clicked() {
                    self.help = !self.help;
                }
            });
        });
        ui.add_space(4.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id_salt("mail-search")
                .desired_width(f32::INFINITY)
                .hint_text("Search mail, people or account    /"),
        );
        if self.focus_search {
            search.request_focus();
            self.focus_search = false;
        }
        if let Some(notice) = &self.notice {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(notice)
                        .small()
                        .color(Tone::Info.color(tokens)),
                );
                if ui.small_button("Undo").clicked() {
                    report.intents.push(Intent::Undo);
                }
            });
        }
        ui.add_space(8.0);
        let wide = ui.available_width() >= 960.0;
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| threads.iter().find(|t| &t.id == id))
            .cloned();
        if wide {
            egui::SidePanel::right("mail-reader")
                .resizable(true)
                .default_width(610.0)
                .min_width(380.0)
                .max_width((ui.available_width() - 350.0).max(380.0))
                .show_inside(ui, |ui| {
                    if let Some(t) = &selected {
                        self.reader(ui, t, tokens, drafts, &mut report.intents);
                    } else {
                        ui.label("Select a conversation");
                    }
                });
        } else if self.reader_open {
            if ui.button("Back to inbox  Esc").clicked() {
                self.reader_open = false;
            }
            if let Some(t) = &selected {
                self.reader(ui, t, tokens, drafts, &mut report.intents);
            }
        }
        if wide || !self.reader_open {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(self.account.as_deref().unwrap_or("All accounts"))
                        .small()
                        .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Newest first").small().weak());
                });
            });
            ui.separator();
            let height = tokens.metrics.body + tokens.metrics.small + tokens.metrics.space[2];
            ScrollArea::vertical().id_salt("mail-list").show_rows(
                ui,
                height,
                visible.len(),
                |ui, range| {
                    for index in range {
                        let thread = visible[index];
                        report.drawn += 1;
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), height),
                            Sense::click(),
                        );
                        report.rows.push((thread.id.clone(), rect));
                        let selected = self.selected.as_ref() == Some(&thread.id);
                        if selected || response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink(1.0),
                                4.0,
                                if selected {
                                    ui.visuals().selection.bg_fill
                                } else {
                                    ui.visuals().widgets.hovered.bg_fill
                                },
                            );
                        }
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                rect.left_top() + egui::vec2(2.0, 10.0),
                                egui::vec2(3.0, height - 20.0),
                            ),
                            1.0,
                            account_color(tokens, &thread.id.account),
                        );
                        let mut row = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect.shrink2(egui::vec2(12.0, 3.0)))
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        row.spacing_mut().item_spacing.y = 2.0;
                        row.spacing_mut().interact_size.y = tokens.metrics.small + 2.0;
                        row.horizontal(|ui| {
                            let width = (ui.available_width() - 165.0).max(20.0);
                            let sender = if thread.unread {
                                RichText::new(&thread.sender)
                                    .size(tokens.metrics.small)
                                    .strong()
                            } else {
                                RichText::new(&thread.sender).size(tokens.metrics.small)
                            };
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 14.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(width);
                                    ui.add(egui::Label::new(sender).truncate());
                                },
                            );
                            let account = if thread.id.account.ends_with("@fpl.dev") {
                                "FPL"
                            } else if thread.id.account.ends_with("@wagar.cc") {
                                "Personal"
                            } else {
                                "Subs"
                            };
                            ui.add_sized(
                                [68.0, 16.0],
                                egui::Label::new(
                                    RichText::new(account)
                                        .small()
                                        .color(account_color(tokens, &thread.id.account)),
                                )
                                .truncate(),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    if thread.starred { "★" } else { "" },
                                    thread.time
                                ))
                                .small()
                                .weak(),
                            );
                        });
                        row.add(
                            egui::Label::new(if thread.unread {
                                RichText::new(&thread.subject).strong()
                            } else {
                                RichText::new(&thread.subject)
                            })
                            .truncate(),
                        );
                        if response.clicked() {
                            self.selected = Some(thread.id.clone());
                            self.reader_open = true;
                            report.intents.push(Intent::Open(thread.id.clone()));
                        }
                        if selected && self.scroll_selected {
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                            self.scroll_selected = false;
                        }
                        response.on_hover_text(format!(
                            "{}\n{}\n{}",
                            thread.id.account, thread.subject, thread.snippet
                        ));
                    }
                },
            );
            if visible.is_empty() {
                ui.add_space(30.0);
                ui.heading("You're caught up");
                ui.label("Choose another split or clear the search.");
            }
        }
        if self.palette {
            egui::Window::new("Commands")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 100.0])
                .show(&ctx, |ui| {
                    ui.label(
                        RichText::new("Actions apply to this demo conversation")
                            .small()
                            .weak(),
                    );
                    if let Some(key) = self.selected.clone() {
                        for (label, intent) in [
                            (
                                "Archive                                      E",
                                Intent::Archive(key.clone()),
                            ),
                            (
                                "Star / unstar                                S",
                                Intent::Star(key.clone()),
                            ),
                            (
                                "Mark unread                                  U",
                                Intent::Unread(key.clone()),
                            ),
                            (
                                "Snooze until tomorrow                        H",
                                Intent::Snooze(key.clone()),
                            ),
                        ] {
                            if ui
                                .add_sized([360.0, 32.0], egui::Button::new(label))
                                .clicked()
                            {
                                report.intents.push(intent);
                                self.palette = false;
                            }
                        }
                        if ui
                            .add_sized(
                                [360.0, 32.0],
                                egui::Button::new("Reply preview                                R"),
                            )
                            .clicked()
                        {
                            self.focus_reply = true;
                            self.reader_open = true;
                            self.palette = false;
                        }
                    }
                    if ui.button("Close  Esc").clicked() {
                        self.palette = false;
                    }
                });
        }
        if self.help {
            egui::Window::new("Mail shortcuts")
                .collapsible(false)
                .open(&mut self.help)
                .show(&ctx, |ui| {
                    for (key, action) in [
                        ("J / K", "Next / previous conversation"),
                        ("Enter / Esc", "Open / back"),
                        ("E / Z", "Archive / undo"),
                        ("S / U / H", "Star / unread / snooze"),
                        ("R", "Reply preview"),
                        ("/", "Search"),
                        ("⌘K", "Commands"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.monospace(key);
                            ui.label(action);
                        });
                    }
                });
        }
        if self.compose_open {
            if self.compose_account.is_empty() {
                self.compose_account = self
                    .account
                    .clone()
                    .or_else(|| self.selected.as_ref().map(|k| k.account.clone()))
                    .unwrap_or_default();
            }
            let mut open = true;
            let mut saved = false;
            egui::Window::new("New draft preview")
                .open(&mut open)
                .collapsible(false)
                .default_width(640.0)
                .show(&ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("From");
                        egui::ComboBox::from_id_salt("compose-from")
                            .selected_text(&self.compose_account)
                            .show_ui(ui, |ui| {
                                let mut accounts: Vec<_> =
                                    threads.iter().map(|t| t.id.account.as_str()).collect();
                                accounts.sort_unstable();
                                accounts.dedup();
                                for account in accounts {
                                    ui.selectable_value(
                                        &mut self.compose_account,
                                        account.to_owned(),
                                        account,
                                    );
                                }
                            });
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.compose_to)
                            .desired_width(f32::INFINITY)
                            .hint_text("To"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.compose_subject)
                            .desired_width(f32::INFINITY)
                            .hint_text("Subject"),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut self.compose_body)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .hint_text("Write a draft preview…"),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.compose_body.trim().is_empty(),
                                egui::Button::new("Save draft preview"),
                            )
                            .clicked()
                        {
                            report.intents.push(Intent::ComposePreview {
                                account: self.compose_account.clone(),
                                to: self.compose_to.clone(),
                                subject: self.compose_subject.clone(),
                                body: self.compose_body.clone(),
                            });
                            saved = true;
                        }
                        ui.label(
                            RichText::new("Local demo only · nothing is sent")
                                .small()
                                .weak(),
                        );
                    });
                });
            self.compose_open = open && !saved;
            if saved {
                self.compose_body.clear();
                self.compose_subject.clear();
                self.compose_to.clear();
            }
        }
        report
    }
    fn reader(
        &mut self,
        ui: &mut egui::Ui,
        t: &Thread,
        tokens: &Tokens,
        drafts: &HashMap<KeyId, String>,
        intents: &mut Vec<Intent>,
    ) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Archive  E").clicked() {
                intents.push(Intent::Archive(t.id.clone()));
            }
            if ui
                .button(if t.starred { "Unstar  S" } else { "Star  S" })
                .clicked()
            {
                intents.push(Intent::Star(t.id.clone()));
            }
            if ui.button("Unread  U").clicked() {
                intents.push(Intent::Unread(t.id.clone()));
            }
            if ui.button("Snooze  H").clicked() {
                intents.push(Intent::Snooze(t.id.clone()));
            }
        });
        ui.add_space(18.0);
        ui.heading(&t.subject);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(account_color(tokens, &t.id.account), &t.id.account);
            ui.label(RichText::new(&t.label).small().weak());
        });
        ui.add_space(10.0);
        ScrollArea::vertical()
            .id_salt(("conversation", &t.id.account, &t.id.thread))
            .show(ui, |ui| {
                for (index, m) in t.messages.iter().enumerate() {
                    if index + 1 < t.messages.len() {
                        egui::CollapsingHeader::new(format!("{} · {}", m.sender, m.time))
                            .id_salt((&t.id.account, &t.id.thread, index))
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(&m.body).wrap());
                            });
                        continue;
                    }
                    if index > 0 {
                        ui.add_space(18.0);
                        ui.separator();
                        ui.add_space(12.0);
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&m.sender).strong());
                        ui.label(RichText::new(&m.time).small().weak());
                    });
                    ui.label(
                        RichText::new(format!(
                            "{}  to  {}",
                            m.address,
                            if m.to.is_empty() {
                                "(recipient not set)"
                            } else {
                                &m.to
                            }
                        ))
                        .small()
                        .weak(),
                    );
                    ui.add_space(12.0);
                    ui.add(egui::Label::new(&m.body).wrap());
                }
                if !t.attachments.is_empty() {
                    ui.add_space(16.0);
                    ui.horizontal_wrapped(|ui| {
                        for a in &t.attachments {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.label(&a.name);
                                ui.label(RichText::new(&a.detail).small().weak());
                            });
                        }
                    });
                }
                if !t.context.is_empty() {
                    ui.add_space(16.0);
                    egui::CollapsingHeader::new("Linked work")
                        .default_open(true)
                        .show(ui, |ui| {
                            for link in &t.context {
                                ui.label(RichText::new(link).color(Tone::Info.color(tokens)));
                            }
                        });
                }
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Reply").strong());
                    ui.label(
                        RichText::new(format!("from {}", t.id.account))
                            .small()
                            .color(account_color(tokens, &t.id.account)),
                    );
                });
                let body = self
                    .editors
                    .entry(t.id.clone())
                    .or_insert_with(|| drafts.get(&t.id).cloned().unwrap_or_default());
                let editor = ui.add(
                    egui::TextEdit::multiline(body)
                        .id_salt(("reply", &t.id.account, &t.id.thread))
                        .desired_rows(5)
                        .desired_width(f32::INFINITY)
                        .hint_text("Write a reply preview…"),
                );
                if self.focus_reply {
                    editor.request_focus();
                    editor.scroll_to_me(Some(egui::Align::Center));
                    self.focus_reply = false;
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !body.trim().is_empty(),
                            egui::Button::new("Save draft preview"),
                        )
                        .clicked()
                    {
                        intents.push(Intent::SavePreview {
                            key: t.id.clone(),
                            body: body.clone(),
                        });
                    }
                    ui.label(
                        RichText::new("Local demo only · nothing is sent")
                            .small()
                            .weak(),
                    );
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coherence_native::{Density, Mode};
    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn frame(
        ctx: &egui::Context,
        mail: &mut Mail,
        threads: &[Thread],
        width: f32,
        events: Vec<egui::Event>,
        active: bool,
    ) -> Report {
        let mut report = Report::default();
        let tokens = Tokens::new(Mode::Dark, Density::Compact);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    report = mail.show(ui, &tokens, threads, &HashMap::new(), active);
                });
            },
        );
        report
    }
    #[test]
    fn keyboard_triage_is_scoped_and_disabled_while_editing() {
        let ctx = egui::Context::default();
        let owner = crate::demo::Owner::new();
        let mut mail = Mail::default();
        frame(&ctx, &mut mail, &owner.threads, 1500.0, vec![], true);
        frame(
            &ctx,
            &mut mail,
            &owner.threads,
            1500.0,
            vec![key(Key::J)],
            true,
        );
        assert_eq!(mail.selected, Some(owner.threads[1].id.clone()));
        let r = frame(
            &ctx,
            &mut mail,
            &owner.threads,
            1500.0,
            vec![key(Key::E)],
            true,
        );
        assert!(matches!(&r.intents[..],[Intent::Archive(k)] if k==&owner.threads[1].id));
        assert!(
            frame(
                &ctx,
                &mut mail,
                &owner.threads,
                1500.0,
                vec![key(Key::E)],
                false
            )
            .intents
            .is_empty()
        );
        frame(
            &ctx,
            &mut mail,
            &owner.threads,
            1500.0,
            vec![key(Key::R)],
            true,
        );
        assert!(
            frame(
                &ctx,
                &mut mail,
                &owner.threads,
                1500.0,
                vec![key(Key::E)],
                true
            )
            .intents
            .is_empty()
        );
    }
    #[test]
    fn narrow_reader_has_a_return_path_and_search_does_not_leak_accounts() {
        let ctx = egui::Context::default();
        let owner = crate::demo::Owner::new();
        let mut mail = Mail {
            account: Some(crate::demo::ACCOUNTS[1].into()),
            ..Default::default()
        };
        let r = frame(&ctx, &mut mail, &owner.threads, 700.0, vec![], true);
        assert!(r.drawn > 0);
        assert!(
            r.rows
                .iter()
                .all(|(key, _)| key.account == crate::demo::ACCOUNTS[1])
        );
        assert_eq!(
            frame(
                &ctx,
                &mut mail,
                &owner.threads,
                700.0,
                vec![key(Key::Enter)],
                true
            )
            .drawn,
            0
        );
        assert!(
            frame(
                &ctx,
                &mut mail,
                &owner.threads,
                700.0,
                vec![key(Key::Escape)],
                true
            )
            .drawn
                > 0
        );
        mail.query = "no such fixture message".into();
        assert_eq!(
            frame(&ctx, &mut mail, &owner.threads, 700.0, vec![], true).matched,
            0
        );
    }
    #[test]
    fn long_lists_are_virtualized_and_rows_do_not_overlap() {
        let ctx = egui::Context::default();
        let owner = crate::demo::Owner::new();
        let mut threads = Vec::new();
        for i in 0..10_000 {
            let mut t = owner.threads[i % owner.threads.len()].clone();
            t.id.thread = format!("synthetic-{i}");
            threads.push(t);
        }
        validate(&threads).unwrap();
        let mut mail = Mail::default();
        let r = frame(&ctx, &mut mail, &threads, 1600.0, vec![], true);
        assert_eq!(r.matched, 10_000);
        assert!(r.drawn > 5 && r.drawn < 80);
        for rows in r.rows.windows(2) {
            assert!(rows[0].1.bottom() <= rows[1].1.top() + 0.1);
        }
    }
}
