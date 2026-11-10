//! Live tail (DESIGN §2): follow a log while Unity writes it.
//!
//! A background thread checks the file every [`POLL_INTERVAL`], reads only
//! the bytes appended since the last check, and sends a fresh snapshot to the
//! UI. It polls file metadata instead of using OS change notifications:
//! a `stat` is nearly free, it works the same on every file system, and it
//! notices when Unity replaces `Editor.log` on restart (notification APIs can
//! report that late or not at all while the file is renamed).
//!
//! A restart is detected when the file got shorter or its first bytes
//! changed. The log is then read again from the start.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use comte_core::{DETECT_LINES, Insights, TailDocument};
use eframe::egui;

use crate::loader::{
    HUGE_LOG, HUGE_TAIL, Loaded, Picked, Truncated, head_facts, head_lines, pick_parser, read_huge,
};
use crate::view::DocIndex;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Bytes compared at the start of the file to notice it was replaced.
const PREFIX_BYTES: usize = 512;

#[derive(Debug)]
pub enum LiveEvent {
    /// New lines arrived (`added` complete lines since the previous event).
    Update {
        loaded: Box<Loaded>,
        added: usize,
    },
    /// The file was replaced or truncated (Unity restarted); this is the new log.
    Restarted(Box<Loaded>),
    Failed(String),
}

/// Follows one file until dropped.
#[derive(Debug)]
pub struct LiveTail {
    stop: Arc<AtomicBool>,
    rx: Receiver<LiveEvent>,
}

impl LiveTail {
    pub fn start(path: PathBuf, title: String, ctx: &egui::Context) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let thread_stop = Arc::clone(&stop);
        let thread_path = path;
        std::thread::spawn(move || run(thread_path, title, &thread_stop, &tx, &ctx));
        Self { stop, rx }
    }

    /// All events since the last call, oldest first.
    pub fn poll(&self) -> Vec<LiveEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for LiveTail {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn run(
    path: PathBuf,
    title: String,
    stop: &AtomicBool,
    tx: &Sender<LiveEvent>,
    ctx: &egui::Context,
) {
    let send = |event: LiveEvent| {
        let ok = tx.send(event).is_ok();
        ctx.request_repaint();
        ok
    };
    let mut state = match TailState::open(&path) {
        Ok(state) => state,
        Err(err) => {
            send(LiveEvent::Failed(err.to_string()));
            return;
        }
    };
    if !send(LiveEvent::Update {
        loaded: Box::new(state.to_loaded(&path, &title)),
        added: 0,
    }) {
        return;
    }

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(POLL_INTERVAL);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let event = match state.check(&path) {
            Ok(Change::None) => continue,
            Ok(Change::Appended(added)) => LiveEvent::Update {
                loaded: Box::new(state.to_loaded(&path, &title)),
                added,
            },
            Ok(Change::Restarted(new_state)) => {
                state = *new_state;
                LiveEvent::Restarted(Box::new(state.to_loaded(&path, &title)))
            }
            // The file can vanish for a moment while Unity rotates it.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => LiveEvent::Failed(err.to_string()),
        };
        if !send(event) {
            break; // the UI dropped the receiver
        }
    }
}

enum Change {
    None,
    Appended(usize),
    Restarted(Box<TailState>),
}

/// Parser state for one followed file.
struct TailState {
    tail: TailDocument,
    picked_label: String,
    kind: Option<(comte_core::EngineId, comte_core::LogKind)>,
    /// Bytes consumed so far.
    offset: u64,
    prefix: Vec<u8>,
    parse_time: Duration,
    /// Only the end of a huge log is followed.
    truncated: Option<Truncated>,
}

impl TailState {
    fn open(path: &Path) -> io::Result<Self> {
        let started = Instant::now();
        let len = std::fs::metadata(path)?.len();
        // A huge log is followed from its last part, like it is opened.
        let (head_bytes, bytes, truncated) = if len > HUGE_LOG {
            let parts = read_huge(path, len, HUGE_TAIL)?;
            (parts.head, parts.tail, Some(parts.truncated))
        } else {
            let bytes = std::fs::read(path)?;
            (bytes.clone(), bytes, None)
        };
        let head = head_lines(&head_bytes);
        let head: Vec<&str> = head.iter().map(String::as_str).take(DETECT_LINES).collect();
        let Picked {
            parser,
            kind_label,
            kind,
        } = pick_parser(&head);
        let mut tail = TailDocument::new(parser);
        if truncated.is_some() {
            for fact in head_facts(&head_bytes, pick_parser(&head).parser.as_mut()) {
                tail.apply(comte_core::ParseEvent::Fact(fact));
            }
        }
        tail.push_bytes(&bytes);
        Ok(Self {
            tail,
            picked_label: kind_label,
            kind,
            offset: len.max(bytes.len() as u64),
            prefix: head_bytes[..head_bytes.len().min(PREFIX_BYTES)].to_vec(),
            parse_time: started.elapsed(),
            truncated,
        })
    }

