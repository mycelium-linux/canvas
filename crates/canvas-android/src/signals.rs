//! Thin Android paint adapter for the canonical gfx signal geometry.
use fpl_gfx::{PrimitiveKind, Rect, Scene, signals::SignalFrame};

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
