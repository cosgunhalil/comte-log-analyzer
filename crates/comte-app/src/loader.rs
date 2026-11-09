//! Loading and parsing logs on a background thread, so the window stays
//! responsive on large files.

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use comte_core::{
    DETECT_LINES, EngineId, Insights, LogDocument, LogKind, LogParser, PlainTextParser,
    SessionHeader, detect_engine, parse_header, parse_kind_token, parse_reader,
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
    /// Engine and log kind, when an engine recognized the log.
    pub kind: Option<(EngineId, LogKind)>,
    /// Header of a saved session (note, tags, …) when this file is one.
    pub session: Option<SessionHeader>,
    pub doc: LogDocument,
    pub index: DocIndex,
    /// Verdict, repeats and timings, computed once per (re)load.
    pub insights: Insights,
    /// The log's lines (lossy UTF-8) without any session header; line `n`
    /// of the document is `raw[n - 1]`.
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

/// The parser for a log, and how to describe it.
pub struct Picked {
    pub parser: Box<dyn LogParser>,
    pub kind_label: String,
    pub kind: Option<(EngineId, LogKind)>,
}

fn picked(detected: Option<(&dyn comte_core::EngineSupport, LogKind)>) -> Picked {
    match detected {
        Some((engine, kind)) => Picked {
            parser: engine.new_parser(kind),
            kind_label: format!("{} {}", engine.id(), kind.label()),
            kind: Some((engine.id(), kind)),
        },
        None => Picked {
            parser: Box::new(PlainTextParser),
            kind_label: "Plain text".to_owned(),
            kind: None,
        },
    }
}

/// Detects the engine and kind from a log's first lines.
pub fn pick_parser(head: &[&str]) -> Picked {
    let engines = engines();
    picked(detect_engine(&engines, head))
}

/// Reads, detects and parses a log (or a saved session) synchronously.
pub fn load(path: &Path, title: &str) -> io::Result<Loaded> {
    let started = Instant::now();
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut raw: Vec<String> = text.lines().map(str::to_owned).collect();

    // A saved session: drop its header; the body is an ordinary log.
    let session = parse_header(&raw).map(|(header, body_start)| {
        raw.drain(..body_start);
        header
    });

    let engines = engines();
    // A session says which parser reads its body; anything else is detected.
    let from_header = session
        .as_ref()
        .and_then(|s| parse_kind_token(&s.kind))
        .and_then(|(id, kind)| {
            engines
                .iter()
                .find(|e| e.id() == id)
                .map(|e| (e.as_ref(), kind))
        });
    let head: Vec<&str> = raw.iter().take(DETECT_LINES).map(String::as_str).collect();
    let detected = from_header.or_else(|| detect_engine(&engines, &head));
    let Picked {
        mut parser,
        kind_label,
        kind,
    } = picked(detected);

    let doc = if session.is_some() {
        let body = raw.join("\n");
        parse_reader(Cursor::new(body.as_bytes()), parser.as_mut())?
    } else {
        parse_reader(Cursor::new(&bytes), parser.as_mut())?
    };
    let index = DocIndex::new(&doc);
    let insights = Insights::of(&doc);
    let kind_label = if session.is_some() {
        format!("{kind_label} · saved session")
    } else {
        kind_label
    };
    Ok(Loaded {
        path: path.to_owned(),
        title: title.to_owned(),
        kind_label,
        kind,
        session,
        doc,
        index,
        insights,
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
