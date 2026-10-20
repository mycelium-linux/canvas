//! Dense semantic rows using egui layout, truncation and native focus.
use crate::Tokens;
use ::egui::{
    Align, Color32, Label, Layout, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    vec2,
};

/// Semantic emphasis, independent of domain status names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Ordinary informational content.
    Neutral,
    /// Selected or active information.
    Info,
    /// An observed successful/completed state.
    Success,
    /// Attention is required.
    Warning,
    /// An observed error or blocked state.
    Danger,
}
impl Tone {
    /// Resolve an authored sRGB color from the current Coherence appearance.
    pub fn color(self, tokens: &Tokens) -> Color32 {
        let rgb = match self {
            Self::Neutral => tokens.colors.base.muted,
            Self::Info => tokens.colors.info,
            Self::Success => tokens.colors.base.success,
            Self::Warning => tokens.colors.base.warning,
            Self::Danger => tokens.colors.base.danger,
        };
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    }
}

/// Borrowed display facts. No domain action, transport or credentials.
pub struct DenseRow<'a> {
    /// Stable caller-owned identity used for egui focus state.
    pub id: &'a str,
    /// Primary full text, elided visually and retained in hover/accessibility.
    pub title: &'a str,
    /// Explicit human-readable status, never inferred from color.
    pub status: &'a str,
    /// Semantic status emphasis.
    pub tone: Tone,
    /// Compact priority label.
    pub priority: &'a str,
    /// Secondary category, such as a project or mailbox.
    pub category: &'a str,
    /// Human-readable owner label.
    pub owner: &'a str,
    /// Human-readable observed update label.
    pub updated: &'a str,
}

/// Responsive columns shared by a list's header and its visible rows.
#[derive(Clone, Copy, Debug)]
pub struct Columns {
    widths: [f32; 6],
    visible: usize,
}
impl Columns {
    /// Derive column visibility from available logical width and shared metrics.
    pub fn new(width: f32, tokens: &Tokens) -> Self {
        let scale = tokens.metrics.body / 14.0;
        let visible = if width >= 940.0 * scale {
            6
        } else if width >= 760.0 * scale {
            5
        } else if width >= 590.0 * scale {
            4
        } else if width >= 430.0 * scale {
            3
        } else {
            2
        };
        let mut widths = [
            0.0,
            86.0 * scale,
            38.0 * scale,
            96.0 * scale,
            76.0 * scale,
            60.0 * scale,
        ];
        let fixed: f32 = widths[1..visible].iter().sum();
        let gaps = tokens.metrics.space[2] * (visible as f32 - 1.0);
        widths[0] = (width - fixed - gaps).max(1.0);
        Self { widths, visible }
    }
    /// Number of rendered columns; narrow layouts retain primary text and status.
    pub fn visible_count(self) -> usize {
        self.visible
    }
    /// Logical width retained for the title column.
    pub fn title_width(self) -> f32 {
        self.widths[0]
    }
}

fn cell(ui: &mut Ui, width: f32, height: f32, text: &str, color: Color32, size: f32) {
    ui.allocate_ui_with_layout(
        vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(vec2(width, height));
            ui.add(Label::new(RichText::new(text).size(size).color(color)).truncate());
        },
    );
}

/// Draw aligned column labels without consuming selection/focus.
pub fn dense_header(ui: &mut Ui, tokens: &Tokens, columns: Columns, labels: [&str; 6]) {
    let height = tokens.metrics.control_height;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.spacing_mut().item_spacing.x = tokens.metrics.space[2];
    for (index, label) in labels.iter().enumerate().take(columns.visible) {
        cell(
            &mut child,
            columns.widths[index],
            height,
            label,
            Tone::Neutral.color(tokens),
            tokens.metrics.small,
        );
    }
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, super::color(tokens.colors.border)),
    );
}

/// Height of a single-line table row, derived from the shared control target.
pub fn dense_row_height(tokens: &Tokens) -> f32 {
    tokens
        .metrics
        .control_height
        .max(tokens.metrics.body + tokens.metrics.space[1])
}

/// Draw a one-line semantic table row using the minimum shared control target.
pub fn dense_row(
    ui: &mut Ui,
    tokens: &Tokens,
    columns: Columns,
    row: &DenseRow<'_>,
    selected: bool,
) -> Response {
    let height = dense_row_height(tokens);
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let fill = if selected {
        tokens.colors.surface_accent_strong
    } else if response.hovered() {
        tokens.colors.surface_accent
    } else {
        tokens.colors.base.background
    };
    ui.painter()
        .rect_filled(rect, tokens.metrics.radius_small, super::color(fill));
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            tokens.metrics.radius_small,
            Stroke::new(1.0_f32, super::color(tokens.colors.focus)),
            StrokeKind::Inside,
        );
    }
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(row.id)
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.spacing_mut().item_spacing.x = tokens.metrics.space[2];
    let values = [
        row.title,
        row.status,
        row.priority,
        row.category,
        row.owner,
        row.updated,
    ];
    for (index, value) in values.iter().enumerate().take(columns.visible) {
        let color = if index == 0 {
            super::color(tokens.colors.base.text)
        } else if index == 1 {
            row.tone.color(tokens)
        } else {
            Tone::Neutral.color(tokens)
        };
        cell(
            &mut child,
            columns.widths[index],
            height,
            value,
            color,
            tokens.metrics.body,
        );
    }
    response.widget_info(|| {
        ::egui::WidgetInfo::selected(
            ::egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            format!(
                "{}, {}, {}, {}",
                row.title, row.status, row.category, row.owner
            ),
        )
    });
    response.on_hover_text(row.title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Density, Mode};
    #[test]
    fn single_line_tables_do_not_inherit_two_line_list_height() {
        for density in [Density::Compact, Density::Comfortable] {
            let tokens = Tokens::new(Mode::Dark, density);
            assert!(dense_row_height(&tokens) >= tokens.metrics.control_height);
            assert!(dense_row_height(&tokens) < tokens.metrics.row_height);
        }
    }
    #[test]
    fn columns_preserve_title_space_and_reduce_metadata_at_narrow_widths() {
        let tokens = Tokens::new(Mode::Dark, Density::Compact);
        for width in [320.0, 480.0, 640.0, 800.0, 1200.0] {
            let columns = Columns::new(width, &tokens);
            assert!(
                columns.title_width() >= width * 0.35,
                "title lost at {width}"
            );
            assert!((2..=6).contains(&columns.visible_count()));
            let total: f32 = columns.widths[..columns.visible].iter().sum();
            assert!(
                total + tokens.metrics.space[2] * (columns.visible as f32 - 1.0) <= width + 0.1
            );
        }
    }
    #[test]
    fn semantic_status_colors_follow_all_supported_appearances() {
        for mode in [Mode::Light, Mode::Dark, Mode::Sepia] {
            let tokens = Tokens::new(mode, Density::Compact);
            assert_ne!(Tone::Success.color(&tokens), Tone::Danger.color(&tokens));
            assert_ne!(Tone::Info.color(&tokens), Tone::Neutral.color(&tokens));
        }
    }
}
