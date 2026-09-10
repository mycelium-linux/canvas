//! Spike: does iced_aw give Canvas the interaction surface it is about to need?
//!
//! Two panes, same four interaction surfaces, built twice.
//!
//! * Left  — plain `iced` 0.14 only.
//! * Right — `iced_aw` 0.14.1 where it plausibly helps.
//!
//! Both panes emit the *same* `Message::Ask(Intent, Origin)` and are resolved by
//! the same [`host::Host`]. Neither pane can perform anything: see `host.rs`.
//!
//! A third control simulates a hostile SSE/Lua payload asking for the identical
//! `Intent` the approve button asks for, so the audit log shows the gate doing
//! the work rather than the widget.

mod host;
mod palette;

use canvas_core::{Accent, ThemeMode, ThemeSelection, VisualTheme};
use host::{Density, Denied, Host, Intent, Origin};
use iced::widget::{
    Space, button, column, container, pick_list, row, scrollable, text, text_input,
};
use iced::{Element, Fill, Length, Theme};

use iced_aw::menu::{Item, Menu};
use iced_aw::{DropDown, badge, card, drop_down, menu_bar};

const APPROVAL_ID: &str = "appr-7301";

pub fn main() -> iced::Result {
    iced::application(Spike::new, Spike::update, Spike::view)
        .title(|_: &Spike| "Canvas interaction spike — plain iced vs iced_aw".to_string())
        .theme(Spike::theme)
        // iced_aw ships its own icon font and will render tofu without this
        // line. Canvas would have to add it to its daemon builder too.
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    /// The only thing any widget in this app is allowed to emit.
    Ask(Intent, Origin),
    NoteChanged(String),
    ToggleDropDown,
    DismissDropDown,
    ToggleGate,
}

struct Spike {
    visual: VisualTheme,
    host: Host,
    note: String,
    density: Density,
    dropdown_open: bool,
    last_effect: Option<String>,
}

impl Spike {
    fn new() -> Self {
        Self {
            // The real Coherence-derived theme, not a mock palette.
            visual: ThemeSelection::coherence(ThemeMode::Dark, Accent::Brand).visual_theme(),
            host: Host::default(),
            note: String::new(),
            density: Density::Comfortable,
            dropdown_open: false,
            last_effect: None,
        }
    }

