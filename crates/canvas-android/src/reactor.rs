//! Android raster records from shared GFX geometry, not a second visual design.
use fpl_gfx::reactor::{Activity, Mark, Paint};
/// Shared native-popup chrome delivered to the platform drawing adapter.
/// This is presentation metadata, never a workspace or execution command.
pub fn popup_descriptor(activity: Activity, caption: Option<&str>) -> serde_json::Value {
    use fpl_gfx::voice_popup as popup;
    serde_json::json!({
        "size": popup::SIZE, "radius": popup::RADIUS,
        "reactor_center": popup::REACTOR_CENTER, "reactor_width": popup::REACTOR_WIDTH,
        "text_rect": popup::TEXT_RECT, "font_size": popup::FONT_SIZE,
        "surface": popup::SURFACE, "border": popup::BORDER,
        "text": popup::TEXT, "ink": popup::INK, "accent": popup::accent(activity),
        "label": popup::label(activity, caption),
        "linger_seconds": popup::LINGER_SECONDS,
        "reveal_seconds": popup::REVEAL_SECONDS,
        "dismiss_seconds": popup::DISMISS_SECONDS, "slide": popup::SLIDE
    })
}
/// Nine floats per record: kind, x, y, radius/width, stroke/height,
/// start degrees, sweep degrees, semantic ink, opacity.
pub fn records(activity: Activity, seconds: f32, level: Option<u16>) -> Vec<f32> {
    let mut out = Vec::with_capacity(fpl_gfx::reactor::MAX_MARKS * 9);
    fpl_gfx::reactor::visit_with_level(
        activity,
        seconds,
        level.map(|v| f32::from(v) / 1000.0),
        |m| {
            let [x, y] = fpl_gfx::reactor::CENTER;
            let record = match m {
                Mark::Disc {
                    radius,
                    paint,
                    alpha,
                } => [
                    1.0,
                    x,
                    y,
                    radius,
                    0.0,
                    0.0,
                    0.0,
                    match paint {
                        Paint::Surface => 0.0,
                        Paint::Accent => 1.0,
                        Paint::Ink => 2.0,
                    },
                    alpha,
                ],
                Mark::Ring {
                    radius,
                    width,
                    alpha,
                } => [2.0, x, y, radius, width, 0.0, 360.0, 1.0, alpha],
                Mark::Arc {
                    radius,
                    width,
                    start,
                    end,
                    alpha,
                } => [
                    3.0,
                    x,
                    y,
                    radius,
                    width,
                    start.to_degrees(),
                    (end - start).to_degrees(),
                    1.0,
                    alpha,
                ],
                Mark::Bar {
                    x,
                    height,
                    width,
                    alpha,
                } => [
                    4.0,
                    88.0 + x,
                    83.0 - height / 2.0,
                    width,
                    height,
                    0.0,
                    0.0,
                    2.0,
                    alpha,
                ],
            };
            out.extend_from_slice(&record);
        },
    );
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_are_bounded_and_hidden_is_empty() {
        assert!(records(Activity::Hidden, 0.0, None).is_empty());
        for activity in [Activity::Resting, Activity::Thinking, Activity::Speaking] {
            let r = records(activity, 1.0, Some(0));
            assert!(r.len() <= 38 * 9 && r.len() % 9 == 0 && r.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn popup_descriptor_is_the_shared_native_contract_and_never_rewrites_asr() {
        let value = popup_descriptor(Activity::Thinking, Some("Jarvis, opened Safari"));
        assert_eq!(value["size"], serde_json::json!([340.0, 54.0]));
        assert_eq!(value["label"], "Jarvis, opened Safari");
        assert_eq!(value["radius"], 27.0);
        assert!(value.get("diagnostic").is_none());
    }
}
