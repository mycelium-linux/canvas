use dock_canvas::control::{
    PresenceOccupant, PresencePreview, PresenceSignal, PresenceSignalState, PresenceSnapshot,
};
use iced::mouse;
use iced::widget::{canvas, column, container, image, row, stack, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, Vector, alignment};
use std::time::Duration;

const RAIL_HEIGHT: f32 = 58.0;
const PREVIEW_WIDTH: f32 = 360.0;
const PREVIEW_HEIGHT: f32 = 270.0;
const MAX_VISIBLE_OCCUPANTS: usize = 4;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PresencePalette {
    pub panel: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub presence: Color,
    pub face: Color,
    pub voice: Color,
    pub warning: Color,
}

pub(crate) struct Preview<'a> {
    pub value: &'a PresencePreview,
    pub image: iced::widget::image::Handle,
    pub elapsed: Duration,
}

pub(crate) fn rail<'a, Message: 'a>(
    snapshot: &'a PresenceSnapshot,
    stale: bool,
    palette: PresencePalette,
) -> Element<'a, Message> {
    let zone = column![
        text(snapshot.zone.as_str()).size(16).color(palette.text),
        text(if stale {
            "Presence stale"
        } else if snapshot.occupied {
            "Room occupied"
        } else {
            "Room clear"
        })
        .size(11)
        .color(if stale {
            palette.warning
        } else {
            palette.muted
        }),
    ]
    .spacing(1)
    .width(Length::Fixed(150.0));

    let mut occupants = row![].spacing(26).align_y(iced::Alignment::Center);
    for occupant in snapshot.occupants.iter().take(MAX_VISIBLE_OCCUPANTS) {
        occupants = occupants.push(occupant_view(occupant, stale, palette));
    }
    if snapshot.occupants.len() > MAX_VISIBLE_OCCUPANTS {
        occupants = occupants.push(
            text(format!(
                "+{}",
                snapshot.occupants.len() - MAX_VISIBLE_OCCUPANTS
            ))
            .size(12)
            .color(palette.muted),
        );
    }
    if snapshot.occupants.is_empty() {
        occupants = occupants.push(text("No one detected").size(12).color(palette.muted));
    }

    let content = row![zone, occupants]
        .spacing(26)
        .align_y(iced::Alignment::Center);
    let bar = container(content)
        .width(Length::Fill)
        .height(Length::Fixed(RAIL_HEIGHT))
        .padding([8, 18])
        .style(move |_| container::Style {
            background: Some(palette.panel.into()),
            border: iced::Border {
                color: palette.border,
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.2),
                offset: Vector::new(0.0, -3.0),
                blur_radius: 14.0,
            },
            ..Default::default()
        });

    container(bar)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(alignment::Vertical::Bottom)
        .into()
}

fn occupant_view<'a, Message: 'a>(
    occupant: &'a PresenceOccupant,
    stale: bool,
    palette: PresencePalette,
) -> Element<'a, Message> {
    column![
        text(occupant.display_name.as_str())
            .size(13)
            .color(palette.text),
        row![
            signal(
                "Presence",
                &occupant.presence,
                stale,
                palette.presence,
                palette
            ),
            signal("Face", &occupant.face, stale, palette.face, palette),
            signal("Voice", &occupant.voice, stale, palette.voice, palette),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(2)
    .into()
}

fn signal<'a, Message: 'a>(
    label: &'a str,
    signal: &'a PresenceSignal,
    stale: bool,
    channel: Color,
    palette: PresencePalette,
) -> Element<'a, Message> {
    let active = !stale
        && matches!(
            signal.state,
            PresenceSignalState::Detected | PresenceSignalState::Verified
        );
    let color = if active { channel } else { palette.muted };
    row![
        canvas(SignalDot { color, active })
            .width(Length::Fixed(7.0))
            .height(Length::Fixed(7.0)),
        text(label).size(11).color(color),
    ]
    .spacing(5)
    .align_y(iced::Alignment::Center)
    .into()
}

