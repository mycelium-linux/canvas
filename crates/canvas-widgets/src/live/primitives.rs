//! Small semantic building blocks shared by widget compositions.
//!
//! These own typography, spacing, and theme-role selection. Domain widgets
//! supply meaning and data; they should not repeatedly spell out Canvas's
//! visual grammar with raw `row!`, `text`, and container styles.

use canvas_core::VisualTheme;
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Length};

pub(super) fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

/// A section heading with optional right-aligned context such as a unit or
/// time window.
pub(super) fn header<'a, Message: 'a>(
    title: impl Into<String>,
    context: Option<String>,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let context = context.filter(|value| !value.is_empty());
    let context: Element<'a, Message> = context.map_or_else(
        || space().width(Length::Shrink).into(),
        |context| {
            container(
                text(context)
                    .size(metrics.micro)
                    .color(color(palette.text_muted)),
            )
            .padding([metrics.space_1 * 0.5, metrics.space_2])
            .style(move |_| container::Style {
                background: Some(
                    Color {
                        a: 0.58,
                        ..color(palette.surface_muted)
                    }
                    .into(),
                ),
                border: iced::Border {
                    color: Color {
                        a: 0.55,
                        ..color(palette.border_strong)
                    },
                    width: 1.0,
                    radius: metrics.radius_sm.into(),
                },
                ..Default::default()
            })
            .into()
        },
    );
    row![
        text(title.into())
            .size(metrics.large)
            .color(color(palette.text)),
        space::horizontal(),
        context,
    ]
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

/// The recurring label/value/detail hierarchy used by stats, gauges, meters,
/// audio measurements, and transport telemetry.
pub(super) fn metric<'a, Message: 'a>(
    label: impl Into<String>,
    value: impl Into<String>,
    detail: Option<String>,
    value_color: Color,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let mut content = column![
        text(label.into())
            .size(metrics.small)
            .color(color(palette.text_muted)),
        text(value.into()).size(metrics.hero).color(value_color),
    ]
    .spacing(metrics.space_1);
    if let Some(detail) = detail.filter(|value| !value.is_empty()) {
        content = content.push(
            text(detail)
                .size(metrics.small)
                .color(color(palette.text_muted)),
        );
    }
    content.into()
}

/// A reusable compact row for feeds, status lists, command results, and event
/// streams. The optional marker is semantic state, not widget-specific chrome.
pub(super) fn list_row<'a, Message: 'a>(
    primary: impl Into<String>,
    secondary: Option<String>,
    marker: Option<Color>,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let marker: Element<'a, Message> = marker.map_or_else(
        || space().width(Length::Fixed(8.0)).into(),
        |fill| {
            container(space())
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0))
                .style(move |_| container::Style {
                    background: Some(fill.into()),
                    border: iced::Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        },
    );
    let mut copy = column![
        text(primary.into())
            .size(metrics.body)
            .color(color(palette.text))
    ]
    .spacing(metrics.space_1);
    if let Some(secondary) = secondary.filter(|value| !value.is_empty()) {
        copy = copy.push(
            text(secondary)
                .size(metrics.small)
                .color(color(palette.text_muted)),
        );
    }
    row![marker, copy]
        .spacing(metrics.space_2)
        .align_y(iced::alignment::Vertical::Center)
        .into()
}
