//! Bounded local animation host; dynamics and line geometry belong to gfx.
use fpl_gfx::{Primitive, Rect, Scene, ThemeRole, boids::Flock};

pub const COUNT: usize = 64;
#[derive(Default)]
pub struct Runtime {
    flock: Option<Flock>,
    aspect: f32,
    last: f32,
    aquarium: bool,
}
impl Runtime {
    pub fn frame(
        &mut self,
        width: u32,
        height: u32,
        elapsed: f32,
        aquarium: bool,
    ) -> Result<Vec<f32>, &'static str> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || !elapsed.is_finite()
            || elapsed < 0.0
        {
            return Err("Invalid flock viewport or clock");
        }
        let inset = (height as f32 * 0.025).min(32.0);
        let viewport = Rect::new(
            inset,
            inset,
            width as f32 - 2.0 * inset,
            height as f32 - 2.0 * inset,
        );
        let aspect = viewport.width / viewport.height;
        if self.flock.is_none()
            || (aspect - self.aspect).abs() > 0.001
            || elapsed < self.last
            || aquarium != self.aquarium
        {
            self.flock = Some(if aquarium {
                Flock::aquarium(COUNT, aspect)?
            } else {
                Flock::bounded(COUNT, aspect)?
            });
            self.aquarium = aquarium;
            self.aspect = aspect;
            self.last = elapsed;
        }
        let dt = (elapsed - self.last).clamp(0.0, 1.0 / 15.0);
        self.last = elapsed;
        let flock = self.flock.as_mut().ok_or("Flock unavailable")?;
        flock.step(dt)?;
        let mut scene = Scene::with_capacity(COUNT * 2);
        flock.append(&mut scene, viewport, ThemeRole::Primary);
        {
            let x = viewport.x;
            let y = viewport.y;
            let r = x + viewport.width;
            let b = y + viewport.height;
            for (start, end) in [
                ([x, y], [r, y]),
                ([r, y], [r, b]),
                ([r, b], [x, b]),
                ([x, b], [x, y]),
            ] {
                scene.push(Primitive::line(start, end, 2.0, ThemeRole::TextMuted));
            }
        }
        let mut records = crate::clock::paint_scene(&scene)?;
        if aquarium && flock.catch_flash() > 0.0 {
            records.extend_from_slice(&[9.0, flock.catch_flash(), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_finite_bounded_and_resume_without_catchup() {
        let mut host = Runtime::default();
        let first = host.frame(1920, 1080, 0.0, false).unwrap();
        let next = host.frame(1920, 1080, 0.033, false).unwrap();
        assert_eq!(next.len(), (COUNT * 2 + 4) * 8);
        assert!(next.iter().all(|x| x.is_finite()));
        assert_ne!(first, next);
        assert!(host.frame(1920, 1080, 10000.0, false).is_ok());
        assert_eq!(host.frame(1920, 1080, 0.0, false).unwrap(), first);
        assert!(host.frame(0, 1080, 0.0, false).is_err());
        assert!(host.frame(1920, 1080, f32::NAN, false).is_err());
        let aquarium = host.frame(1920, 1080, 0.0, true).unwrap();
        assert_eq!(aquarium.len(), (COUNT * 2 + 9 + 4) * 8);
        assert!(aquarium.iter().all(|x| x.is_finite()));
        assert_eq!(host.frame(1920, 1080, 0.0, false).unwrap(), first);
    }
}