pub(crate) fn preview<'a, Message: 'a>(
    preview: Preview<'a>,
    palette: PresencePalette,
) -> Element<'a, Message> {
    let phase = (preview.elapsed.as_secs_f32() / 0.7).min(1.0);
    let zoom = 1.0 + (1.0 - phase) * 0.045;
    let picture = image(preview.image)
        .width(Length::Fixed(PREVIEW_WIDTH))
        .height(Length::Fixed(PREVIEW_HEIGHT))
        .content_fit(iced::ContentFit::Cover)
        .scale(zoom);
    let tracker = canvas(FaceTracker {
        faces: preview.value.faces.as_slice(),
        source_size: [preview.value.width, preview.value.height],
        zoom,
        elapsed: preview.elapsed,
        palette,
    })
    .width(Length::Fixed(PREVIEW_WIDTH))
    .height(Length::Fixed(PREVIEW_HEIGHT));
    let best = preview
        .value
        .faces
        .iter()
        .filter(|face| face.occupant_id.is_some())
        .max_by(|a, b| a.confidence.total_cmp(&b.confidence));
    let name = best
        .and_then(|face| face.display_name.as_deref())
        .unwrap_or("Unrecognized person");
    let confidence = best
        .or_else(|| {
            preview
                .value
                .faces
                .iter()
                .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
        })
        .map_or(0.0, |face| face.confidence);
    let status_color = if best.is_some() {
        palette.face
    } else {
        palette.warning
    };
    let footer = row![
        column![
            text(name).size(17).color(palette.text),
            text(if best.is_some() {
                "Face verified"
            } else {
                "Face detected"
            })
            .size(11)
            .color(status_color),
        ]
        .spacing(1),
        text(format!("{:.0}%", confidence * 100.0))
            .size(12)
            .color(palette.muted),
    ]
    .spacing(14)
    .align_y(iced::Alignment::Center);
    let panel = container(column![stack![picture, tracker], footer].spacing(10))
        .padding(10)
        .style(move |_| container::Style {
            background: Some(palette.panel.into()),
            border: iced::Border {
                color: palette.border,
                width: 1.0,
                radius: 6.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.34),
                offset: Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        });

    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(iced::Padding {
            top: 18.0,
            right: 18.0,
            bottom: RAIL_HEIGHT + 18.0,
            left: 18.0,
        })
        .align_x(alignment::Horizontal::Left)
        .align_y(alignment::Vertical::Bottom)
        .into()
}

struct SignalDot {
    color: Color,
    active: bool,
}

impl<Message> canvas::Program<Message> for SignalDot {
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
        if self.active {
            frame.fill(
                &canvas::Path::circle(frame.center(), 3.5),
                Color {
                    a: 0.18,
                    ..self.color
                },
            );
        }
        frame.fill(
            &canvas::Path::circle(frame.center(), if self.active { 2.3 } else { 1.7 }),
            self.color,
        );
        vec![frame.into_geometry()]
    }
}

struct FaceTracker<'a> {
    faces: &'a [dock_canvas::control::PresencePreviewFace],
    source_size: [u32; 2],
    zoom: f32,
    elapsed: Duration,
    palette: PresencePalette,
}

impl<Message> canvas::Program<Message> for FaceTracker<'_> {
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
        let pulse = (self.elapsed.as_secs_f32() * 2.4).sin().abs();
        let image = cover_geometry(self.source_size, bounds.size(), self.zoom);
        for face in self.faces {
            let color = if face.occupant_id.is_some() {
                self.palette.face
            } else {
                self.palette.warning
            };
            let x = image.x + face.bounds[0] * image.width;
            let y = image.y + face.bounds[1] * image.height;
            let width = (face.bounds[2] - face.bounds[0]) * image.width;
            let height = (face.bounds[3] - face.bounds[1]) * image.height;
            let rectangle =
                canvas::Path::rectangle(Point::new(x, y), iced::Size::new(width, height));
            frame.stroke(
                &rectangle,
                canvas::Stroke::default()
                    .with_width(1.4 + pulse * 0.6)
                    .with_color(Color { a: 0.9, ..color }),
            );
            let scan_y = y + height * ((self.elapsed.as_secs_f32() * 0.75) % 1.0);
            frame.stroke(
                &canvas::Path::line(Point::new(x, scan_y), Point::new(x + width, scan_y)),
                canvas::Stroke::default()
                    .with_width(1.0)
                    .with_color(Color { a: 0.32, ..color }),
            );
        }
        vec![frame.into_geometry()]
    }
}

fn cover_geometry(source: [u32; 2], bounds: iced::Size, zoom: f32) -> Rectangle {
    let source_width = source[0] as f32;
    let source_height = source[1] as f32;
    let scale = (bounds.width / source_width).max(bounds.height / source_height) * zoom;
    let width = source_width * scale;
    let height = source_height * scale;
    Rectangle::new(
        Point::new((bounds.width - width) / 2.0, (bounds.height - height) / 2.0),
        iced::Size::new(width, height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_geometry_matches_centered_cover_crop_and_zoom() {
        let wide = cover_geometry([640, 360], iced::Size::new(360.0, 270.0), 1.0);
        assert_eq!(
            wide,
            Rectangle::new(Point::new(-60.0, 0.0), iced::Size::new(480.0, 270.0))
        );

        let zoomed = cover_geometry([640, 480], iced::Size::new(360.0, 270.0), 1.04);
        assert!((zoomed.x + 7.2).abs() < 0.001);
        assert!((zoomed.y + 5.4).abs() < 0.001);
        assert!((zoomed.width - 374.4).abs() < 0.001);
        assert!((zoomed.height - 280.8).abs() < 0.001);
    }
}
