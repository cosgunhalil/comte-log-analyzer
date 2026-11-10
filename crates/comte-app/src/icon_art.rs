//! The Comte Log Analyzer icon, drawn in code: a rounded indigo tile with four
//! "log lines". Each line has a severity dot (info, info, warning, error).
//!
//! Shared by the app (window icon) and `build.rs` (the `.exe` icon), so there
//! is no binary image file to keep in sync.

/// RGBA pixels of the icon at `size`×`size`, anti-aliased.
pub fn render(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // Pixel center in unit space.
            let p = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let mut color = [0.0f32; 4];

            // Background tile with a vertical gradient.
            let tile = rounded_box(p, (0.5, 0.5), (0.46, 0.46), 0.2);
            let top = [0.22, 0.24, 0.58];
            let bottom = [0.10, 0.11, 0.30];
            let t = p.1;
            let bg = [
                top[0] + (bottom[0] - top[0]) * t,
                top[1] + (bottom[1] - top[1]) * t,
                top[2] + (bottom[2] - top[2]) * t,
            ];
            blend(&mut color, bg, coverage(tile, s));

            // Log lines: (y, bar end, dot color, bar color).
            let gray_dot = [0.67, 0.70, 0.80];
            let gray_bar = [0.80, 0.82, 0.90];
            let rows = [
                (0.30, 0.78, gray_dot, gray_bar),
                (0.43, 0.64, gray_dot, gray_bar),
                (0.56, 0.74, [0.98, 0.72, 0.24], [0.98, 0.78, 0.40]),
                (0.69, 0.58, [0.95, 0.36, 0.36], [0.98, 0.50, 0.46]),
            ];
            for (row_y, end, dot, bar) in rows {
                let d_dot = ((p.0 - 0.23).powi(2) + (p.1 - row_y).powi(2)).sqrt() - 0.045;
                blend(&mut color, dot, coverage(d_dot, s));
                let start = 0.32;
                let half_w = (end - start) / 2.0;
                let d_bar = rounded_box(p, (start + half_w, row_y), (half_w, 0.032), 0.032);
                blend(&mut color, bar, coverage(d_bar, s));
            }

            let i = ((y * size + x) * 4) as usize;
            for c in 0..4 {
                pixels[i + c] = (color[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    pixels
}

/// Signed distance from `p` to a rounded box (negative inside).
fn rounded_box(p: (f32, f32), center: (f32, f32), half: (f32, f32), radius: f32) -> f32 {
    let qx = (p.0 - center.0).abs() - (half.0 - radius);
    let qy = (p.1 - center.1).abs() - (half.1 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// How much of a pixel a shape covers, from its signed distance (1 px blur).
fn coverage(distance: f32, size: f32) -> f32 {
    (0.5 - distance * size).clamp(0.0, 1.0)
}

/// Paints `rgb` with opacity `alpha` over `dst` (straight alpha).
fn blend(dst: &mut [f32; 4], rgb: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let out_a = alpha + dst[3] * (1.0 - alpha);
    for c in 0..3 {
        dst[c] = (rgb[c] * alpha + dst[c] * dst[3] * (1.0 - alpha)) / out_a.max(f32::EPSILON);
    }
    dst[3] = out_a;
}
