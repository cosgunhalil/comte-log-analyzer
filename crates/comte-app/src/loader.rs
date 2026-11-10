//! Loading and parsing logs on a background thread, so the window stays
//! responsive on large files.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use comte_core::{
    DETECT_LINES, EngineId, Fact, Insights, LogDocument, LogKind, LogParser, PlainTextParser,
    SessionHeader, detect_engine, parse_header, parse_kind_token, parse_reader,
};

const MB: u64 = 1024 * 1024;
/// Logs bigger than this are opened from the end (runaway logs reach tens of
/// gigabytes; nobody reads those from the top, and they don't fit in memory).
pub const HUGE_LOG: u64 = 512 * MB;
/// How much of the end of a huge log is read.
pub const HUGE_TAIL: u64 = 64 * MB;
/// How much of the start of a huge log is read for detection and facts.
const HUGE_HEAD: u64 = 256 * 1024;

/// Only the end of a huge log was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated {
    pub total_bytes: u64,
    /// Bytes read from the end.
    pub shown_bytes: u64,
}

/// The start and the end of a huge file. The end starts after the first
/// newline, so it holds whole lines only.
pub struct HugeParts {
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub truncated: Truncated,
}

pub fn read_huge(path: &Path, len: u64, tail_bytes: u64) -> io::Result<HugeParts> {
    let mut file = File::open(path)?;
    let mut head = Vec::new();
    (&mut file).take(HUGE_HEAD).read_to_end(&mut head)?;
    let start = len.saturating_sub(tail_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(len - start).read_to_end(&mut tail)?;
    if start > 0
        && let Some(newline) = tail.iter().position(|&b| b == b'\n')
    {
        tail.drain(..=newline);
    }
    Ok(HugeParts {
        head,
        truncated: Truncated {
            total_bytes: len,
            shown_bytes: tail.len() as u64,
        },
        tail,
    })
}

/// Facts (engine version, OS, GPU, …) from the start of a huge log, which
/// the end doesn't repeat.
pub fn head_facts(head: &[u8], parser: &mut dyn LogParser) -> Vec<Fact> {
    // Drop a last line cut in the middle.
    let end = head.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    parse_reader(Cursor::new(&head[..end]), parser)
        .map(|doc| doc.facts().to_vec())
        .unwrap_or_default()
}

/// The first lines of `bytes`, for engine detection.
pub fn head_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)])
        .lines()
        .take(DETECT_LINES)
        .map(str::to_owned)
        .collect()
}

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
    /// Set when only the end of a huge log was read; line numbers then count
    /// from the first line read.
    pub truncated: Option<Truncated>,
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
    // Dev hook: `COMTE_HUGE_KB=16` treats logs over 16 KB as huge (reading the
    // last quarter of that), to check huge-log mode without a huge file.
    match std::env::var("COMTE_HUGE_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => load_with_limit(path, title, kb * 1024, kb * 256),
        None => load_with_limit(path, title, HUGE_LOG, HUGE_TAIL),
    }
}

/// [`load`] with the huge-log limits as parameters (for tests).
pub fn load_with_limit(path: &Path, title: &str, huge: u64, tail: u64) -> io::Result<Loaded> {
    let len = std::fs::metadata(path)?.len();
    if len > huge {
        return load_huge(path, title, len, tail);
    }
    load_whole(path, title)
}

fn load_huge(path: &Path, title: &str, len: u64, tail: u64) -> io::Result<Loaded> {
    let started = Instant::now();
    let parts = read_huge(path, len, tail)?;
    let head = head_lines(&parts.head);
    let head: Vec<&str> = head.iter().map(String::as_str).collect();
    let Picked {
        mut parser,
        kind_label,
        kind,
    } = pick_parser(&head);
    let facts = head_facts(&parts.head, pick_parser(&head).parser.as_mut());
    let mut doc = parse_reader(Cursor::new(&parts.tail), parser.as_mut())?;
    for fact in facts {
        doc.apply(comte_core::ParseEvent::Fact(fact));
    }
    let raw: Vec<String> = String::from_utf8_lossy(&parts.tail)
        .lines()
        .map(str::to_owned)
        .collect();
    let index = DocIndex::new(&doc);
    let insights = Insights::of(&doc);
    Ok(Loaded {
        path: path.to_owned(),
        title: title.to_owned(),
        kind_label,
        kind,
        session: None,
        doc,
        index,
        insights,
        raw,
        elapsed: started.elapsed(),
        truncated: Some(parts.truncated),
    })
}

fn load_whole(path: &Path, title: &str) -> io::Result<Loaded> {
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
        truncated: None,
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
    fn huge_logs_are_read_from_the_end_with_facts_from_the_start() {
        let mut text = std::fs::read_to_string(fixture("editor_session.log")).unwrap();
        for i in 0..2000 {
            text.push_str(&format!("Shadow limit exceeded on frame {i}\n"));
        }
        text.push_str(
            "[Worker0] Start importing Assets/Models/Huge.fbx using Guid(02) Importer(1,2) \n",
        );
        let path = std::env::temp_dir().join(format!("comte-huge-{}.log", std::process::id()));
        std::fs::write(&path, &text).unwrap();
        let loaded = load_with_limit(&path, "Editor.log", 16 * 1024, 8 * 1024).unwrap();
        let _ = std::fs::remove_file(&path);

        let cut = loaded.truncated.expect("read from the end");
        assert_eq!(cut.total_bytes, text.len() as u64);
        assert!(cut.shown_bytes <= 8 * 1024);
        assert_eq!(loaded.kind_label, "Unity Editor", "detected from the start");
        assert!(
            loaded.raw[0].starts_with("Shadow limit exceeded on frame"),
            "starts on a whole line: {}",
            loaded.raw[0]
        );
        assert_eq!(
            loaded.doc.fact(&comte_core::FactKey::EngineVersion),
            Some("6000.3.11f1"),
            "facts come from the start of the file"
        );
        assert_eq!(
            loaded.insights.verdict.headline,
            "Stopped while importing Assets/Models/Huge.fbx"
        );
        assert!(
            loaded
                .insights
                .loudest
                .as_ref()
                .is_some_and(|l| l.headline.starts_with("Shadow limit exceeded")),
            "the spam that filled the file stands out"
        );

        let small = load(&fixture("editor_session.log"), "Editor").unwrap();
        assert!(small.truncated.is_none());
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
