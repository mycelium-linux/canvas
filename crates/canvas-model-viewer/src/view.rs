use canvas_core::VisualTheme;
use iced::widget::container;
use iced::{Element, Length};

use crate::{OrbitAction, OrbitController, state::Timeline};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimelineAction {
    Older,
    Newer,
    Latest,
    Jump(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    ToggleOrbit,
    Camera(OrbitAction),
    Timeline(TimelineAction),
}

pub fn toolbar<'a, Message: Clone + 'a>(
    visual: VisualTheme,
    stream_id: &'a str,
    controller: Option<&OrbitController>,
    operation_timeline: Option<&Timeline>,
    orbitable: bool,
    timeline: bool,
    message: impl Fn(String, Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    use canvas_widgets::live::app_chrome::Item;
    let orbit_enabled = controller.is_some_and(OrbitController::enabled);
    let selected = controller.and_then(OrbitController::named_view);
    let mut controls = Vec::new();
    if orbitable {
        let toggle = message(stream_id.to_string(), Action::ToggleOrbit);
        controls.push(if orbit_enabled {
            Item::active("ORBIT ON", toggle)
        } else {
            Item::action("ORBIT", toggle)
        });
        controls.push(Item::Divider);
        for (label, view) in [("ISO", "iso"), ("F", "front"), ("R", "right"), ("T", "top")] {
            let event = message(
                stream_id.to_string(),
                Action::Camera(OrbitAction::Snap(view)),
            );
            controls.push(if selected == Some(view) {
                Item::active(label, event)
            } else {
                Item::action(label, event)
            });
        }
    }
    if timeline {
        if orbitable {
            controls.push(Item::Divider);
        }
        let steps_back = controller
            .and_then(OrbitController::steps_back)
            .unwrap_or(0);
        controls.push(Item::action(
            "◀",
            message(
                stream_id.to_string(),
                Action::Timeline(TimelineAction::Older),
            ),
        ));
        let fallback;
        let items = if let Some(timeline) = operation_timeline {
            timeline.items.iter().collect::<Vec<_>>()
        } else {
            fallback = (0..=11)
                .rev()
                .map(|back| crate::state::TimelineItem {
                    steps_back: back,
                    glyph: if back == 0 {
                        "●".into()
                    } else {
                        "•".into()
                    },
                    label: format!("step {}", 12 - back),
                    component: None,
                    feature: None,
                })
                .collect::<Vec<_>>();
            fallback.iter().collect()
        };
        let mut component = None;
        for item in items {
            if item.component.as_deref() != component {
                if let Some(name) = item.component.as_deref() {
                    controls.push(Item::Divider);
                    controls.push(Item::Status(name.to_ascii_uppercase()));
                }
                component = item.component.as_deref();
            }
            let back = item.steps_back;
            let event = message(
                stream_id.to_string(),
                Action::Timeline(TimelineAction::Jump(back)),
            );
            let label = format!("{} {}", item.glyph, item.label);
            controls.push(if back == steps_back {
                Item::active(label, event)
            } else if back < steps_back {
                Item::faded(label, event)
            } else {
                Item::action(label, event)
            });
        }
        controls.push(Item::action(
            "▶",
            message(
                stream_id.to_string(),
                Action::Timeline(TimelineAction::Newer),
            ),
        ));
        controls.push(Item::action(
            if steps_back == 0 {
                "LIVE".into()
            } else {
                format!("−{steps_back}")
            },
            message(
                stream_id.to_string(),
                Action::Timeline(TimelineAction::Latest),
            ),
        ));
    }
    canvas_widgets::live::app_chrome::bar(visual, controls)
}

pub fn view_cube<'a, Message: Clone + 'a>(
    visual: VisualTheme,
    stream_id: &'a str,
    controller: Option<&OrbitController>,
    message: impl Fn(String, Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    // Match the model host's default isometric camera before the first input.
    // The cube is rendered by Neo and must never begin detached from the model.
    let default_controller = OrbitController::default();
    let yaw = controller.map_or(default_controller.yaw(), OrbitController::yaw);
    let pitch = controller.map_or(default_controller.pitch(), OrbitController::pitch);
    let selected = controller.and_then(OrbitController::named_view);
    let stream = stream_id.to_owned();
    let cube = crate::view_cube::view(visual, yaw, pitch, selected, move |view| {
        message(stream.clone(), Action::Camera(OrbitAction::Snap(view)))
    });
    container(cube)
        .width(Length::Fixed(132.0))
        .height(Length::Fixed(132.0))
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}
