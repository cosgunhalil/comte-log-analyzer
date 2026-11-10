//! Development hooks, driven by environment variables. They let the UI be
//! checked automatically (screenshots of a given state) without a human
//! clicking around:
//!
//! - `COMTE_SCREENSHOT=out.png`: once a log is loaded, save a screenshot and quit.
//! - `COMTE_SELECT=problem`: select the first warning or error after loading;
//!   `COMTE_SELECT=verdict`: follow the overview's "Show it" link.
//! - `COMTE_SEARCH=text`: start with this search text; `COMTE_SEARCH_MODE=highlight`
//!   only highlights, `=regex` treats it as a regular expression.
//! - `COMTE_NOISE=1`: start with engine noise shown.
//! - `COMTE_OPEN_SAVE=1`: open the "Save session" dialog after loading, with
//!   `COMTE_SAVE_NOTE`, `COMTE_SAVE_TAGS` and `COMTE_SAVE_EXTRA` ("Also redact") prefilled.
//! - `COMTE_SAVE_TO=file.log`: save the session there right away (no file dialog).
//! - `COMTE_LIVE=1`: turn live tail on once the log is loaded.
//! - `COMTE_COMPARE=1`: open the comparison with the previous run after loading.
//! - `COMTE_SETTINGS_OPEN=1`: open the settings window.
//! - `COMTE_NO_DISCOVERY=1`: don't list this machine's logs (clean screenshots).
//! - `COMTE_COPY=summary|lines|message`: copy that to the clipboard after loading
//!   (`lines`/`message` use the selected entry).
//! - `COMTE_HUGE_KB=16`: treat logs over 16 KB as huge (read only their end).
//! - `COMTE_PIN=12:note here,40`: pin the entries starting on these lines after
//!   loading, with an optional note after `:`.
//! - `COMTE_PACK=1`: open the support pack dialog after loading;
//!   `COMTE_PACK_TO=pack.zip` writes the pack there right away (default options).
//! - `COMTE_REPORT=1`: open the report dialog after loading (`=slack` for Slack style).
//! - `COMTE_IDE_DRY_RUN=1`: print the editor command instead of starting it;
//!   `COMTE_OPEN=1` opens the selected entry's location after loading.
//! - `COMTE_SCREENSHOT_DELAY_MS=4000`: wait this long after loading before the screenshot.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ColorImage};

#[derive(Debug, Default)]
pub struct DevHooks {
    screenshot: Option<PathBuf>,
    loaded_at: Option<std::time::Instant>,
    requested: bool,
    done: Arc<AtomicBool>,
    pub select_problem: bool,
    pub select_verdict: bool,
    pub search: Option<String>,
    pub search_mode: Option<String>,
    pub show_noise: bool,
    pub open_save: bool,
    pub save_note: Option<String>,
    pub save_tags: Option<String>,
    pub save_extra: Option<String>,
    pub save_to: Option<PathBuf>,
    pub live: bool,
    pub compare: bool,
    pub settings_open: bool,
    pub no_discovery: bool,
    pub copy: Option<String>,
    pub pins: Vec<(usize, String)>,
    pub report: bool,
    pub pack: bool,
    pub pack_to: Option<PathBuf>,
    pub report_slack: bool,
    pub ide_dry_run: bool,
    pub open_selected: bool,
    screenshot_delay: std::time::Duration,
}

impl DevHooks {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            screenshot: var("COMTE_SCREENSHOT").map(PathBuf::from),
            select_problem: var("COMTE_SELECT").as_deref() == Some("problem"),
            select_verdict: var("COMTE_SELECT").as_deref() == Some("verdict"),
            search: var("COMTE_SEARCH"),
            search_mode: var("COMTE_SEARCH_MODE"),
            show_noise: var("COMTE_NOISE").as_deref() == Some("1"),
            open_save: var("COMTE_OPEN_SAVE").as_deref() == Some("1")
                || var("COMTE_SAVE_TO").is_some(),
            // A literal `\n` in the variable starts a new note line.
            save_note: var("COMTE_SAVE_NOTE").map(|n| n.replace("\\n", "\n")),
            save_tags: var("COMTE_SAVE_TAGS"),
            save_extra: var("COMTE_SAVE_EXTRA"),
            save_to: var("COMTE_SAVE_TO").map(PathBuf::from),
            live: var("COMTE_LIVE").as_deref() == Some("1"),
            compare: var("COMTE_COMPARE").as_deref() == Some("1"),
            settings_open: var("COMTE_SETTINGS_OPEN").as_deref() == Some("1"),
            no_discovery: var("COMTE_NO_DISCOVERY").as_deref() == Some("1"),
            copy: var("COMTE_COPY"),
            pins: var("COMTE_PIN")
                .map(|v| {
                    v.split(',')
                        .filter_map(|pin| {
                            let (line, note) = pin.split_once(':').unwrap_or((pin, ""));
                            Some((line.trim().parse().ok()?, note.trim().to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            report: var("COMTE_REPORT").is_some(),
            pack: var("COMTE_PACK").as_deref() == Some("1"),
            pack_to: var("COMTE_PACK_TO").map(PathBuf::from),
            report_slack: var("COMTE_REPORT").as_deref() == Some("slack"),
            ide_dry_run: var("COMTE_IDE_DRY_RUN").as_deref() == Some("1"),
            open_selected: var("COMTE_OPEN").as_deref() == Some("1"),
            screenshot_delay: std::time::Duration::from_millis(
                var("COMTE_SCREENSHOT_DELAY_MS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1200),
            ),
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
        let loaded_at = *self.loaded_at.get_or_insert_with(std::time::Instant::now);
        // Let layout settle and animations (dialog fade-in) finish.
        if loaded_at.elapsed() >= self.screenshot_delay && !self.requested {
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
