//! Embeds the icon and version info into the Windows `.exe` files.
//!
//! The icon is drawn by `src/icon_art.rs` (shared with the window icon),
//! written as a multi-size `.ico`, and compiled with the Windows SDK's
//! resource compiler. If no resource compiler is found (e.g. the GNU
//! toolchain), the build continues without an exe icon and says so.

#[path = "src/icon_art.rs"]
#[allow(dead_code)]
mod icon_art;

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let ico = Path::new(&out_dir).join("comte.ico");
    if let Err(err) = write_ico(&ico) {
        println!("cargo:warning=could not write the icon: {err}");
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(&ico.to_string_lossy())
        .set("ProductName", "Comte Log Analyzer")
        .set(
            "FileDescription",
            "Comte Log Analyzer: log analyzer for game engines",
        )
        .set("LegalCopyright", "Comte Log Analyzer contributors");
    if let Err(err) = resource.compile() {
        println!("cargo:warning=no exe icon (resource compiler unavailable): {err}");
    }
}

/// An `.ico` with PNG-encoded images (supported since Windows Vista).
fn write_ico(path: &Path) -> std::io::Result<()> {
    const SIZES: [u32; 5] = [16, 24, 32, 48, 256];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| png(s)).collect::<Result<_, _>>()?;

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (size, image) in SIZES.iter().zip(&images) {
        let dim = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in &images {
        ico.extend_from_slice(image);
    }
    std::fs::write(path, ico)
}

fn png(size: u32) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&icon_art::render(size))
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)?;
    Ok(bytes)
}
