//! Smithay owns pointer routing/grab lifetime; Canvas owns window geometry.
use super::*;
use crate::window_drag::WindowDrag;
use smithay::input::pointer::*;

pub(super) struct NativeWindowGrab {
    pub id: String,
    pub start_data: GrabStartData<App>,
    pub geometry: WindowDrag,
}

// These events still traverse Smithay while client pointer focus is cleared.
// No desktop gesture interpretation or separate input owner is introduced.
macro_rules! forward_event {
    ($method:ident, $event:ty) => {
        fn $method(
            &mut self,
            data: &mut App,
            handle: &mut PointerInnerHandle<'_, App>,
            event: &$event,
        ) {
            handle.$method(data, event);
        }
    };
}

impl PointerGrab<App> for NativeWindowGrab {
    fn motion(
        &mut self,
        data: &mut App,
        handle: &mut PointerInnerHandle<'_, App>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !data.seat_active || !data.canvas.is_mapped(&self.id) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let Some(top) = data
            .toplevel_map
            .iter()
            .find(|(_, id)| id == &self.id)
            .map(|(top, _)| top.clone())
        else {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        let (min, max) = smithay::wayland::compositor::with_states(top.wl_surface(), |states| {
            let mut cached = states
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let state = cached.current();
            (
                (state.min_size.w, state.min_size.h),
                (state.max_size.w, state.max_size.h),
            )
        });
        let frame = self
            .geometry
            .frame_at((event.location.x, event.location.y), min, max);
        if data
            .canvas
            .window(&self.id)
            .is_some_and(|window| window.frame == frame)
        {
            return;
        }
        if let Err(error) = data.canvas.place_window(&self.id, frame) {
            eprintln!("native window drag: {error}");
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        data.configure_toplevel(&top, &self.id);
    }

    fn relative_motion(
        &mut self,
        data: &mut App,
        handle: &mut PointerInnerHandle<'_, App>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut App,
        handle: &mut PointerInnerHandle<'_, App>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if event.button == self.start_data.button && event.state == ButtonState::Released {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut App,
        handle: &mut PointerInnerHandle<'_, App>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }
    fn frame(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>) {
        handle.frame(data);
    }
    forward_event!(gesture_swipe_begin, GestureSwipeBeginEvent);
    forward_event!(gesture_swipe_update, GestureSwipeUpdateEvent);
    forward_event!(gesture_swipe_end, GestureSwipeEndEvent);
    forward_event!(gesture_pinch_begin, GesturePinchBeginEvent);
    forward_event!(gesture_pinch_update, GesturePinchUpdateEvent);
    forward_event!(gesture_pinch_end, GesturePinchEndEvent);
    forward_event!(gesture_hold_begin, GestureHoldBeginEvent);
    forward_event!(gesture_hold_end, GestureHoldEndEvent);

    fn start_data(&self) -> &GrabStartData<App> {
        &self.start_data
    }
    fn unset(&mut self, data: &mut App) {
        data.finish_native_interaction(&self.id);
    }
}
