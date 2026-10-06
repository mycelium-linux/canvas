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
    if width < 2 || height < 2 || width > 8192 || height > 8192 || raw.len() > 512 * 1024 {
        return Err("Invalid signal frame extent or payload size".into());
    }
    let snapshot: SignalFrame = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let mut scene = Scene::with_capacity(1200);
    snapshot
        .append(&mut scene, Rect::new(0.0, 0.0, width as f32, height as f32))
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
    use super::*;
    #[test]
    fn console_meter_records_use_shared_needle_and_led_geometry() {
        for style in [
            fpl_gfx::console::MeterStyle::Needle,
            fpl_gfx::console::MeterStyle::Led,
        ] {
            let frame = SignalFrame::Console {
                panel: fpl_gfx::console::Console {
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