    fn check(&mut self, path: &Path) -> io::Result<Change> {
        let len = std::fs::metadata(path)?.len();
        if len < self.offset || !self.same_prefix(path)? {
            return Ok(Change::Restarted(Box::new(Self::open(path)?)));
        }
        if len == self.offset {
            return Ok(Change::None);
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut appended = Vec::with_capacity((len - self.offset) as usize);
        file.take(len - self.offset).read_to_end(&mut appended)?;
        self.offset += appended.len() as u64;
        if self.prefix.len() < PREFIX_BYTES {
            let missing = PREFIX_BYTES - self.prefix.len();
            self.prefix
                .extend_from_slice(&appended[..appended.len().min(missing)]);
        }
        Ok(match self.tail.push_bytes(&appended) {
            0 => Change::None, // only part of a line so far
            added => Change::Appended(added),
        })
    }

    fn same_prefix(&self, path: &Path) -> io::Result<bool> {
        let mut start = vec![0; self.prefix.len()];
        File::open(path)?.read_exact(&mut start)?;
        Ok(start == self.prefix)
    }

    fn to_loaded(&self, path: &Path, title: &str) -> Loaded {
        let doc = self.tail.snapshot();
        let index = DocIndex::new(&doc);
        Loaded {
            path: path.to_owned(),
            title: title.to_owned(),
            kind_label: self.picked_label.clone(),
            kind: self.kind,
            session: None,
            insights: Insights::of(&doc),
            doc,
            index,
            raw: self.tail.lines().to_vec(),
            elapsed: self.parse_time,
            truncated: self.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("comte-live-{}-{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn append(path: &Path, text: &str) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    /// Waits for the next event, failing after `timeout`.
    fn next_event(tail: &LiveTail, timeout: Duration) -> LiveEvent {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = tail.poll().into_iter().next() {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "no live event within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    const EDITOR_START: &str = "[Licensing::Module] Trying to connect to existing licensing client channel...\nBuilt from '6000.3/staging' branch; Version is '6000.3.11f1 (3000ef702840) revision 1'; Build Type 'Release'\n";

    #[test]
    fn appended_lines_arrive_within_a_second() {
        let path = temp_file("append.log", EDITOR_START);
        let ctx = egui::Context::default();
        let tail = LiveTail::start(path.clone(), "Editor.log".to_owned(), &ctx);

        let LiveEvent::Update { loaded, added: 0 } = next_event(&tail, Duration::from_secs(2))
        else {
            panic!("expected the initial snapshot");
        };
        assert_eq!(loaded.kind_label, "Unity Editor");
        assert_eq!(loaded.doc.counts().errors(), 0);

        let written = Instant::now();
        append(
            &path,
            "Assets/Game/Player.cs(12,5): error CS0103: The name 'speed' does not exist\n",
        );
        let LiveEvent::Update { loaded, added } = next_event(&tail, Duration::from_secs(2)) else {
            panic!("expected an update");
        };
        assert!(
            written.elapsed() < Duration::from_secs(1),
            "took {:?}",
            written.elapsed()
        );
        assert_eq!(added, 1);
        assert_eq!(loaded.doc.counts().error, 1);
        assert_eq!(loaded.raw.len(), 3);
        drop(tail);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn half_written_line_waits_for_its_end() {
        let path = temp_file("partial.log", EDITOR_START);
        let ctx = egui::Context::default();
        let tail = LiveTail::start(path.clone(), "Editor.log".to_owned(), &ctx);
        next_event(&tail, Duration::from_secs(2));

        append(&path, "Assets/A.cs(1,1): error CS1002: ; exp");
        std::thread::sleep(POLL_INTERVAL * 3);
        assert!(tail.poll().is_empty(), "no update for half a line");
        append(&path, "ected\n");
        let LiveEvent::Update { loaded, .. } = next_event(&tail, Duration::from_secs(2)) else {
            panic!("expected an update");
        };
        let last = loaded.doc.entries().last().unwrap();
        assert_eq!(last.message, "CS1002: ; expected");
        drop(tail);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncated_or_replaced_file_restarts() {
        let path = temp_file(
            "restart.log",
            &format!("{EDITOR_START}line a\nline b\nline c\n"),
        );
        let ctx = egui::Context::default();
        let tail = LiveTail::start(path.clone(), "Editor.log".to_owned(), &ctx);
        next_event(&tail, Duration::from_secs(2));

        // Unity restarted: a new, shorter Editor.log.
        std::fs::write(&path, "[Licensing::Module] new session\n").unwrap();
        let LiveEvent::Restarted(loaded) = next_event(&tail, Duration::from_secs(2)) else {
            panic!("expected a restart");
        };
        assert_eq!(loaded.raw, ["[Licensing::Module] new session"]);

        // Replaced by a *longer* file with different content: still a restart.
        std::fs::write(
            &path,
            format!("[Licensing::Module] third session\n{}", "x\n".repeat(50)),
        )
        .unwrap();
        let LiveEvent::Restarted(loaded) = next_event(&tail, Duration::from_secs(2)) else {
            panic!("expected a restart for a replaced file");
        };
        assert_eq!(loaded.raw.len(), 51);
        drop(tail);
        let _ = std::fs::remove_file(&path);
    }
}
