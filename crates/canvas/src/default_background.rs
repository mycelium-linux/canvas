//! Static Undergrowth artwork, rendered once from fungOS' original kernel.
//! Kept outside the workspace protocol: explicit images/shaders still win.
use iced::widget::image::Handle;
use std::sync::LazyLock;

const PNG: &[u8] = include_bytes!("../../../config/appearance/undergrowth.png");

pub fn handle() -> Handle {
    static IMAGE: LazyLock<Handle> = LazyLock::new(|| Handle::from_bytes(PNG));
    IMAGE.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_wallpaper_decodes_at_expected_dimensions() {
        let image = image::load_from_memory(PNG).expect("bundled wallpaper must decode");
        assert_eq!((image.width(), image.height()), (1600, 1000));
    }
}
