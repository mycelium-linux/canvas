//! Thin Android paint adapter for the canonical gfx signal geometry.
use fpl_gfx::{PrimitiveKind, Rect, Scene, signals::SignalFrame};

pub fn label_records(width: u32, height: u32, raw: &str) -> Result<String, String> {
    if width < 2 || height < 2 || width > 8192 || height > 8192 || raw.len() > 512 * 1024 {
        return Err("Invalid signal labels extent or payload size".into());
    }
    let snapshot: SignalFrame = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let labels = snapshot
        .labels(Rect::new(0.0, 0.0, width as f32, height as f32))
        .map_err(|e| e.to_string())?;
    Ok(serde_json::Value::Array(
        labels
            .into_iter()
            .map(|label| {
                serde_json::json!({
                    "text":label.text,"bounds":label.bounds,"role":label.role as u32
                })
            })
            .collect(),
    )
    .to_string())
}

pub fn paint_records(width: u32, height: u32, raw: &str) -> Result<Vec<f32>, String> {
    paint_records_with_appearance(width, height, raw, "{}")
}

/// Host theme tokens stay separate from the provider's observed signal snapshot.
pub fn paint_records_with_appearance(
    width: u32,
    height: u32,
    raw: &str,
    appearance: &str,
) -> Result<Vec<f32>, String> {
    if width < 2 || height < 2 || width > 8192 || height > 8192 || raw.len() > 512 * 1024 {
        return Err("Invalid signal frame extent or payload size".into());
    }
    let snapshot: SignalFrame = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if appearance.len() > 8192 {
        return Err("Console appearance exceeds limit".into());
    }
    let appearance: fpl_gfx::console::ConsoleAppearance =
        serde_json::from_str(appearance).map_err(|e| format!("Invalid console appearance: {e}"))?;
    appearance.validate().map_err(|e| e.to_string())?;
    let mut scene = Scene::with_capacity(1200);
    let bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
    match snapshot {
        SignalFrame::Console { panel } => {
            panel.append_with_appearance(&mut scene, bounds, &appearance)
        }
        frame => frame.append(&mut scene, bounds),
    }
    .map_err(|e| e.to_string())?;
    let mut records = Vec::with_capacity(scene.len() * 8);
    for p in scene.iter() {
        match p.kind {
            PrimitiveKind::RoundedRect => records.extend_from_slice(&[
                0.0,
                p.rect.x,
                p.rect.y,
                p.rect.width,
                p.rect.height,
                p.radius,
                p.fill as u32 as f32,
                0.0,
            ]),
            PrimitiveKind::Disc | PrimitiveKind::Lamp => {
                // Existing Android records can paint circles as round rectangles.
                // Preserve unknown lamps as recessed wells, never invented activity.
                let side = p.rect.width.min(p.rect.height);
                let center = p.rect.center();
                records.extend_from_slice(&[
                    0.0,
                    center.x - side * 0.5,
                    center.y - side * 0.5,
                    side,
                    side,
                    side * 0.5,
                    p.fill as u32 as f32,
                    0.0,
                ]);
                if p.kind == PrimitiveKind::Lamp && p.value > 0.0 {
                    let inset = side * 0.15;
                    records.extend_from_slice(&[
                        0.0,
                        center.x - side * 0.5 + inset,
                        center.y - side * 0.5 + inset,
                        side - inset * 2.0,
                        side - inset * 2.0,
                        side * 0.35,
                        p.accent as u32 as f32,
                        0.0,
                    ]);
                }
            }
            PrimitiveKind::Line => {
                let center = p.rect.center();
                let dx = p.rotation.cos() * p.value;
                let dy = p.rotation.sin() * p.value;
                records.extend_from_slice(&[
                    5.0,
                    center.x - dx,
                    center.y - dy,
                    center.x + dx,
                    center.y + dy,
                    p.radius * 2.0,
                    p.fill as u32 as f32,
                    0.0,
                ]);
            }
            _ => return Err("Unsupported signal primitive".into()),
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    #[test]
    fn plot_uses_shared_axes_legend_and_theme_geometry() {
        let raw = r#"{"view":"plot","plot":{"x":{"label":"Time","unit":"s","minimum":0,"maximum":1},"y":{"label":"Voltage","unit":"V","minimum":0,"maximum":5},"series":[{"id":"output","label":"U1 pin3 output","role":"primary","points":[{"x":0,"y":0},{"x":0.5,"y":3.66},{"x":1,"y":0}]}],"viewport":null}}"#;
        let records = super::paint_records(640, 240, raw).unwrap();
        assert_eq!(records.len() % 8, 0);
        assert!(records.iter().all(|v| v.is_finite()));
        let labels = super::label_records(640, 240, raw).unwrap();
        assert!(
            labels.contains("Time")
                && labels.contains("Voltage")
                && labels.contains("U1 pin3 output")
        );
        let bad = raw.replace("\"maximum\":1", "\"maximum\":0");
        assert!(super::paint_records(640, 240, &bad).is_err());
    }
    #[test]
    fn android_signal_freshness_never_dims_the_instrument() {
        // Contract check for the Java paint shim: freshness stays in its footer,
        // while provider-safe frames (including stale em dashes) remain readable.
        let source = include_str!("../android/dev/fpl/canvas/TvActivity.java");
        let signal = source.split("private final class SignalView").nth(1).unwrap()
            .split("/** Generic themed primitive").next().unwrap();
        assert!(!signal.contains("setAlpha(state"));
        assert_eq!(signal.matches("ink.setAlpha(255)").count(), 3);
        assert!(signal.contains("footer.setText(label)"));
        assert!(signal.contains("frame.getJSONObject(\"panel\").put(\"fresh\", false)"));
    }
    use super::*;
    #[test]
    fn console_meter_records_use_shared_needle_and_led_geometry() {
        for style in [
            fpl_gfx::console::MeterStyle::Needle,
            fpl_gfx::console::MeterStyle::Led,
        ] {
            let frame = SignalFrame::Console {
                panel: fpl_gfx::console::Console {
                    icon: None,
                    strips: vec![fpl_gfx::console::Strip {
                        show_meter: true,
                        name: "L".into(),
                        detail: "Monitor output".into(),
                        state: fpl_gfx::topology::State::Connected,
                        peak_dbfs: Some(-12.0),
                        gain_db: None,
                    }],
                    columns: true,
                    meter: fpl_gfx::console::MeterConfig {
                        style,
                        ..Default::default()
                    },
                },
            };
            let raw = serde_json::to_string(&frame).unwrap();
            let records = paint_records(400, 240, &raw).unwrap();
            assert_eq!(records.len() % 8, 0);
            assert!(records.len() < 800);
            assert!(label_records(400, 240, &raw).unwrap().contains("dBFS"));
            let flat = serde_json::to_string(&fpl_gfx::console::ConsoleAppearance::flat()).unwrap();
            let flat_records = paint_records_with_appearance(400, 240, &raw, &flat).unwrap();
            assert!(flat_records.len() < records.len());
            assert!(paint_records_with_appearance(400, 240, &raw, r#"{"grain":1}"#).is_err());
            assert!(
                paint_records_with_appearance(400, 240, &raw, r#"{"unrecognized":true}"#).is_err()
            );
        }
    }
    #[test]
    fn topology_uses_shared_cards_cables_and_label_anchors() {
        let raw = r#"{"view":"topology","graph":{"nodes":[{"id":"a","label":"Keys","lane":0,"state":"connected","level":null},{"id":"b","label":"Synth","lane":1,"state":"configured","level":0.5}],"edges":[{"source":"a","target":"b","state":"configured"}]}}"#;
        let records = paint_records(800, 400, raw).unwrap();
        assert_eq!(records.len() % 8, 0);
        assert!(records.chunks_exact(8).any(|r| r[0] == 5.0));
        let labels: serde_json::Value =
            serde_json::from_str(&label_records(800, 400, raw).unwrap()).unwrap();
        assert_eq!(labels.as_array().unwrap().len(), 4);
        assert_eq!(labels[0]["text"], "Keys");
        assert!(
            labels[0]["bounds"]["x"].as_f64().unwrap() < labels[2]["bounds"]["x"].as_f64().unwrap()
        );
    }
    #[test]
    fn native_records_are_shared_geometry() {
        let raw = r#"{"view":"scope","range":{"minimum":-1,"maximum":1},"samples":[0,1,0]}"#;
        let records = paint_records(400, 200, raw).unwrap();
        assert_eq!(records.len(), 24);
        assert_eq!(records[0], 0.0);
        assert_eq!(records[8], 5.0);
        assert!(paint_records(0, 200, raw).is_err());
        assert!(
            paint_records(
                400,
                200,
                r#"{"view":"scope","range":{"minimum":1,"maximum":0},"samples":[]}"#
            )
            .is_err()
        );
    }
}
