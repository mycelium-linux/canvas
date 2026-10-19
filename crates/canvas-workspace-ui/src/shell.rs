//! Shared mock workspace shell, independent of native/window/browser ownership.
use crate::{
    Inbox, Snapshot, demo,
    mail::{self, Filter},
};
use coherence_native::{Density, Mode, Tokens};
use egui::RichText;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Mail,
    Work,
    Calendar,
}
pub struct Shell {
    pub owner: demo::Owner,
    pub mail: mail::Mail,
    pub work: Inbox,
    pub calendar: canvas_calendar::Agenda,
    pub view: View,
    pub mode: Mode,
    work_data: Snapshot,
    calendar_data: canvas_calendar::Snapshot,
    applied: Option<Mode>,
    pub last_rows: usize,
}
impl Shell {
    pub fn demo() -> Result<Self, String> {
        Ok(Self {
            owner: demo::Owner::new(),
            mail: mail::Mail::default(),
            work: Inbox::default(),
            calendar: canvas_calendar::Agenda::default(),
            view: View::Mail,
            mode: Mode::Dark,
            work_data: demo::work(),
            calendar_data: demo::calendar()?,
            applied: None,
            last_rows: 0,
        })
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        let tokens = Tokens::new(self.mode, Density::Compact);
        for (key, view) in [
            (egui::Key::Num1, View::Mail),
            (egui::Key::Num2, View::Work),
            (egui::Key::Num3, View::Calendar),
        ] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
                self.view = view;
            }
        }
        if self.applied != Some(self.mode)
            && let Ok(mut style) = coherence_native::egui::style(&tokens)
        {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            ctx.set_style(style);
            self.applied = Some(self.mode);
        }
        egui::TopBottomPanel::top("workspace-top")
            .exact_height(43.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("Canvas").strong());
                    ui.label(RichText::new("/ Workspace").weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (mode, label) in [
                            (Mode::Sepia, "Sepia"),
                            (Mode::Light, "Light"),
                            (Mode::Dark, "Dark"),
                        ] {
                            if ui.selectable_label(self.mode == mode, label).clicked() {
                                self.mode = mode;
                            }
                        }
                        ui.label(
                            RichText::new("Demo data")
                                .small()
                                .color(mail::account_color(&tokens, demo::ACCOUNTS[2])),
                        );
                    });
                });
            });
        let narrow = ctx.content_rect().width() < 1000.0;
        egui::SidePanel::left("workspace-nav")
            .resizable(false)
            .exact_width(if narrow { 164.0 } else { 220.0 })
            .show(ctx, |ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("Avery's workspace").strong());
                ui.add_space(12.0);
                if ui
                    .add_sized(
                        [ui.available_width(), tokens.metrics.control_height],
                        egui::Button::new("New draft").shortcut_text("C"),
                    )
                    .clicked()
                {
                    self.view = View::Mail;
                    self.mail.compose_open = true;
                }
                ui.add_space(18.0);
                for (view, label, shortcut) in [
                    (View::Mail, "Mail", "⌘1"),
                    (View::Work, "Work inbox", "⌘2"),
                    (View::Calendar, "Calendar", "⌘3"),
                ] {
                    if ui
                        .add_sized(
                            [ui.available_width(), 30.0],
                            egui::Button::selectable(self.view == view, label)
                                .shortcut_text(shortcut),
                        )
                        .clicked()
                    {
                        self.view = view;
                    }
                }
                ui.add_space(22.0);
                ui.label(RichText::new("Mail splits").small().weak());
                for filter in [
                    Filter::Inbox,
                    Filter::Unread,
                    Filter::Starred,
                    Filter::Snoozed,
                    Filter::Drafts,
                    Filter::All,
                ] {
                    let mut probe = mail::Mail::default();
                    probe.filter = filter;
                    probe.account = self.mail.account.clone();
                    let count = probe.visible(&self.owner.threads, &self.owner.drafts).len();
                    if ui
                        .add_sized(
                            [ui.available_width(), tokens.metrics.control_height],
                            egui::Button::selectable(
                                self.view == View::Mail && self.mail.filter == filter,
                                filter.label(),
                            )
                            .right_text(count.to_string())
                            .truncate(),
                        )
                        .clicked()
                    {
                        self.view = View::Mail;
                        self.mail.filter = filter;
                        self.mail.reader_open = false;
                    }
                }
                ui.add_space(24.0);
                ui.label(RichText::new("Accounts").small().weak());
                if ui
                    .selectable_label(self.mail.account.is_none(), "All accounts")
                    .clicked()
                {
                    self.mail.account = None;
                    self.mail.reader_open = false;
                }
                for account in demo::ACCOUNTS {
                    if ui
                        .selectable_label(
                            self.mail.account.as_deref() == Some(account),
                            RichText::new(account)
                                .size(12.0)
                                .color(mail::account_color(&tokens, account)),
                        )
                        .clicked()
                    {
                        self.mail.account = Some(account.into());
                        self.view = View::Mail;
                        self.mail.reader_open = false;
                    }
                }
                ui.add_space(25.0);
                egui::CollapsingHeader::new("Workspace context")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Coding").small().strong());
                        ui.label(RichText::new("Edgerunner + Alfalfa").small().weak());
                        ui.add_space(8.0);
                        ui.label(RichText::new("Messaging").small().strong());
                        ui.label(RichText::new("ohmOS").small().weak());
                    });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(
                        RichText::new("Synthetic fixtures · no live actions")
                            .small()
                            .weak(),
                    );
                    if ui.small_button("Reset demo").clicked() {
                        self.owner = demo::Owner::new();
                        self.mail = mail::Mail::default();
                    }
                });
            });
        egui::TopBottomPanel::bottom("workspace-footer").exact_height(28.0).show(ctx,|ui|{ui.horizontal(|ui|{
            ui.label(RichText::new("J / K  Navigate     E  Archive     Z  Undo     R  Reply     /  Search     ⌘K  Commands").small().weak());
        });});
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(16.0))
            .show(ctx, |ui| match self.view {
                View::Mail => {
                    let report =
                        self.mail
                            .show(ui, &tokens, &self.owner.threads, &self.owner.drafts, true);
                    self.last_rows = report.drawn;
                    for intent in report.intents {
                        self.mail.notice = Some(match self.owner.apply(intent) {
                            Ok(message) => message,
                            Err(error) => format!("Demo action failed: {error}"),
                        });
                    }
                }
                View::Work => {
                    self.work.show(
                        ui,
                        &tokens,
                        &self.work_data,
                        crate::Options {
                            keyboard_active: true,
                            observation_expired: false,
                        },
                    );
                }
                View::Calendar => {
                    let colors = demo::ACCOUNTS.map(|a| mail::account_color(&tokens, a));
                    self.calendar.show(ui, &self.calendar_data, &colors);
                }
            });
    }
}
