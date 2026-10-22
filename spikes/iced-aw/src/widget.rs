//! A widget written in the shape `canvas-widgets` already uses.
//!
//! This is the finding that surprised me most, and it is already true in the
//! tree: every function in `crates/canvas-widgets/src/live.rs` is generic over
//! an **unconstrained** `Message` (`Element<'a, Message>`, `Message: 'a`). A
//! type parameter with no bounds cannot be constructed. So the crate has no way
//! to invent a message — the only messages it can emit are values the host
//! handed it at construction.
//!
//! That is a stronger guarantee than "the host validates what the widget sent",
//! because it is checked by the compiler rather than by a runtime branch
//! someone can forget to write. The `tests` at the bottom turn it into a
//! mechanical check.
//!
//! Runtime validation in `host.rs` is still needed, but for a different
//! attacker: the *host* builds messages in a loop over stream-derived rows, so
//! the host is the one that can be tricked into minting a message about the
//! wrong row. Handles plus a table lookup close that. The two layers cover
//! different holes; Canvas wants both.

use crate::palette::CoherenceLike;

use iced::widget::{button, column, container, row, text};
use iced::{Border, Color, Element, Length};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Primary,
    Danger,
    Neutral,
}

/// One action offered by a card. `message: None` renders the control disabled —
/// which is how a widget expresses "not available" without ever needing to
/// decide anything.
pub struct Action<'a, Message> {
    pub label: &'a str,
    pub message: Option<Message>,
    pub tone: Tone,
}

/// An approval card in plain iced. Compare with the `iced_aw::Card` version in
/// `main.rs`: this is ~15 lines longer and owes no third party a style
/// function.
pub fn approval_card<'a, Message>(
    title: &'a str,
    detail: &'a str,
    actions: Vec<Action<'a, Message>>,
    footer: Option<&'a str>,
    t: &CoherenceLike,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    let (surface, surface_muted, fg, muted, border, radius) = (
        t.surface,
        t.surface_muted,
        t.text,
        t.text_muted,
        t.border,
        t.radius_md,
    );

    let controls = row(actions
        .into_iter()
        .map(|action| action_button(action, t))
        .collect::<Vec<_>>())
    .spacing(8);

    let foot: Element<'a, Message> = match footer {
        Some(note) => text(note).size(12).color(muted).into(),
        None => controls.into(),
    };

    container(
        column![
            container(text(title).size(14))
                .width(Length::Fill)
                .padding([8, 12])
                .style(move |_| container::Style {
                    background: Some(surface_muted.into()),
                    text_color: Some(fg),
                    border: Border {
                        radius: radius.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            container(text(detail).size(12).color(muted)).padding([4, 12]),
            container(foot).padding([8, 12]),
        ]
        .spacing(2),
    )
    .max_width(340.0)
    .style(move |_| container::Style {
        background: Some(surface.into()),
        border: Border {
            color: border,
            width: 1.0,
            radius: radius.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn action_button<'a, Message>(
    action: Action<'a, Message>,
    t: &CoherenceLike,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    let (primary, danger, neutral, on_accent, fg, radius) = (
        t.primary,
        t.danger,
        t.surface_muted,
        t.background,
        t.text,
        t.radius_sm,
    );
    let tone = action.tone;

    let mut control = button(text(action.label).size(13))
        .padding([6, 14])
        .style(move |_, status| {
            let (base, label) = match tone {
                Tone::Primary => (primary, on_accent),
                Tone::Danger => (danger, on_accent),
                Tone::Neutral => (neutral, fg),
            };
            let background = match status {
                button::Status::Hovered | button::Status::Pressed => Color { a: 0.85, ..base },
                button::Status::Disabled => Color { a: 0.35, ..base },
                button::Status::Active => base,
            };
            button::Style {
                background: Some(background.into()),
                text_color: label,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        });

    // The whole of the widget's authority: it may pass along a message the host
    // gave it. There is no branch here that could construct one.
    if let Some(message) = action.message {
        control = control.on_press(message);
    }
    control.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Intent, RequestId, Verdict};
    use std::convert::Infallible;

    /// The mechanical check for "a widget does not perform its own actions".
    ///
    /// `Infallible` is uninhabited — no value of it exists anywhere in this
    /// program, and none can be created. This instantiation type-checks *only*
    /// because `approval_card` never constructs a `Message`. If someone later
    /// adds a concrete message inside the widget, this test stops compiling.
    ///
    /// The equivalent check for `canvas-widgets` is the same one line, and the
    /// crate passes it today.
    #[test]
    fn a_widget_cannot_mint_a_message() {
        let t = CoherenceLike::dark();
        let _: Element<'_, Infallible> = approval_card(
            "Merge rename/dock-to-canvas",
            "142 files",
            vec![Action {
                label: "Approve",
                message: None,
                tone: Tone::Primary,
            }],
            None,
            &t,
        );
    }

    /// And with a host-supplied message it works normally — the widget is a
    /// courier, not an actor.
    #[test]
    fn a_widget_carries_a_host_supplied_message() {
        let t = CoherenceLike::dark();
        let _: Element<'_, Intent> = approval_card(
            "Merge rename/dock-to-canvas",
            "142 files",
            vec![Action {
                label: "Approve",
                message: Some(Intent::Respond {
                    request: RequestId(1),
                    verdict: Verdict::Approve,
                }),
                tone: Tone::Primary,
            }],
            None,
            &t,
        );
    }
}
