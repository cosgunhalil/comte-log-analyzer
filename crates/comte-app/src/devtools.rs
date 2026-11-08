//! Development hooks, driven by environment variables. They let the UI be
//! checked automatically (screenshots of a given state) without a human
//! clicking around:
//!
//! - `COMTE_SCREENSHOT=out.png`: once a log is loaded, save a screenshot and quit.
//! - `COMTE_SELECT=problem`: select the first warning or error after loading.
//! - `COMTE_SEARCH=text`: start with this search text.
//! - `COMTE_NOISE=1`: start with engine noise shown.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ColorImage};

#[derive(Debug, Default)]
pub struct DevHooks {
    screenshot: Option<PathBuf>,
    frames_since_load: u32,
    requested: bool,
    done: Arc<AtomicBool>,
    pub select_problem: bool,
    pub search: Option<String>,
    pub show_noise: bool,
}

impl DevHooks {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            screenshot: var("COMTE_SCREENSHOT").map(PathBuf::from),
            select_problem: var("COMTE_SELECT").as_deref() == Some("problem"),
            search: var("COMTE_SEARCH"),
            show_noise: var("COMTE_NOISE").as_deref() == Some("1"),
            ..Self::default()
        }
    }

    /// Call once per frame after the UI is drawn.
    pub fn after_frame(&mut self, ctx: &egui::Context, loaded: bool) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        if self.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.request_repaint();
        if !loaded {
            return;
        }
        self.frames_since_load += 1;
        // A few frames let layout settle (panel sizes, scroll positions).
        if self.frames_since_load >= 5 && !self.requested {
            self.requested = true;
            let done = Arc::clone(&self.done);
            ctx.request_screenshot(move |image| {
                if let Err(err) = save_png(&path, &image) {
                    eprintln!("screenshot failed: {err}");
                }
                done.store(true, Ordering::SeqCst);
            });
        }
    }
}

fn save_png(path: &PathBuf, image: &ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let [width, height] = image.size;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        u32::try_from(width)?,
        u32::try_from(height)?,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(image.as_raw())?;
    Ok(())
}
