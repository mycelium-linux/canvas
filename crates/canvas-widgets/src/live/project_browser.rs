//! Native Finder-style project browser presentation.
//!
//! Thumbnail bytes are supplied by the host after CID resolution. This view
//! never fetches and therefore cannot bypass project authorization or create a
//! second cache.

use std::collections::BTreeMap;

use canvas_core::VisualTheme;
use canvas_core::project_browser::{
    BrowserAction, BrowserItem, BrowserModel, BrowserState, GalleryPlan, PreviewKind,
    ProjectManifest,
};
use iced::widget::{
    button, column, container, grid, image, row, scrollable, space, text, text_input,
};
use iced::{ContentFit, Element, Length};

use super::app_chrome::{
    Density, action_style, list_item_style, panel_style, rgb, work_area_style,
};

pub fn project_view<'a, Message: Clone + 'a>(
    title: &'a str,
    owner: &'a str,
    node: &'a str,
    mirror_connected: bool,
    manifest: &'a ProjectManifest,
    model: &'a BrowserModel,
    state: &'a BrowserState,
    thumbnails: &'a BTreeMap<String, image::Handle>,
    visual: VisualTheme,
    on_action: impl Fn(BrowserAction) -> Message + Clone + 'a,
    switch_project: Message,
    close: Message,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let insets = Density::Compact.insets(visual);
    let sync_color = if mirror_connected {
        palette.success
    } else {
        palette.warning
    };
    let sync_label = if mirror_connected {
        "Shared workspace connected"
    } else {
        "Local workspace only"
    };
    let header = row![
        column![
            text(title).size(metrics.title).color(rgb(palette.text)),
            row![
                text(format!("{owner} · {} files", manifest.entries.len()))
                    .size(metrics.small)
                    .color(rgb(palette.text_muted)),
                text("●").size(metrics.small).color(rgb(sync_color)),
                text(format!("{sync_label} on {node}"))
                    .size(metrics.small)
                    .color(rgb(palette.text_muted)),
            ]
            .spacing(metrics.space_2)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(metrics.space_1),
        space::horizontal(),
        button(text("Projects").size(metrics.small))
            .padding([insets.control_y, insets.control_x])
            .on_press(switch_project)
            .style(move |_, status| action_style(visual, false, false, status)),
        button(text("Close").size(metrics.small))
            .padding([insets.control_y, insets.control_x])
            .on_press(close)
            .style(move |_, status| action_style(visual, false, false, status)),
    ]
    .spacing(insets.gap)
    .align_y(iced::Alignment::Center);

    let mut roots = manifest
        .entries
        .iter()
        .filter_map(|entry| entry.path.split_once('/').map(|(root, _)| root))
        .collect::<Vec<_>>();
    roots.sort_unstable();
    roots.dedup();
    let mut sidebar = column![
        text("Project")
            .size(metrics.micro)
            .color(rgb(palette.text_muted)),
        button(text("All files").size(metrics.body))
            .width(Length::Fill)
            .on_press(on_action(BrowserAction::OpenFolder(String::new())))
            .style(move |_, status| action_style(visual, state.folder.is_empty(), false, status)),
    ]
    .spacing(insets.gap);
    for root in roots {
        let selected = state.folder == root || state.folder.starts_with(&format!("{root}/"));
        sidebar = sidebar.push(
            button(text(root.replace(['-', '_'], " ")).size(metrics.body))
                .width(Length::Fill)
                .on_press(on_action(BrowserAction::OpenFolder(root.to_owned())))
                .style(move |_, status| action_style(visual, selected, false, status)),
        );
    }
    let browser = view(model, state, thumbnails, visual, on_action);
    container(
        column![
            header,
            row![
                container(sidebar)
                    .padding(insets.panel)
                    .width(Length::Fixed(180.0))
                    .height(Length::Fill)
                    .style(move |_| panel_style(visual)),
                browser,
            ]
            .spacing(insets.gap)
            .height(Length::Fill),
        ]
        .spacing(insets.gap),
    )
    .padding(insets.panel)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| work_area_style(visual))
    .into()
}

