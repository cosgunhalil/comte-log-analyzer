//! Loading and parsing logs on a background thread, so the window stays
//! responsive on large files.

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use comte_core::{
    DETECT_LINES, LogDocument, LogParser, PlainTextParser, detect_engine, parse_reader,
};

use crate::engines::engines;
use crate::view::DocIndex;

/// A parsed log, ready to display.
#[derive(Debug)]
pub struct Loaded {
    pub path: PathBuf,
    pub title: String,
    /// "Unity Editor", "Unity Player" or "Plain text".
    pub kind_label: String,
    pub doc: LogDocument,
    pub index: DocIndex,
    /// The original lines (lossy UTF-8), for the "raw lines" view.
    pub raw: Vec<String>,
    pub elapsed: Duration,
}

impl Loaded {
    /// Original lines `start..end` (1-based, half-open).
    pub fn raw_lines(&self, range: std::ops::Range<usize>) -> &[String] {
        let start = range.start.saturating_sub(1).min(self.raw.len());
        let end = range.end.saturating_sub(1).clamp(start, self.raw.len());
        &self.raw[start..end]
    }
}

/// Reads, detects and parses a log synchronously.
pub fn load(path: &Path, title: &str) -> io::Result<Loaded> {
    let started = Instant::now();
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let raw: Vec<String> = text.lines().map(str::to_owned).collect();

    let head: Vec<&str> = raw.iter().take(DETECT_LINES).map(String::as_str).collect();
    let engines = engines();
    let (mut parser, kind_label): (Box<dyn LogParser>, String) =
        match detect_engine(&engines, &head) {
            Some((engine, kind)) => (
                engine.new_parser(kind),
                format!("{} {}", engine.id(), kind.label()),
            ),
            None => (Box::new(PlainTextParser), "Plain text".to_owned()),
        };
    let doc = parse_reader(Cursor::new(&bytes), parser.as_mut())?;
    let index = DocIndex::new(&doc);
    Ok(Loaded {
        path: path.to_owned(),
        title: title.to_owned(),
        kind_label,
        doc,
        index,
        raw,
        elapsed: started.elapsed(),
    })
}

#[derive(Debug)]
pub enum LoadEvent {
    Done(Box<Loaded>),
    Failed { path: PathBuf, error: String },
}

/// Runs [`load`] on a background thread. Only the most recent request's
/// result is delivered; older ones are dropped.
#[derive(Debug)]
pub struct Loader {
    tx: Sender<(u64, LoadEvent)>,
    rx: Receiver<(u64, LoadEvent)>,
    generation: u64,
    pending: Option<PathBuf>,
}

impl Default for Loader {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            generation: 0,
            pending: None,
        }
    }
}

impl Loader {
    pub fn start(&mut self, path: PathBuf, title: String, ctx: &eframe::egui::Context) {
        self.generation += 1;
        let generation = self.generation;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.pending = Some(path.clone());
        std::thread::spawn(move || {
            let event = match load(&path, &title) {
                Ok(loaded) => LoadEvent::Done(Box::new(loaded)),
                Err(err) => LoadEvent::Failed {
                    path,
                    error: err.to_string(),
                },
            };
            let _ = tx.send((generation, event));
            ctx.request_repaint();
        });
    }

    /// The latest finished load, if any.
    pub fn poll(&mut self) -> Option<LoadEvent> {
        let mut latest = None;
        while let Ok((generation, event)) = self.rx.try_recv() {
            if generation == self.generation {
                self.pending = None;
                latest = Some(event);
            }
        }
        latest
    }

    /// The file being loaded right now.
    pub fn pending(&self) -> Option<&Path> {
        self.pending.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../comte-unity/tests/fixtures")
            .join(name)
    }

    #[test]
    fn loads_and_detects_unity_logs() {
        let editor = load(&fixture("editor_session.log"), "Editor").unwrap();
        assert_eq!(editor.kind_label, "Unity Editor");
        assert_eq!(editor.raw.len(), editor.doc.line_count());
        let player = load(&fixture("player_session.log"), "Player").unwrap();
        assert_eq!(player.kind_label, "Unity Player");
        assert_eq!(
            player.raw_lines(3..4),
            ["Initialize engine version: 2019.4.40f1 (ffc62b691db5)"]
        );
    }

    #[test]
    fn unknown_files_fall_back_to_plain_text() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let loaded = load(&path, "Cargo.toml").unwrap();
        assert_eq!(loaded.kind_label, "Plain text");
        assert!(!loaded.doc.entries().is_empty());
    }

    #[test]
    fn raw_lines_clamp_out_of_range() {
        let loaded = load(&fixture("player_session.log"), "Player").unwrap();
        assert!(loaded.raw_lines(10_000..10_005).is_empty());
        assert_eq!(loaded.raw_lines(0..2).len(), 1);
    }
}
