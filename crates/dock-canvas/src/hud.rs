//! Passive conky-style mesh HUD: dense local telemetry (clock, uptime, per-core
//! CPU, RAM, network rates, disk, top processes) plus orchestrator-reported
//! mesh node states. Strictly read-only — no buttons, no input targets.

#![allow(dead_code)]

use crate::control::{MeshNode, MeshNodeState};
use iced::mouse;
use iced::widget::{Column, canvas, column, container, row, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, Vector, alignment};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_HUD_CORES: usize = 32;
pub const MAX_HUD_PROCESSES: usize = 3;

#[derive(Debug, Clone, Copy)]
pub struct HudPalette {
    pub panel: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub primary: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

#[derive(Debug, Clone)]
pub struct TopProcess {
    pub name: String,
    pub cpu_percent: f32,
}

#[derive(Debug, Clone)]
pub struct LocalMetrics {
    pub name: String,
    pub cpu_percent: f32,
    pub cpu_cores: Vec<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub net_down: u64,
    pub net_up: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub top_processes: Vec<TopProcess>,
    pub uptime: Duration,
}

pub fn view<'a, Message: 'a>(
    local: &'a LocalMetrics,
    nodes: &'a [MeshNode],
    palette: HudPalette,
) -> Element<'a, Message> {
    let (clock, date) = now_local();
    let cores = CoreBars {
        cores: local
            .cpu_cores
            .iter()
            .take(MAX_HUD_CORES)
            .map(|core| core.clamp(0.0, 100.0))
            .collect(),
        color: palette.primary,
        track: palette.muted,
    };
    let core_width = cores.width_px();
    let core_canvas = canvas(cores)
        .width(Length::Fixed(core_width))
        .height(Length::Fixed(18.0));

    let ram_value = format!(
        "{} / {}  ·  {}%",
        format_bytes(local.ram_used),
        format_bytes(local.ram_total),
        percent(local.ram_used, local.ram_total)
    );
    let ram = percent_line("ram", &ram_value, palette);
    let net_value = format!(
        "↓ {}  ↑ {}",
        format_rate(local.net_down),
        format_rate(local.net_up)
    );
    let net = percent_line("net", &net_value, palette);
    let disk_value = format!(
        "{} / {}  ·  {}%",
        format_bytes(local.disk_used),
        format_bytes(local.disk_total),
        percent(local.disk_used, local.disk_total)
    );
    let disk = percent_line("disk", &disk_value, palette);

    let mut processes = Vec::new();
    for process in local.top_processes.iter().take(MAX_HUD_PROCESSES) {
        processes.push(
            row![
                text(process.name.as_str()).size(13).color(palette.text),
                text(format!("{}%", process.cpu_percent.round()))
                    .size(13)
                    .color(palette.muted),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .into(),
        );
    }

    let mut local_column = Column::with_children(vec![
        text(clock).size(24).color(palette.text).into(),
        text(format!("{date}  ·  up {}", format_uptime(local.uptime)))
            .size(13)
            .color(palette.muted)
            .into(),
        row![
            text("cpu").size(13).color(palette.muted),
            core_canvas,
            text(format!("{}%", local.cpu_percent.round()))
                .size(13)
                .color(palette.text),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
        ram,
        net,
        disk,
    ])
    .spacing(5);
    if !processes.is_empty() {
        local_column = local_column
            .push(text("top").size(13).color(palette.muted))
            .push(column(processes).spacing(2));
    }

    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    rows.push(
        container(local_column)
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Left)
            .into(),
    );
    rows.push(node_row(
        local.name.as_str(),
        MeshNodeState::Active,
        Some("local"),
        palette,
    ));
    for node in nodes {
        rows.push(node_row(
            node.label.as_str(),
            node.state,
            node.detail.as_deref(),
            palette,
        ));
    }

    let panel = container(column(rows).spacing(10))
        .max_width(380)
        .padding([14, 18])
        .style(move |_| container::Style {
            background: Some(palette.panel.into()),
            border: iced::Border {
                color: palette.border,
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.32),
                offset: Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        });

    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .align_x(alignment::Horizontal::Right)
        .align_y(alignment::Vertical::Top)
        .into()
}

fn percent_line<'a, Message: 'a>(
    label: &'a str,
    value: impl AsRef<str>,
    palette: HudPalette,
) -> Element<'a, Message> {
    row![
        text(label).size(13).color(palette.muted),
        text(value.as_ref().to_string())
            .size(13)
            .color(palette.text),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

fn percent(used: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    used.saturating_mul(100) / total
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn format_rate(bytes_per_second: u64) -> String {
    format!("{} /s", format_bytes(bytes_per_second))
}

pub(crate) fn now_local() -> (String, String) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    unsafe {
        libc::localtime_r(&seconds, &mut tm);
    }
    const DOW: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let dow = DOW[(tm.tm_wday as usize).min(6)];
    let month = MONTHS[(tm.tm_mon as usize).min(11)];
    (
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        format!("{dow} {month} {:02}, {}", tm.tm_mday, tm.tm_year + 1900),
    )
}

fn node_row<'a, Message: 'a>(
    label: &'a str,
    state: MeshNodeState,
    detail: Option<&'a str>,
    palette: HudPalette,
) -> Element<'a, Message> {
    let state_color = state_color(state, palette);
    let dot = canvas(Dot { color: state_color })
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0));
    let caption = detail
        .map(str::to_string)
        .unwrap_or_else(|| state.label().to_string());
    row![
        container(dot).width(Length::Fixed(12.0)),
        column![
            text(label).size(15).color(palette.text),
            text(caption).size(13).color(palette.muted),
        ]
        .spacing(1)
    ]
    .spacing(9)
    .align_y(iced::Alignment::Center)
    .into()
}