pub fn view<'a, Message: Clone + 'a>(
    model: &'a BrowserModel,
    state: &'a BrowserState,
    thumbnails: &'a BTreeMap<String, image::Handle>,
    visual: VisualTheme,
    on_action: impl Fn(BrowserAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    view_with_density(
        model,
        state,
        thumbnails,
        visual,
        Density::Compact,
        on_action,
    )
}

pub fn view_with_density<'a, Message: Clone + 'a>(
    model: &'a BrowserModel,
    state: &'a BrowserState,
    thumbnails: &'a BTreeMap<String, image::Handle>,
    visual: VisualTheme,
    density: Density,
    on_action: impl Fn(BrowserAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let insets = density.insets(visual);
    let mut crumbs = row![].spacing(insets.gap);
    for crumb in &model.breadcrumbs {
        crumbs = crumbs.push(
            button(text(crumb.label.clone()).size(metrics.small))
                .padding([insets.control_y, insets.control_x])
                .on_press(on_action(BrowserAction::OpenFolder(crumb.folder.clone())))
                .style(move |_, status| action_style(visual, false, false, status)),
        );
    }
    crumbs = crumbs.push(
        text(format!("REV {}", model.revision))
            .size(metrics.micro)
            .color(rgb(palette.text_muted)),
    );

    let search = text_input("Search names in this project…", &state.query)
        .on_input({
            let on_action = on_action.clone();
            move |query| on_action(BrowserAction::Query(query))
        })
        .padding(insets.control_x)
        .size(metrics.body);

    let mut list = column![].spacing(insets.gap);
    for item in &model.items {
        let (id, detail, preview, action, selected) = match item {
            BrowserItem::Folder {
                name: _,
                path,
                descendants,
            } => (
                None,
                format!("{descendants} items"),
                PreviewKind::Generic,
                BrowserAction::OpenFolder(path.clone()),
                false,
            ),
            BrowserItem::File { entry, preview } => (
                Some(entry.id.as_str()),
                format!(
                    "{} · {}",
                    short_media(&entry.media_type),
                    format_bytes(entry.bytes)
                ),
                *preview,
                BrowserAction::Select(entry.id.clone()),
                state.selected_id.as_deref() == Some(entry.id.as_str()),
            ),
        };
        let thumbnail = thumbnail(
            id.and_then(|id| thumbnails.get(id)),
            preview,
            visual,
            Length::Fixed(if density == Density::Compact {
                60.0
            } else {
                72.0
            }),
            Length::Fixed(if density == Density::Compact {
                45.0
            } else {
                54.0
            }),
        );
        let content = row![
            thumbnail,
            column![
                text(item.label())
                    .size(metrics.body)
                    .color(rgb(palette.text)),
                text(detail)
                    .size(metrics.small)
                    .color(rgb(palette.text_muted)),
            ]
            .spacing(insets.gap)
        ]
        .spacing(insets.control_x)
        .align_y(iced::Alignment::Center);
        list = list.push(
            button(content)
                .width(Length::Fill)
                .padding(insets.control_y)
                .on_press(on_action(action))
                .style(move |_, status| list_item_style(visual, selected, status)),
        );
    }
    if model.items.is_empty() {
        list = list.push(
            container(
                text(if state.query.trim().is_empty() {
                    "This folder is empty"
                } else {
                    "No matching files"
                })
                .size(metrics.body)
                .color(rgb(palette.text_muted)),
            )
            .padding(insets.panel),
        );
    }

    let preview: Element<'a, Message> = if let Some(entry) = &model.selected {
        let kind = canvas_core::project_browser::preview_kind(entry);
        let picture = thumbnail(
            thumbnails.get(&entry.id),
            kind,
            visual,
            Length::Fill,
            Length::FillPortion(3),
        );
        column![
            picture,
            text(entry.display_name.clone())
                .size(metrics.title)
                .color(rgb(palette.text)),
            text(format!(
                "{} · {}",
                entry.media_type,
                format_bytes(entry.bytes)
            ))
            .size(metrics.small)
            .color(rgb(palette.text_muted)),
            button(text("OPEN").size(metrics.small))
                .padding([metrics.space_1, metrics.space_3])
                .on_press(on_action(BrowserAction::OpenSelected))
                .style(move |_, status| action_style(visual, true, false, status)),
        ]
        .spacing(insets.control_x)
        .into()
    } else {
        container(
            text("Select a file for a larger preview")
                .size(metrics.body)
                .color(rgb(palette.text_muted)),
        )
        .center(Length::Fill)
        .into()
    };

    let count = if model.more == 0 {
        format!("{} items", model.matched)
    } else {
        format!("{} items · {} more", model.items.len(), model.more)
    };
    container(
        column![
            crumbs,
            search,
            row![
                scrollable(list)
                    .height(Length::Fill)
                    .width(Length::FillPortion(2)),
                container(preview)
                    .padding(insets.panel)
                    .width(Length::FillPortion(3))
                    .height(Length::Fill)
                    .style(move |_| panel_style(visual)),
            ]
            .spacing(insets.gap)
            .height(Length::Fill),
            text(count)
                .size(metrics.micro)
                .color(rgb(palette.text_muted)),
        ]
        .spacing(insets.gap),
    )
    .padding(insets.panel)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| work_area_style(visual))
    .into()
}

