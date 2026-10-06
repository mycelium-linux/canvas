//! Android paint records derived from the same canonical gfx clock as desktop.
use fpl_gfx::{AnalogClock, PrimitiveKind, Rect, Scene, ThemeRole};

/// Eight floats per record: kind, geometry, stroke width and semantic color roles.
pub fn paint_records(
    width: u32,
    height: u32,
    hour: u8,
    minute: u8,
    second: f32,
) -> Result<Vec<f32>, &'static str> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || hour > 23
        || minute > 59
        || !second.is_finite()
        || !(0.0..60.0).contains(&second)
    {
        return Err("Invalid clock frame");
    }
    let mut scene = Scene::with_capacity(65);
    AnalogClock {
        hour,
        minute,
        second,
    }
    .append(&mut scene, Rect::new(0.0, 0.0, width as f32, height as f32));
    let role = |role| match role {
        ThemeRole::Surface => Ok(0.0),
        ThemeRole::TextMuted => Ok(1.0),
        ThemeRole::Text => Ok(2.0),
        ThemeRole::Primary => Ok(3.0),
        _ => Err("Unsupported clock ink"),
    };
    let mut records = Vec::with_capacity(65 * 8);
    for p in scene.iter() {
        let center = [
            p.rect.x + p.rect.width * 0.5,
            p.rect.y + p.rect.height * 0.5,
        ];
        match p.kind {
            PrimitiveKind::Disc => records.extend_from_slice(&[
                1.0,
                center[0],
                center[1],
                p.rect.width.min(p.rect.height) * 0.5,
                0.0,
                p.outline_width,
                role(p.fill)?,
                if p.outline_width > 0.0 {
                    role(p.outline)?
                } else {
                    0.0
                },
            ]),
            PrimitiveKind::Line => {
                let dx = p.rotation.cos() * p.value;
                let dy = p.rotation.sin() * p.value;
                records.extend_from_slice(&[
                    5.0,
                    center[0] - dx,
                    center[1] - dy,
                    center[0] + dx,
                    center[1] + dy,
                    p.radius * 2.0,
                    role(p.fill)?,
                    0.0,
                ]);
            }
            _ => return Err("Unsupported clock primitive"),
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_scene_keeps_geometry_and_theme_roles() {
        let records = paint_records(400, 300, 3, 0, 0.0).unwrap();
        assert_eq!(records.len(), 65 * 8);
        assert_eq!(&records[..3], &[1.0, 200.0, 150.0]);
        assert!((records[3] - 117.0).abs() < 0.001);
        let hour = &records[61 * 8..62 * 8];
        assert_eq!(hour[0], 5.0);
        assert!((hour[3] - 256.16).abs() < 0.01);
        assert!((hour[4] - 150.0).abs() < 0.01);
        assert_eq!(hour[6], 2.0);
        assert_eq!(records[63 * 8 + 6], 3.0);
    }
    #[test]
    fn invalid_time_and_extents_fail() {
        assert!(paint_records(0, 300, 1, 0, 0.0).is_err());
        assert!(paint_records(400, 300, 24, 0, 0.0).is_err());
        assert!(paint_records(400, 300, 1, 60, 0.0).is_err());
        assert!(paint_records(400, 300, 1, 0, f32::NAN).is_err());
    }
}