    /// Exactly what `crates/canvas/src/main.rs:4385` does today: project the
    /// 13-role VisualTheme down onto iced's 6-role Palette. Note what is lost —
    /// border, border_strong, surface, surface_muted, text_muted, info, and
    /// every metric. Anything iced_aw resolves for itself only sees this.
    fn theme(&self) -> Theme {
        Theme::custom(
            "Canvas · spike".to_string(),
            iced::theme::Palette {
                background: palette::color(self.visual.palette.background),
                text: palette::color(self.visual.palette.text),
                primary: palette::color(self.visual.palette.primary),
                success: palette::color(self.visual.palette.success),
                danger: palette::color(self.visual.palette.danger),
                warning: palette::color(self.visual.palette.warning),
            },
        )
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Ask(intent, origin) => {
                // Every interaction in the app funnels through this arm. There
                // is no other path from a widget to an effect.
                match self.host.authorize(intent.clone(), origin) {
                    Ok(action) => {
                        if let Intent::SetDensity(d) = intent {
                            self.density = d;
                        }
                        if matches!(intent, Intent::SaveNote { .. }) {
                            self.note.clear();
                        }
                        self.last_effect = Some(self.host.perform(action));
                    }
                    Err(denied) => {
                        // Fail loud: the denial is surfaced, not swallowed.
                        self.last_effect = Some(format!("DENIED — {denied}"));
                    }
                }
                self.dropdown_open = false;
            }
            Message::NoteChanged(v) => self.note = v,
            Message::ToggleDropDown => self.dropdown_open = !self.dropdown_open,
            Message::DismissDropDown => self.dropdown_open = false,
            Message::ToggleGate => {
                self.host.gate_open = !self.host.gate_open;
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let v = &self.visual;
        let m = v.metrics;
        let gap = if self.density == Density::Compact {
            m.space_2
        } else {
            m.space_3
        };

        let header = column![
            text("Canvas interaction spike").size(m.title).color(palette::color(v.palette.text)),
            text("Same four surfaces, built twice. Every press is an Intent the host adjudicates.")
                .size(m.small)
                .color(palette::color(v.palette.text_muted)),
        ]
        .spacing(m.space_1);

        let panes = row![
            self.plain_pane(gap),
            self.aw_pane(gap),
        ]
        .spacing(m.space_4)
        .width(Fill);

        container(
            column![header, panes, self.audit_pane()]
                .spacing(m.space_4)
                .width(Fill),
        )
        .padding(m.space_4)
        .width(Fill)
        .height(Fill)
        .style(palette::screen(*v))
        .into()
    }

    // ---------------------------------------------------------------- plain

    /// Every surface below is plain iced 0.14 with a style closure that reads
    /// the full `VisualTheme`. No extra dependency.
    fn plain_pane(&self, gap: f32) -> Element<'_, Message> {
        let v = self.visual;
        let m = v.metrics;

        // 1. Approval card. `container` + `column` + `button`.
        let approval = container(
            column![
                row![
                    text("Routed approval").size(m.body).color(palette::color(v.palette.text)),
                    Space::new().width(Fill),
                    // A status pill — the thing iced_aw's `badge` exists for.
                    container(text(APPROVAL_ID).size(m.micro).color(palette::color(v.palette.background)))
                        .padding([2, 8])
                        .style(palette::pill(v, v.palette.warning)),
                ]
                .align_y(iced::Alignment::Center),
                text("agent `rss-sync` requests write access to config/projects.json")
                    .size(m.small)
                    .color(palette::color(v.palette.text_muted)),
                row![
                    button(text("Approve").size(m.small))
                        .padding([6, 14])
                        .style(palette::solid(v, v.palette.success))
                        .on_press(Message::Ask(
                            Intent::Approve { approval_id: APPROVAL_ID.into() },
                            Origin::LocalPointer,
                        )),
                    button(text("Deny").size(m.small))
                        .padding([6, 14])
                        .style(palette::solid(v, v.palette.danger))
                        .on_press(Message::Ask(
                            Intent::Deny { approval_id: APPROVAL_ID.into() },
                            Origin::LocalPointer,
                        )),
                ]
                .spacing(m.space_2),
            ]
            .spacing(m.space_2),
        )
        .padding(m.space_3)
        .style(palette::card(v));

        // 2. Settings. `pick_list` — already in iced.
        let settings = container(
            column![
                text("Settings").size(m.body).color(palette::color(v.palette.text)),
                pick_list(&Density::ALL[..], Some(self.density), |d| Message::Ask(
                    Intent::SetDensity(d),
                    Origin::LocalPointer
                ))
                .text_size(m.small)
                .padding([6, 10])
                .style(palette::picklist(v)),
            ]
            .spacing(m.space_2),
        )
        .padding(m.space_3)
        .style(palette::card(v));

        // 3. Note. `text_input` with on_submit — already in iced.
        let note = container(
            column![
                text("Note").size(m.body).color(palette::color(v.palette.text)),
                text_input("write a note…", &self.note)
                    .size(m.small)
                    .padding([6, 10])
                    .style(palette::input(v))
                    .on_input(Message::NoteChanged)
                    .on_submit(Message::Ask(
                        Intent::SaveNote { body: self.note.clone() },
                        Origin::LocalPointer,
                    )),
            ]
            .spacing(m.space_2),
        )
        .padding(m.space_3)
        .style(palette::card(v));

        // 4. Click-to-open / click-to-jump. Plain `button` with a link style.
        let links = container(
            column![
                text("Feed & tiles").size(m.body).color(palette::color(v.palette.text)),
                button(text("Rust 1.94 released — blog.rust-lang.org").size(m.small))
                    .padding([4, 8])
                    .style(palette::ghost(v))
                    .on_press(Message::Ask(
                        Intent::OpenUrl { url: "https://blog.rust-lang.org".into() },
                        Origin::LocalPointer,
                    )),
                button(text("Studio  →  jump to workspace").size(m.small))
                    .padding([4, 8])
                    .style(palette::ghost(v))
                    .on_press(Message::Ask(
                        Intent::JumpToProject { project_id: "Studio".into() },
                        Origin::LocalPointer,
                    )),
            ]
            .spacing(m.space_1),
        )
        .padding(m.space_3)
        .style(palette::card(v));

        column![
            text("PLAIN ICED 0.14").size(m.micro).color(palette::color(v.palette.info)),
            approval,
            settings,
            note,
            links,
        ]
        .spacing(gap)
        .width(Fill)
        .into()
    }