/// Full-page query result. Every slot exists from the first frame so the page
/// does not jump around; cached previews settle in according to the bounded
/// reveal plan while misses continue resolving independently.
pub fn gallery<'a, Message: Clone + 'a>(
    plan: &'a GalleryPlan,
    thumbnails: &'a BTreeMap<String, image::Handle>,
    elapsed_ms: u64,
    visual: VisualTheme,
    on_open: impl Fn(String) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    let metrics = visual.metrics;
    let palette = visual.palette;
    let cards = plan.slots.iter().map(|slot| {
        let revealed = elapsed_ms >= slot.reveal_after_ms;
        let preview = thumbnail(
            revealed.then(|| thumbnails.get(&slot.entry.id)).flatten(),
            slot.preview,
            visual,
            Length::Fill,
            Length::FillPortion(3),
        );
        let label = if revealed {
            slot.entry.display_name.clone()
        } else {
            "Resolving preview…".into()
        };
        button(
            column![
                preview,
                text(label).size(metrics.body).color(rgb(if revealed {
                    palette.text
                } else {
                    palette.text_muted
                })),
                text(format_bytes(slot.entry.bytes))
                    .size(metrics.micro)
                    .color(rgb(palette.text_muted)),
            ]
            .spacing(metrics.space_1),
        )
        .padding(metrics.space_1)
        .on_press(on_open(slot.entry.id.clone()))
        .style(move |_, status| action_style(visual, false, false, status))
        .into()
    });
    let summary = if plan.more == 0 {
        format!("{} results", plan.matched)
    } else {
        format!("{} shown · {} more", plan.slots.len(), plan.more)
    };
    let filters = gallery_filter_summary(&plan.query);
    container(
        column![
            column![
                row![
                    text(plan.project_id.to_uppercase())
                        .size(metrics.title)
                        .color(rgb(palette.text)),
                    space::horizontal().width(Length::Fill),
                    text(summary)
                        .size(metrics.small)
                        .color(rgb(palette.text_muted)),
                ]
                .align_y(iced::Alignment::Center),
                row![
                    text(filters)
                        .size(metrics.small)
                        .color(rgb(palette.primary)),
                    space::horizontal().width(Length::Fill),
                    text(format!("REV {}", plan.manifest_revision))
                        .size(metrics.micro)
                        .color(rgb(palette.text_muted)),
                ]
                .align_y(iced::Alignment::Center),
            ]
            .spacing(metrics.space_1),
            scrollable(grid(cards).fluid(260.0).spacing(metrics.space_2)).height(Length::Fill),
        ]
        .spacing(metrics.space_2),
    )
    .padding(metrics.space_2)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| work_area_style(visual))
    .into()
}

fn thumbnail<'a, Message: 'a>(
    handle: Option<&image::Handle>,
    kind: PreviewKind,
    visual: VisualTheme,
    width: Length,
    height: Length,
) -> Element<'a, Message> {
    if let Some(handle) = handle {
        return image(handle.clone())
            .content_fit(ContentFit::Contain)
            .width(width)
            .height(height)
            .into();
    }
    let label = match kind {
        PreviewKind::Image => "IMAGE",
        PreviewKind::Pdf => "PDF",
        PreviewKind::Model => "3D",
        PreviewKind::Markdown => "MD",
        PreviewKind::Text => "TEXT",
        PreviewKind::Generic => "FILE",
    };
    container(
        column![
            space::vertical().height(Length::Fill),
            text(label)
                .size(visual.metrics.small)
                .color(rgb(visual.palette.text_muted)),
            space::vertical().height(Length::Fill),
        ]
        .align_x(iced::Alignment::Center),
    )
    .center_x(width)
    .height(height)
    .style(move |_| panel_style(visual))
    .into()
}

fn short_media(media: &str) -> &str {
    media.rsplit_once('/').map_or(media, |(_, subtype)| subtype)
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn gallery_filter_summary(query: &canvas_core::project_browser::GalleryQuery) -> String {
    let mut parts: Vec<String> = query
        .formats
        .iter()
        .map(|format| format.to_uppercase())
        .collect();
    parts.extend(query.terms.iter().cloned());
    if let Some(after) = query.modified_after_ms {
        parts.push(format!("since {}", unix_date(after)));
    }
    if let Some(before) = query.modified_before_ms {
        parts.push(format!("before {}", unix_date(before)));
    }
    if parts.is_empty() {
        "All project files".into()
    } else {
        parts.join("  ·  ")
    }
}

fn unix_date(timestamp_ms: u64) -> String {
    // Proleptic Gregorian conversion from days since the Unix epoch. Keeping
    // this tiny avoids adding a clock dependency to a renderer that receives
    // absolute cutoffs from its caller.
    let z = (timestamp_ms / 86_400_000) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_compact_and_human_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn gallery_keeps_exact_query_provenance_visible() {
        let text = gallery_filter_summary(&canvas_core::project_browser::GalleryQuery {
            project_id: "transmog".into(),
            formats: vec!["pdf".into()],
            terms: vec!["thermal".into()],
            modified_after_ms: Some(42),
            modified_before_ms: None,
            limit: 24,
        });
        assert_eq!(text, "PDF  ·  thermal  ·  since 1970-01-01");
        assert_eq!(unix_date(1_704_067_200_000), "2024-01-01");
    }
}
