//! Stateful map interaction with independently ordered raster/vector GPU layers.
use super::{gfx_signals, map_raster, CacheState};
use advanced::{graphics::geometry::Renderer as _, image::Renderer as _, Renderer as _};
use fpl_gfx::{
    geo_map::{camera::Camera, raster::cropped_extent},
    signals::SignalFrame,
    Rect,
};
use iced::{
    advanced::{
        self, image, layout, renderer,
        widget::{tree, Tree},
        Widget,
    },
    mouse, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
};

pub(super) fn view<'a, Message: 'a>(
    frame: SignalFrame,
    theme: fpl_gfx::Theme,
) -> Element<'a, Message> {
    if let SignalFrame::GeoMap { map } = &frame {
        if let Err(error) = map.validate() {
            return iced::widget::text(format!("Map error: {error}")).into();
        }
    }
    Element::new(MapWidget { frame, theme })
}
struct MapWidget {
    frame: SignalFrame,
    theme: fpl_gfx::Theme,
}
#[derive(Default)]
struct State {
    camera: Camera,
    drag: Option<Point>,
    geometry: CacheState,
}
fn image_bounds(
    base: fpl_gfx::geo::Extent,
    view: fpl_gfx::geo::Extent,
    plot: Rect,
    bounds: Rectangle,
) -> Rectangle {
    let sx = (base.east_deg - base.west_deg) / (view.east_deg - view.west_deg);
    let sy = (base.north_deg - base.south_deg) / (view.north_deg - view.south_deg);
    let x = f64::from(plot.x)
        + (base.west_deg - view.west_deg) / (view.east_deg - view.west_deg) * f64::from(plot.width)
        - f64::from(plot.x) * sx;
    let y = f64::from(plot.y)
        + (view.north_deg - base.north_deg) / (view.north_deg - view.south_deg)
            * f64::from(plot.height)
        - f64::from(plot.y) * sy;
    Rectangle::new(
        Point::new(bounds.x + x as f32, bounds.y + y as f32),
        Size::new(bounds.width * sx as f32, bounds.height * sy as f32),
    )
}
impl MapWidget {
    fn region(&self, bounds: Rectangle) -> Option<(Rect, fpl_gfx::geo::Extent)> {
        let SignalFrame::GeoMap { map } = &self.frame else {
            return None;
        };
        let plot = map
            .inner(Rect::new(0., 0., bounds.width, bounds.height))
            .ok()?;
        let extent = if map.basemap.is_some() {
            cropped_extent(map.extent, plot).ok()?
        } else {
            map.extent
        };
        Some((plot, extent))
    }
    fn interact(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> bool {
        let Some((plot, base)) = self.region(bounds) else {
            return false;
        };
        let position = cursor.position_in(bounds);
        let inside = position.is_some_and(|p| {
            p.x >= plot.x && p.x <= plot.right() && p.y >= plot.y && p.y <= plot.bottom()
        });
        let handled = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if inside => {
                state.drag = position;
                true
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.drag.is_some() =>
            {
                state.drag = None;
                true
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.drag.is_some() => {
                let point = Point::new(position.x - bounds.x, position.y - bounds.y);
                let previous = state.drag.replace(point).unwrap();
                state
                    .camera
                    .pan(
                        base,
                        f64::from((point.x - previous.x) / plot.width),
                        f64::from((point.y - previous.y) / plot.height),
                    )
                    .is_ok()
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let amount = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => f64::from(*y) * 0.18,
                    mouse::ScrollDelta::Pixels { y, .. } => f64::from(*y) * 0.002,
                };
                let p = position.unwrap();
                state
                    .camera
                    .zoom_at(
                        base,
                        amount.clamp(-0.8, 0.8).exp(),
                        [
                            f64::from((p.x - plot.x) / plot.width),
                            f64::from((p.y - plot.y) / plot.height),
                        ],
                    )
                    .is_ok()
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) if inside => {
                state.camera = Camera::default();
                state.drag = None;
                true
            }
            Event::Window(iced::window::Event::Unfocused) => {
                state.drag = None;
                false
            }
            _ => false,
        };
        handled
    }
}
impl<Message> Widget<Message, Theme, Renderer> for MapWidget {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn layout(&mut self, _: &mut Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        _: &Renderer,
        _: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, Message>,
        _: &Rectangle,
    ) {
        if shell.is_event_captured() {
            return;
        }
        if self.interact(
            tree.state.downcast_mut::<State>(),
            event,
            layout.bounds(),
            cursor,
        ) {
            shell.capture_event();
            shell.request_redraw();
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        if tree.state.downcast_ref::<State>().drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _: &renderer::Style,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        let Some((plot, base)) = self.region(bounds) else {
            let program = gfx_signals::Program {
                frame: self.frame.clone(),
                theme: self.theme.clone(),
                fingerprint: 0,
                raster_backed: false,
                raster_error: Some("Map viewport is too small".into()),
            };
            renderer.with_layer(bounds, |renderer| {
                renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
                    for geometry in iced::widget::canvas::Program::<Message>::draw(
                        &program,
                        &state.geometry,
                        renderer,
                        theme,
                        bounds,
                        cursor,
                    ) {
                        renderer.draw_geometry(geometry);
                    }
                })
            });
            return;
        };
        let mut frame = self.frame.clone();
        let SignalFrame::GeoMap { map } = &mut frame else {
            return;
        };
        let view = match state.camera.extent(base) {
            Ok(view) => view,
            Err(_) => base,
        };
        map.extent = view;
        let displayed = if map.basemap.is_some() {
            cropped_extent(view, plot).unwrap_or(view)
        } else {
            view
        };
        let SignalFrame::GeoMap { map: source } = &self.frame else {
            return;
        };
        let raster = if source.basemap.is_some() {
            std::env::var_os("CANVAS_MAP_ASSETS")
                .ok_or_else(|| "Map asset directory not configured".to_string())
                .and_then(|root| {
                    map_raster::retained(
                        source,
                        bounds.width as u32,
                        bounds.height as u32,
                        std::path::Path::new(&root),
                    )
                })
                .map(Some)
        } else {
            Ok(None)
        };
        let (image, error) = match raster {
            Ok(image) => (image, None),
            Err(error) => (None, Some(error)),
        };
        let c = self.theme.color(fpl_gfx::ThemeRole::SurfaceRecessed);
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..Default::default()
            },
            iced::Color::from_linear_rgba(c.r, c.g, c.b, 1.),
        );
        let clip = Rectangle::new(
            Point::new(bounds.x + plot.x, bounds.y + plot.y),
            Size::new(plot.width, plot.height),
        );
        if let Some(handle) = &image {
            renderer.with_layer(clip, |renderer| {
                renderer.draw_image(
                    image::Image::new(handle.clone()),
                    image_bounds(base, displayed, plot, bounds),
                    clip,
                )
            });
        }
        let fingerprint = match gfx_signals::cache_fingerprint(&frame, &self.theme) {
            Ok(key) => key,
            Err(_) => return,
        };
        let outside = displayed.west_deg < base.west_deg - 1e-8
            || displayed.east_deg > base.east_deg + 1e-8
            || displayed.south_deg < base.south_deg - 1e-8
            || displayed.north_deg > base.north_deg + 1e-8;
        let program = gfx_signals::Program {
            frame,
            theme: self.theme.clone(),
            fingerprint: fingerprint ^ u64::from(image.is_some()),
            raster_backed: image.is_some(),
            raster_error: error,
        };
        renderer.with_layer(bounds, |renderer| {
            renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
                for geometry in iced::widget::canvas::Program::<Message>::draw(
                    &program,
                    &state.geometry,
                    renderer,
                    theme,
                    bounds,
                    cursor,
                ) {
                    renderer.draw_geometry(geometry);
                }
                let mut footer = iced::widget::canvas::Frame::new(renderer, bounds.size());
                let text = self.theme.color(fpl_gfx::ThemeRole::TextMuted);
                footer.fill_text(iced::widget::canvas::Text {
                    content: format!(
                        "Drag to pan · Scroll to zoom · Right-click to reset{}",
                        if outside && image.is_some() {
                            " · Outside cached coverage"
                        } else {
                            ""
                        }
                    ),
                    position: Point::new(8., bounds.height - 16.),
                    size: iced::Pixels(11.),
                    color: iced::Color::from_linear_rgba(text.r, text.g, text.b, 1.),
                    ..Default::default()
                });
                renderer.draw_geometry(footer.into_geometry());
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> fpl_gfx::geo::Extent {
        fpl_gfx::geo::Extent {
            west_deg: -123.,
            east_deg: -121.,
            south_deg: 47.,
            north_deg: 49.,
        }
    }
    #[test]
    fn real_pointer_events_capture_drag_zoom_reset_and_cancel_on_focus_loss() {
        let frame = serde_json::from_value(
            serde_json::json!({"view":"geo_map","map":{"extent":base(),"markers":[],"paths":[]}}),
        )
        .unwrap();
        let widget = MapWidget {
            frame,
            theme: fpl_gfx::Theme::studio_dark(),
        };
        let bounds = Rectangle::new(Point::new(30., 80.), Size::new(1000., 700.));
        let cursor = mouse::Cursor::Available(Point::new(500., 400.));
        let mut state = State::default();
        let wheel = Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0., y: 3. },
        });
        assert!(!widget.interact(&mut state, &wheel, bounds, mouse::Cursor::Unavailable));
        assert!(widget.interact(&mut state, &wheel, bounds, cursor));
        let zoomed = state.camera.extent(base()).unwrap();
        assert!(zoomed.east_deg - zoomed.west_deg < 2.);
        assert!(widget.interact(
            &mut state,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            cursor
        ));
        assert!(widget.interact(
            &mut state,
            &Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(540., 420.)
            }),
            bounds,
            cursor
        ));
        let panned = state.camera.extent(base()).unwrap();
        assert_ne!(panned, zoomed);
        let mut tree = Tree::new(&widget as &dyn Widget<(), Theme, Renderer>);
        tree.state.downcast_mut::<State>().camera = state.camera;
        let refreshed = MapWidget {
            frame: widget.frame.clone(),
            theme: widget.theme.clone(),
        };
        tree.diff(&refreshed as &dyn Widget<(), Theme, Renderer>);
        assert_eq!(
            panned,
            tree.state
                .downcast_ref::<State>()
                .camera
                .extent(base())
                .unwrap()
        );
        widget.interact(
            &mut state,
            &Event::Window(iced::window::Event::Unfocused),
            bounds,
            cursor,
        );
        assert!(state.drag.is_none());
        assert!(widget.interact(
            &mut state,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            bounds,
            cursor
        ));
        assert_eq!(state.camera.extent(base()).unwrap(), base());
    }
    #[test]
    fn raster_transform_and_vector_projection_share_exact_registration() {
        let source = base();
        let mut camera = Camera::default();
        camera.zoom_at(source, 3., [0.2, 0.8]).unwrap();
        camera.pan(source, 0.15, -0.12).unwrap();
        let view = camera.extent(source).unwrap();
        let bounds = Rectangle::new(Point::new(30., 80.), Size::new(1000., 700.));
        let plot = Rect::new(56., 24., 930., 642.);
        let image = image_bounds(source, view, plot, bounds);
        let lon = -122.4;
        let lat = 47.6;
        let sx = plot.x as f64
            + (lon - source.west_deg) / (source.east_deg - source.west_deg) * plot.width as f64;
        let sy = plot.y as f64
            + (source.north_deg - lat) / (source.north_deg - source.south_deg) * plot.height as f64;
        let raster_x = image.x as f64 + sx * image.width as f64 / bounds.width as f64;
        let raster_y = image.y as f64 + sy * image.height as f64 / bounds.height as f64;
        let vector_x = bounds.x as f64
            + plot.x as f64
            + (lon - view.west_deg) / (view.east_deg - view.west_deg) * plot.width as f64;
        let vector_y = bounds.y as f64
            + plot.y as f64
            + (view.north_deg - lat) / (view.north_deg - view.south_deg) * plot.height as f64;
        assert!((raster_x - vector_x).abs() < 0.001);
        assert!((raster_y - vector_y).abs() < 0.001);
    }
    #[test]
    fn pointer_zoom_pan_bounds_and_reset_are_retained_independent_of_feed() {
        let source = base();
        let mut camera = Camera::default();
        camera.zoom_at(source, 2., [0.25, 0.75]).unwrap();
        let view = camera.extent(source).unwrap();
        assert!((view.west_deg + 0.25 * (view.east_deg - view.west_deg) - (-122.5)).abs() < 1e-10);
        assert!((view.north_deg - 0.75 * (view.north_deg - view.south_deg) - 47.5).abs() < 1e-10);
        camera.pan(source, 0.1, 0.2).unwrap();
        let moved = camera.extent(source).unwrap();
        assert!(moved.west_deg < view.west_deg && moved.north_deg > view.north_deg);
        assert!(camera.pan(source, f64::NAN, 0.).is_err());
        assert_eq!(moved, camera.extent(source).unwrap());
        camera.zoom_at(source, 1e9, [0.5, 0.5]).unwrap();
        assert!(
            (camera.extent(source).unwrap().east_deg
                - camera.extent(source).unwrap().west_deg
                - 0.125)
                .abs()
                < 1e-10
        );
        camera.pan(source, 1e9, -1e9).unwrap();
        camera.extent(source).unwrap().validate().unwrap();
        assert_eq!(Camera::default().extent(source).unwrap(), source);
    }
}