    // ------------------------------------------------------------- iced_aw

    fn aw_pane(&self, gap: f32) -> Element<'_, Message> {
        let v = self.visual;
        let m = v.metrics;

        // 1. `iced_aw::card`. Same three buttons in `.foot()`.
        //    Left as `style::card::primary` on purpose so the screenshot shows
        //    what iced_aw's own styling does with Canvas's palette: the head
        //    goes DODGER BLUE (a hardcoded `colors::PRIMARY`), while background
        //    and text do follow the supplied palette.
        let aw_card = card(
            text("Routed approval").size(m.body),
            column![
                text("agent `rss-sync` requests write access to config/projects.json")
                    .size(m.small),
                // `badge` — the one plain iced has no direct equivalent for.
                badge(text(APPROVAL_ID).size(m.micro)).style(iced_aw::style::badge::warning),
            ]
            .spacing(m.space_2),
        )
        .foot(
            row![
                button(text("Approve").size(m.small))
                    .padding([6, 14])
                    .style(palette::solid(v, v.palette.success))
                    .on_press(Message::Ask(
                        Intent::Approve { approval_id: APPROVAL_ID.into() },
                        Origin::LocalPointer,
                    )),
                button(text("Deny").size(m.small))
                    .padding([6, 14])
                    .style(palette::solid(v, v.palette.danger))
                    .on_press(Message::Ask(
                        Intent::Deny { approval_id: APPROVAL_ID.into() },
                        Origin::LocalPointer,
                    )),
            ]
            .spacing(m.space_2),
        )
        .style(iced_aw::style::card::primary);

        // The same card again, this time with a closure that resolves every
        // field from the VisualTheme. This is the escape hatch, and it is also
        // the whole cost: you write the styling yourself anyway.
        let aw_card_themed = card(
            text("Routed approval (themed closure)").size(m.body),
            text("every field resolved from canvas-core VisualTheme").size(m.small),
        )
        .style(palette::aw_card(v));

