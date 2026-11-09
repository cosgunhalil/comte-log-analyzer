//! The window icon (the `.exe` icon is embedded by `build.rs`).

#[path = "icon_art.rs"]
mod art;

use eframe::egui::IconData;

pub fn window_icon() -> IconData {
    const SIZE: u32 = 64;
    IconData {
        rgba: art::render(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_a_transparent_corner_and_an_opaque_center() {
        let icon = window_icon();
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        let alpha = |x: usize, y: usize| icon.rgba[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "rounded corner");
        assert_eq!(alpha(32, 32), 255);
    }

    /// Writes a 256 px preview: `COMTE_ICON_PREVIEW=out.png cargo test -p comte-app -- --ignored icon`.
    #[test]
    #[ignore = "needs COMTE_ICON_PREVIEW"]
    fn write_icon_preview() {
        let path = std::env::var("COMTE_ICON_PREVIEW").expect("set COMTE_ICON_PREVIEW");
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 256, 256);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&art::render(256)).unwrap();
    }
}
