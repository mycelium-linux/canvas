//! Wiki-race presentation composed from the shared Canvas visual primitives.

use super::primitives::{color, header, list_row, metric};
use crate::{WikiRaceStatus, WikiRaceTraceEntry, wiki_race_timing};
use canvas_core::VisualTheme;
use iced::widget::{column, container, row, scrollable, space, text};
use iced::{Color, Element};

#[derive(Debug, Clone, Copy)]
pub(super) struct WikiRaceView<'a> {
    pub start_page: &'a str,
    pub target_page: &'a str,
    pub clicks: u32,
    pub status: WikiRaceStatus,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub presentation_hold_ms: u64,
    pub hold_started_at_ms: Option<i64>,
    pub now_ms: i64,
}

pub(super) fn body<'a, Message: 'a>(
    race: WikiRaceView<'a>,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let timing = wiki_race_timing(
        race.status,
        race.started_at_ms,
        race.finished_at_ms,
        race.presentation_hold_ms,
        race.hold_started_at_ms,
        race.now_ms,
    );
    let palette = visual.palette;
    let metrics = visual.metrics;
    let status = status_label(race.status);
    let status_color = match race.status {
        WikiRaceStatus::Ready => color(palette.text_muted),
        WikiRaceStatus::Racing => color(palette.success),
        WikiRaceStatus::Holding => color(palette.warning),
        WikiRaceStatus::Finished => color(palette.primary),
        WikiRaceStatus::Stopped => color(palette.danger),
    };
    let clock_label = if timing.final_time {
        "FINAL TIME"
    } else {
        "RACE CLOCK"
    };

    column![
        header("WIKI RACE", Some(status.into()), visual),
        column![
            list_row(
                race.start_page,
                Some("START PAGE".into()),
                Some(color(palette.info)),
                visual,
            ),
            list_row(
                race.target_page,
                Some("TARGET PAGE".into()),
                Some(color(palette.primary)),
                visual,
            ),
        ]
        .spacing(metrics.space_2),
        row![
            metric(
                clock_label,
                format_elapsed(timing.race_ms),
                Some(format!(
                    "{} presentation hold",
                    format_elapsed(race.presentation_hold_ms)
                )),
                color(palette.text),
                visual,
            ),
            space::horizontal(),
            metric(
                "WALL CLOCK",
                format_elapsed(timing.wall_ms),
                Some(format!("{} clicks", race.clicks)),
                color(palette.text),
                visual,
            ),
        ]
        .align_y(iced::alignment::Vertical::Bottom),
        row![
            control(
                "START",
                can_start(race.status),
                color(palette.success),
                visual
            ),
            control("STOP", can_stop(race.status), color(palette.danger), visual),
            space::horizontal(),
            text(format!("{:02} CLICKS", race.clicks))
                .size(metrics.small)
                .color(status_color),
        ]
        .spacing(metrics.space_2)
        .align_y(iced::alignment::Vertical::Center),
    ]
    .spacing(metrics.space_3)
    .into()
}

pub(super) fn trace<'a, Message: 'a>(
    title: &'a str,
    entries: &'a [WikiRaceTraceEntry],
    follow: bool,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let ordered: Box<dyn Iterator<Item = &WikiRaceTraceEntry>> = if follow {
        Box::new(entries.iter().rev())
    } else {
        Box::new(entries.iter())
    };
    let lines = ordered.fold(column![].spacing(metrics.space_2), |lines, entry| {
        let error = entry.error.is_some();
        let marker = if error {
            color(palette.danger)
        } else {
            color(palette.info)
        };
        let primary = entry.error.as_deref().map_or_else(
            || {
                format!(
                    "#{:02}  {}",
                    entry.sequence,
                    entry.selected_choice.as_deref().unwrap_or("navigation")
                )
            },
            |message| format!("#{:02}  ERROR  {message}", entry.sequence),
        );
        let destination = entry.destination_url.as_deref().unwrap_or("—");
        let timing = match (entry.decision_ms, entry.navigation_ms) {
            (Some(decision), Some(navigation)) => {
                format!("decide {decision}ms · load {navigation}ms")
            }
            (Some(decision), None) => format!("decide {decision}ms"),
            (None, Some(navigation)) => format!("load {navigation}ms"),
            (None, None) => "timing pending".into(),
        };
        lines.push(list_row(
            primary,
            Some(format!(
                "{}  {}  {}",
                entry.observed_at, destination, timing
            )),
            Some(marker),
            visual,
        ))
    });
    column![
        header(
            title,
            Some(if follow { "LIVE" } else { "HOLD" }.into()),
            visual
        ),
        scrollable(lines),
    ]
    .spacing(metrics.space_2)
    .into()
}

fn control<'a, Message: 'a>(
    label: &'static str,
    enabled: bool,
    accent: Color,
    visual: &VisualTheme,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let fill = if enabled {
        Color { a: 0.22, ..accent }
    } else {
        Color {
            a: 0.34,
            ..color(palette.surface_muted)
        }
    };
    let stroke = if enabled {
        accent
    } else {
        color(palette.border)
    };
    let foreground = if enabled {
        color(palette.text)
    } else {
        color(palette.text_muted)
    };
    container(text(label).size(metrics.small).color(foreground))
        .padding([metrics.space_1, metrics.space_3])
        .style(move |_| container::Style {
            background: Some(fill.into()),
            border: iced::Border {
                color: stroke,
                width: 1.0,
                radius: metrics.radius_sm.into(),
            },
            ..Default::default()
        })
        .into()
}

fn can_start(status: WikiRaceStatus) -> bool {
    matches!(
        status,
        WikiRaceStatus::Ready | WikiRaceStatus::Finished | WikiRaceStatus::Stopped
    )
}

fn can_stop(status: WikiRaceStatus) -> bool {
    matches!(status, WikiRaceStatus::Racing | WikiRaceStatus::Holding)
}

fn status_label(status: WikiRaceStatus) -> &'static str {
    match status {
        WikiRaceStatus::Ready => "READY",
        WikiRaceStatus::Racing => "RACING",
        WikiRaceStatus::Holding => "FOCUS HOLD",
        WikiRaceStatus::Finished => "FINISHED",
        WikiRaceStatus::Stopped => "STOPPED",
    }
}

fn format_elapsed(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    let seconds = (milliseconds % 60_000) / 1_000;
    let centiseconds = (milliseconds % 1_000) / 10;
    format!("{minutes:02}:{seconds:02}.{centiseconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_format_keeps_centisecond_precision() {
        assert_eq!(format_elapsed(123_456), "02:03.45");
    }

    #[test]
    fn controls_follow_race_lifecycle() {
        assert!(can_start(WikiRaceStatus::Ready));
        assert!(can_stop(WikiRaceStatus::Racing));
        assert!(can_stop(WikiRaceStatus::Holding));
        assert!(!can_stop(WikiRaceStatus::Finished));
    }
}