        // 2. `iced_aw::menu_bar` — a real dropdown menu, which plain iced lacks.
        let menu_tpl = |items| Menu::new(items).width(190.0).offset(4.0).spacing(2.0);
        let entry = |label: &'static str, intent: Intent| {
            Item::new(
                button(text(label).size(m.small))
                    .width(Fill)
                    .padding([5, 10])
                    .style(palette::ghost(v))
                    .on_press(Message::Ask(intent, Origin::LocalPointer)),
            )
        };
        let settings_menu = menu_bar!((
            button(text("Settings ▾").size(m.small))
                .padding([6, 12])
                .style(palette::ghost(v)),
            menu_tpl(vec![
                entry("Comfortable", Intent::SetDensity(Density::Comfortable)),
                entry("Compact", Intent::SetDensity(Density::Compact)),
                entry(
                    "Open Coherence docs",
                    Intent::OpenUrl { url: "https://fpl.dev/coherence".into() }
                ),
            ])
        ))
        .style(palette::aw_menu(v));

        // 3. `iced_aw::DropDown` — an anchored overlay, also absent from iced.
        let dropdown = DropDown::new(
            button(text("Project tile ▾").size(m.small))
                .padding([6, 12])
                .style(palette::ghost(v))
                .on_press(Message::ToggleDropDown),
            container(
                column![
                    button(text("Dock").size(m.small))
                        .width(Fill)
                        .padding([5, 10])
                        .style(palette::ghost(v))
                        .on_press(Message::Ask(
                            Intent::JumpToProject { project_id: "Dock".into() },
                            Origin::LocalPointer,
                        )),
                    button(text("Studio").size(m.small))
                        .width(Fill)
                        .padding([5, 10])
                        .style(palette::ghost(v))
                        .on_press(Message::Ask(
                            Intent::JumpToProject { project_id: "Studio".into() },
                            Origin::LocalPointer,
                        )),
                ]
                .spacing(2),
            )
            .width(160)
            .padding(4)
            .style(palette::card(v)),
            self.dropdown_open,
        )
        .on_dismiss(Message::DismissDropDown)
        .alignment(drop_down::Alignment::Bottom);

        let controls = container(
            column![
                text("Overlays iced does not ship").size(m.body).color(palette::color(v.palette.text)),
                row![settings_menu, dropdown].spacing(m.space_2),
            ]
            .spacing(m.space_2),
        )
        .padding(m.space_3)
        .style(palette::card(v));

        column![
            text("ICED_AW 0.14.1").size(m.micro).color(palette::color(v.palette.info)),
            aw_card,
            aw_card_themed,
            controls,
        ]
        .spacing(gap)
        .width(Fill)
        .into()
    }

    // ----------------------------------------------------------- audit pane

    fn audit_pane(&self) -> Element<'_, Message> {
        let v = self.visual;
        let m = v.metrics;

        // The hostile-payload control. It asks for the byte-identical Intent
        // the Approve button asks for, differing only in Origin.
        let hostile = button(text("Simulate stream payload asking to Approve").size(m.small))
            .padding([6, 12])
            .style(palette::solid(v, v.palette.warning))
            .on_press(Message::Ask(
                Intent::Approve {
                    approval_id: APPROVAL_ID.into(),
                },
                Origin::Stream { adapter: "rss.lua" },
            ));

        let gate = button(
            text(if self.host.gate_open {
                "Gate: OPEN (click to revoke)"
            } else {
                "Gate: REVOKED (click to restore)"
            })
            .size(m.small),
        )
        .padding([6, 12])
        .style(palette::ghost(v))
        .on_press(Message::ToggleGate);

        let rows = self.host.audit.iter().rev().take(10).map(|e| {
            let (mark, tint) = match e.verdict {
                Ok(()) => ("PASS", v.palette.success),
                Err(Denied::StreamOriginated) => ("BLOCK", v.palette.danger),
                Err(_) => ("DENY", v.palette.warning),
            };
            let origin = match &e.origin {
                Origin::LocalPointer => "pointer".to_string(),
                Origin::Stream { adapter } => format!("stream:{adapter}"),
            };
            row![
                container(text(mark).size(m.micro).color(palette::color(v.palette.background)))
                    .padding([1, 6])
                    .style(palette::pill(v, tint)),
                text(origin).size(m.micro).color(palette::color(v.palette.text_muted)).width(110),
                text(e.intent.clone()).size(m.micro).color(palette::color(v.palette.text)),
            ]
            .spacing(m.space_2)
            .align_y(iced::Alignment::Center)
            .into()
        });

        container(
            column![
                row![
                    text("Host audit log").size(m.body).color(palette::color(v.palette.text)),
                    Space::new().width(Fill),
                    gate,
                    hostile,
                ]
                .spacing(m.space_2)
                .align_y(iced::Alignment::Center),
                text(
                    self.last_effect
                        .clone()
                        .unwrap_or_else(|| "no effect yet".into())
                )
                .size(m.small)
                .color(palette::color(v.palette.text_muted)),
                scrollable(column(rows).spacing(m.space_1))
                    .height(Length::Fixed(150.0)),
            ]
            .spacing(m.space_2),
        )
        .padding(m.space_3)
        .width(Fill)
        .style(palette::card(v))
        .into()
    }
}
