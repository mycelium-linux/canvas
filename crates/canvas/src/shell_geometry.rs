//! Shared logical-pixel sizing for shell presentation and native hit targets.
pub const DOCK_HEIGHT: f32 = 72.0;
pub fn sheet_size(window_cycle: bool, count: usize, viewport: [f32; 2]) -> [f32; 2] {
    let width = if window_cycle {
        32.0 + count.clamp(1, 6) as f32 * 112.0
    } else {
        600.0
    };
    let height: f32 = if window_cycle { 152.0 } else { 440.0 };
    [
        width.min((viewport[0] - 32.0).max(1.0)),
        height.min((viewport[1] - 32.0).max(1.0)),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_not_screen_size_drives_switcher() {
        assert_eq!(sheet_size(true, 1, [1408.0, 881.0]), [144.0, 152.0]);
        assert_eq!(sheet_size(true, 3, [3840.0, 2160.0]), [368.0, 152.0]);
        assert_eq!(sheet_size(true, 90, [3840.0, 2160.0]), [704.0, 152.0]);
    }
    #[test]
    fn small_outputs_are_bounded() {
        assert_eq!(sheet_size(true, 6, [320.0, 180.0]), [288.0, 148.0]);
        assert_eq!(sheet_size(false, 0, [1.0, 1.0]), [1.0, 1.0]);
    }
}