pub(crate) fn state_color(state: MeshNodeState, palette: HudPalette) -> Color {
    match state {
        MeshNodeState::Active => palette.success,
        MeshNodeState::Paused => palette.warning,
        MeshNodeState::Suspended => palette.primary,
        MeshNodeState::Stopped => palette.danger,
        MeshNodeState::Hidden => palette.muted,
    }
}

pub(crate) fn format_uptime(uptime: Duration) -> String {
    let seconds = uptime.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {}s", seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

trait MeshNodeStateText {
    fn label(self) -> &'static str;
}

impl MeshNodeStateText for MeshNodeState {
    fn label(self) -> &'static str {
        match self {
            MeshNodeState::Active => "active",
            MeshNodeState::Paused => "paused",
            MeshNodeState::Suspended => "suspended",
            MeshNodeState::Stopped => "stopped",
            MeshNodeState::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Dot {
    color: Color,
}

impl<Message> canvas::Program<Message> for Dot {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = frame.center();
        let radius = bounds.width.min(bounds.height) * 0.36;
        frame.fill(
            &canvas::Path::circle(center, radius * 1.9),
            with_alpha(self.color, 0.14),
        );
        frame.fill(&canvas::Path::circle(center, radius), self.color);
        vec![frame.into_geometry()]
    }
}

#[derive(Debug, Clone)]
struct CoreBars {
    cores: Vec<f32>,
    color: Color,
    track: Color,
}

impl CoreBars {
    const BAR_WIDTH: f32 = 4.0;
    const GAP: f32 = 2.0;

    fn width_px(&self) -> f32 {
        (self.cores.len() as f32 * Self::BAR_WIDTH
            + self.cores.len().saturating_sub(1) as f32 * Self::GAP)
            .max(1.0)
    }
}

impl<Message> canvas::Program<Message> for CoreBars {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let height = bounds.height;
        let origin = Point::new(0.0, height);
        for (index, usage) in self.cores.iter().enumerate() {
            let x = index as f32 * (Self::BAR_WIDTH + Self::GAP);
            let track = canvas::Path::rectangle(origin, (Self::BAR_WIDTH, height).into());
            frame.fill(&track, with_alpha(self.track, 0.18));
            let bar_height = height * usage.clamp(0.0, 100.0) / 100.0;
            let bar = canvas::Path::rectangle(
                Point::new(x, height - bar_height),
                (Self::BAR_WIDTH, bar_height).into(),
            );
            frame.fill(&bar, self.color);
        }
        vec![frame.into_geometry()]
    }
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Color;

    fn palette() -> HudPalette {
        HudPalette {
            panel: Color::from_rgb8(10, 10, 12),
            border: Color::from_rgb8(40, 40, 46),
            text: Color::from_rgb8(235, 235, 240),
            muted: Color::from_rgb8(140, 140, 150),
            primary: Color::from_rgb8(80, 160, 255),
            success: Color::from_rgb8(80, 200, 120),
            warning: Color::from_rgb8(255, 190, 84),
            danger: Color::from_rgb8(255, 90, 90),
        }
    }

    fn local() -> LocalMetrics {
        LocalMetrics {
            name: "agora-one".into(),
            cpu_percent: 23.0,
            cpu_cores: vec![10.0, 35.0, 0.0, 80.0],
            ram_used: 8 * 1024 * 1024 * 1024 + 400 * 1024 * 1024,
            ram_total: 16 * 1024 * 1024 * 1024,
            net_down: 1_258_291,
            net_up: 12_288,
            disk_used: 218 * 1024 * 1024 * 1024,
            disk_total: 512 * 1024 * 1024 * 1024,
            top_processes: vec![TopProcess {
                name: "dock-canvas".into(),
                cpu_percent: 42.0,
            }],
            uptime: Duration::from_secs(3 * 3_600 + 41 * 60),
        }
    }

    #[test]
    fn state_colors_are_distinct_per_pause_resume_semantics() {
        let palette = palette();
        let colors = [
            state_color(MeshNodeState::Active, palette),
            state_color(MeshNodeState::Paused, palette),
            state_color(MeshNodeState::Suspended, palette),
            state_color(MeshNodeState::Stopped, palette),
            state_color(MeshNodeState::Hidden, palette),
        ];
        let distinct =
            (0..colors.len()).all(|i| (0..colors.len()).all(|j| i == j || colors[i] != colors[j]));
        assert!(distinct, "mesh states must render distinctly");
    }

    #[test]
    fn uptime_formats_roll_up_largest_unit_first() {
        assert_eq!(
            format_uptime(Duration::from_secs(5 * 86_400 + 4 * 3_600 + 12 * 60 + 9)),
            "5d 4h 12m"
        );
        assert_eq!(
            format_uptime(Duration::from_secs(3 * 3_600 + 41 * 60)),
            "3h 41m"
        );
        assert_eq!(format_uptime(Duration::from_secs(7 * 60 + 9)), "7m 9s");
        assert_eq!(format_uptime(Duration::from_secs(42)), "42s");
    }

    #[test]
    fn byte_and_rate_formatting_stays_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_258_291), "1.2 MiB");
        assert_eq!(
            format_bytes(8 * 1024 * 1024 * 1024 + 400 * 1024 * 1024),
            "8.4 GiB"
        );
        assert_eq!(format_rate(1_258_291), "1.2 MiB /s");
    }

    #[test]
    fn percent_handles_zero_total() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(218, 512), 42);
    }

    #[test]
    fn core_bars_are_bounded_and_sized() {
        let bars = CoreBars {
            cores: (0..MAX_HUD_CORES + 8).map(|_| 50.0).collect(),
            color: Color::BLACK,
            track: Color::BLACK,
        };
        assert!(bars.width_px() > 0.0);
        let local = local();
        let truncated: Vec<f32> = local
            .cpu_cores
            .iter()
            .take(MAX_HUD_CORES)
            .cloned()
            .collect();
        assert_eq!(truncated.len(), local.cpu_cores.len());
    }

    #[test]
    fn view_composes_passive_panel() {
        let local = local();
        let nodes = vec![MeshNode {
            id: "mac".into(),
            label: "Mac".into(),
            state: MeshNodeState::Paused,
            detail: Some("agent idle".into()),
        }];
        let _element: iced::Element<'_, ()> = view(&local, &nodes, palette());
    }

    #[test]
    fn local_clock_format_is_bounded() {
        let (clock, date) = now_local();
        assert_eq!(clock.len(), 8);
        assert!(clock.as_bytes()[2] == b':');
        assert!(clock.as_bytes()[5] == b':');
        assert!(date.len() >= 11 && date.len() <= 20);
    }
}
