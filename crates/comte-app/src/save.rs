//! Saving the current log as a session file with a note and tags
//! (DESIGN §6.1–6.2). Pure functions; the dialog lives in `app.rs`.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use comte_core::{
    FactKey, Pin, RedactionReport, Redactor, SessionContent, SessionHeader, kind_token,
    parse_header, parse_tags, read_head, write_session,
};

use crate::loader::Loaded;
use crate::pins::pins_for_saved_lines;
use crate::view::Filter;

/// Which lines go into the saved file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentChoice {
    /// The whole log.
    #[default]
    Full,
    /// Only the lines of entries matching the current filter (duplicates
    /// included, even when collapsed in the list).
    Visible,
}

/// What the user typed and chose in the save dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRequest {
    pub note: String,
    /// Comma-separated.
    pub tags: String,
    pub content: ContentChoice,
    pub redact: bool,
    /// Pins of the open log (by its line numbers), with notes.
    pub pins: Vec<Pin>,
}

impl Default for SaveRequest {
    fn default() -> Self {
        Self {
            note: String::new(),
            tags: String::new(),
            content: ContentChoice::Full,
            redact: true, // DESIGN §0: redact by default
            pins: Vec::new(),
        }
    }
}

/// A session ready to be written.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub header: SessionHeader,
    pub lines: Vec<String>,
    pub report: Option<RedactionReport>,
}

/// The original lines of every entry the filter shows, in file order.
pub fn visible_lines<'a>(loaded: &'a Loaded, filter: &Filter) -> Vec<&'a str> {
    visible_line_numbers(loaded, filter)
        .into_iter()
        .filter_map(|n| loaded.raw.get(n - 1).map(String::as_str))
        .collect()
}

/// The (1-based) numbers of the lines [`visible_lines`] returns.
pub fn visible_line_numbers(loaded: &Loaded, filter: &Filter) -> Vec<usize> {
    let query = filter.query();
    let mut ranges: Vec<std::ops::Range<usize>> = loaded
        .doc
        .entries()
        .iter()
        .filter(|e| filter.matches(e, &query))
        .map(|e| e.line_range.clone())
        .collect();
    ranges.sort_by_key(|r| r.start);
    let mut numbers = Vec::new();
    let mut next = 1; // first line not yet taken
    for range in ranges {
        let start = range.start.max(next);
        let end = range.end.min(loaded.raw.len() + 1);
        if start < end {
            numbers.extend(start..end);
            next = end;
        }
    }
    numbers
}

/// "errors and warnings, noise hidden, search "save""-style description.
pub fn describe_filter(filter: &Filter) -> String {
    let severities: Vec<&str> = [
        (filter.show_errors, "errors"),
        (filter.show_warnings, "warnings"),
        (filter.show_info, "info"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let mut parts = vec![match severities.as_slice() {
        [] => "nothing".to_owned(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }];
    parts.push(
        if filter.show_noise {
            "with engine noise"
        } else {
            "noise hidden"
        }
        .to_owned(),
    );
    if !filter.hidden_categories.is_empty() {
        let mut hidden: Vec<String> = filter
            .hidden_categories
            .iter()
            .map(|c| c.label().to_lowercase())
            .collect();
        hidden.sort();
        parts.push(format!("hiding {}", hidden.join(", ")));
    }
    if filter.search_filters && !filter.search.trim().is_empty() {
        let mode = match (filter.search_regex, filter.search_case) {
            (true, true) => " (regex, case-sensitive)",
            (true, false) => " (regex)",
            (false, true) => " (case-sensitive)",
            (false, false) => "",
        };
        parts.push(format!("search \"{}\"{mode}", filter.search.trim()));
    }
    parts.join(", ")
}

/// Builds the header and the (optionally redacted) lines to save.
pub fn prepare(
    loaded: &Loaded,
    filter: &Filter,
    request: &SaveRequest,
    saved_at: String,
    user_names: &[&str],
) -> Prepared {
    let (selected, pins): (Vec<&str>, Vec<Pin>) = match request.content {
        ContentChoice::Full => (
            loaded.raw.iter().map(String::as_str).collect(),
            request.pins.clone(),
        ),
        ContentChoice::Visible => {
            let numbers = visible_line_numbers(loaded, filter);
            (
                numbers
                    .iter()
                    .map(|&n| loaded.raw[n - 1].as_str())
                    .collect(),
                pins_for_saved_lines(&request.pins, &numbers),
            )
        }
    };
    // A reopened session keeps pointing at the log it was made from.
    let (source, source_path) = match &loaded.session {
        Some(s) => (s.source.clone(), s.source_path.clone()),
        None => (loaded.title.clone(), loaded.path.display().to_string()),
    };

    let (lines, source_path, report, pins) = if request.redact {
        // Learn user names from the whole log, not just the saved part.
        let mut redactor = Redactor::for_lines(&loaded.raw, user_names);
        let lines: Vec<String> = selected
            .iter()
            .map(|l| redactor.redact_line(l).into_owned())
            .collect();
        let pins = pins
            .into_iter()
            .map(|pin| Pin {
                note: redactor.redact_line(&pin.note).into_owned(),
                ..pin
            })
            .collect();
        let report = redactor.report().clone();
        let path = Redactor::for_lines(&[source_path.as_str()], user_names)
            .redact_line(&source_path)
            .into_owned();
        (lines, path, Some(report), pins)
    } else {
        (
            selected.iter().map(|l| (*l).to_owned()).collect(),
            source_path,
            None,
            pins,
        )
    };

    let header = SessionHeader {
        comte_version: env!("CARGO_PKG_VERSION").to_owned(),
        source,
        source_path,
        kind: loaded
            .kind
            .map(|(engine, kind)| kind_token(engine, kind))
            .unwrap_or_else(|| "plain".to_owned()),
        saved_at,
        engine_version: loaded.doc.fact(&FactKey::EngineVersion).map(str::to_owned),
        content: match request.content {
            ContentChoice::Full => SessionContent::Full,
            ContentChoice::Visible => SessionContent::Filtered(describe_filter(filter)),
        },
        redacted: report.as_ref().map(|r| r.total),
        tags: parse_tags(&request.tags),
        note: request.note.trim_end().to_owned(),
        pins,
    };
    Prepared {
        header,
        lines,
        report,
    }
}

pub fn write(path: &Path, prepared: &Prepared) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = BufWriter::new(fs::File::create(path)?);
    write_session(
        &mut out,
        &prepared.header,
        prepared.lines.iter().map(String::as_str),
    )?;
    out.flush()
}

/// `Documents\ComteLogger\sessions` (follows OneDrive folder redirection).
pub fn default_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.document_dir().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ComteLogger")
        .join("sessions")
}

/// `2026-11-08_14-05_Flanne-MinutesTillDawn.log`
pub fn default_file_name(title: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    let base = title
        .trim_end_matches(".log")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = base
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "log".to_owned()
    } else {
        slug
    };
    format!("{}_{slug}.log", now.format("%Y-%m-%d_%H-%M"))
}

/// The current Windows/Unix user, to redact even where no path reveals it.
pub fn current_user_name() -> Option<String> {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|n| !n.trim().is_empty())
}

/// A saved session found in the sessions folder.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub title: String,
    pub modified: Option<SystemTime>,
    pub tags: Vec<String>,
    pub note: String,
}

/// Saved sessions in `dir`, newest first. Files that are not sessions are skipped.
pub fn discover_sessions(dir: &Path) -> Vec<SessionFile> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionFile> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        })
        .filter_map(|path| {
            let head = read_head(&path, 200).ok()?;
            let (header, _) = parse_header(&head)?;
            Some(SessionFile {
                title: path.file_name()?.to_string_lossy().into_owned(),
                modified: fs::metadata(&path).and_then(|m| m.modified()).ok(),
                tags: header.tags,
                note: header.note,
                path,
            })
        })
        .collect();
    sessions.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.title.cmp(&b.title))
    });
    sessions
}

#[cfg(test)]
mod tests {
    use comte_core::{Category, Severity};

    use super::*;
    use crate::loader::load;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../comte-unity/tests/fixtures")
            .join(name)
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("comte-save-{}-{name}", std::process::id()))
    }

    #[test]
    fn round_trip_keeps_note_tags_and_parses_the_same() {
        let original = load(&fixture("editor_session.log"), "Editor.log").unwrap();
        let request = SaveRequest {
            note: "Play mode broke after the save system change.\nRepro: end turn twice."
                .to_owned(),
            tags: "save, play mode, , Save".to_owned(),
            content: ContentChoice::Full,
            redact: true,
            pins: vec![Pin {
                line: 75,
                note: "obsolete API; was this before the crash?".to_owned(),
            }],
        };
        let prepared = prepare(
            &original,
            &Filter::default(),
            &request,
            "2026-11-08T14:05:00+03:00".to_owned(),
            &[],
        );
        let path = temp_path("roundtrip.log");
        write(&path, &prepared).unwrap();

        let reopened = load(&path, "roundtrip.log").unwrap();
        let _ = fs::remove_file(&path);
        let session = reopened.session.as_ref().expect("read back as a session");
        assert_eq!(session.note, request.note);
        assert_eq!(session.tags, ["save", "play mode"]);
        assert_eq!(session.kind, "unity-editor");
        assert_eq!(session.source, "Editor.log");
        assert_eq!(session.engine_version.as_deref(), Some("6000.3.11f1"));
        assert!(reopened.kind_label.ends_with("saved session"));
        // The pin still points at the same entry in the saved file.
        assert_eq!(session.pins, request.pins);
        let pinned = reopened.doc.entry_starting_at(75).expect("pinned entry");
        assert!(pinned.message.contains("CS0618"));

        // Same structure as the original: redaction keeps the log parseable.
        assert_eq!(reopened.doc.entries().len(), original.doc.entries().len());
        assert_eq!(reopened.doc.sections().len(), original.doc.sections().len());
        assert_eq!(reopened.doc.counts(), original.doc.counts());
        assert_eq!(reopened.raw.len(), original.raw.len());
    }

    #[test]
    fn redaction_masks_the_fixture_user_and_token() {
        let loaded = load(&fixture("editor_session.log"), "Editor.log").unwrap();
        let prepared = prepare(
            &loaded,
            &Filter::default(),
            &SaveRequest::default(),
            String::new(),
            &[],
        );
        let text = prepared.lines.join("\n");
        assert!(!text.contains(r"Users\dev"), "user name in paths");
        assert!(text.contains(r"C:\Users\<user>\Projects\Sample Game"));
        let report = prepared.report.unwrap();
        assert!(report.total > 0);
        assert_eq!(prepared.header.redacted, Some(report.total));
        assert!(!prepared.header.source_path.contains("cosgu"));
    }

    #[test]
    fn unredacted_full_save_is_byte_identical_body() {
        let loaded = load(&fixture("player_session.log"), "Player.log").unwrap();
        let request = SaveRequest {
            redact: false,
            ..SaveRequest::default()
        };
        let prepared = prepare(&loaded, &Filter::default(), &request, String::new(), &[]);
        assert_eq!(prepared.lines, loaded.raw);
        assert_eq!(prepared.header.redacted, None);
        assert_eq!(prepared.header.content, SessionContent::Full);
    }

    #[test]
    fn visible_content_keeps_whole_entries_only() {
        let loaded = load(&fixture("player_session.log"), "Player.log").unwrap();
        let filter = Filter {
            show_info: false,
            ..Filter::default()
        };
        let lines = visible_lines(&loaded, &filter);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("NullReferenceException"))
        );
        assert!(lines.iter().any(|l| l.starts_with("Crash!!!")));
        assert!(
            !lines.iter().any(|l| l.starts_with("UnloadTime")),
            "info entries left out"
        );

        // The saved part re-parses into the same problems, and pins move to
        // the saved file's line numbers (a pin on a left-out line is dropped).
        let crash = loaded
            .doc
            .entries()
            .iter()
            .find(|e| e.severity == Severity::Crash)
            .unwrap();
        let info = loaded
            .doc
            .entries()
            .iter()
            .find(|e| e.severity == Severity::Info)
            .unwrap();
        let request = SaveRequest {
            content: ContentChoice::Visible,
            redact: false,
            pins: vec![
                Pin {
                    line: info.line_range.start,
                    note: "not saved".to_owned(),
                },
                Pin {
                    line: crash.line_range.start,
                    note: "here".to_owned(),
                },
            ],
            ..SaveRequest::default()
        };
        let prepared = prepare(&loaded, &filter, &request, String::new(), &[]);
        assert_eq!(
            prepared.header.content,
            SessionContent::Filtered("errors and warnings, noise hidden".to_owned())
        );
        let path = temp_path("visible.log");
        write(&path, &prepared).unwrap();
        let reopened = load(&path, "visible.log").unwrap();
        let _ = fs::remove_file(&path);
        let original = loaded.doc.counts();
        let saved = reopened.doc.counts();
        assert_eq!(saved.errors(), original.errors());
        assert_eq!(saved.warning, original.warning);
        let pins = &reopened.session.as_ref().unwrap().pins;
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].note, "here");
        let pinned = reopened.doc.entry_starting_at(pins[0].line).unwrap();
        assert_eq!(pinned.severity, Severity::Crash);
    }

    #[test]
    fn filter_descriptions() {
        let mut filter = Filter::default();
        assert_eq!(
            describe_filter(&filter),
            "errors, warnings and info, noise hidden"
        );
        filter.show_info = false;
        filter.show_noise = true;
        filter.search = " save ".to_owned();
        filter.hidden_categories.insert(Category::Compiler);
        assert_eq!(
            describe_filter(&filter),
            "errors and warnings, with engine noise, hiding compiler, search \"save\""
        );
        let _ = Severity::Info;
    }

    #[test]
    fn file_names_and_session_discovery() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 8, 14, 5, 0)
            .unwrap();
        assert_eq!(
            default_file_name("Flanne › MinutesTillDawn", &now),
            "2026-11-08_14-05_Flanne-MinutesTillDawn.log"
        );
        assert_eq!(
            default_file_name("Editor.log", &now),
            "2026-11-08_14-05_Editor.log"
        );
        assert_eq!(default_file_name("›››", &now), "2026-11-08_14-05_log.log");

        let dir = temp_path("sessions-dir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let loaded = load(&fixture("player_session.log"), "Player.log").unwrap();
        let request = SaveRequest {
            note: "crash in scene 3".to_owned(),
            tags: "crash".to_owned(),
            ..SaveRequest::default()
        };
        write(
            &dir.join("a.log"),
            &prepare(&loaded, &Filter::default(), &request, String::new(), &[]),
        )
        .unwrap();
        fs::write(dir.join("plain.log"), "not a session\n").unwrap();
        let found = discover_sessions(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tags, ["crash"]);
        assert_eq!(found[0].note, "crash in scene 3");
    }

    /// Checks redaction on a real log, e.g.
    /// `COMTE_REAL_LOG=%LOCALAPPDATA%\Unity\Editor\Editor.log cargo test -p comte-app -- --ignored`.
    #[test]
    #[ignore = "needs COMTE_REAL_LOG pointing at a real log"]
    fn real_log_redaction_leaves_no_user_name() {
        let path = std::env::var("COMTE_REAL_LOG").expect("set COMTE_REAL_LOG");
        let loaded = load(Path::new(&path), "real").unwrap();
        let user = current_user_name().unwrap_or_default();
        let names: Vec<&str> = vec![user.as_str()];
        let prepared = prepare(
            &loaded,
            &Filter::default(),
            &SaveRequest::default(),
            String::new(),
            &names,
        );
        let leaks: Vec<&String> = prepared
            .lines
            .iter()
            .filter(|l| !user.is_empty() && l.to_lowercase().contains(&user.to_lowercase()))
            .collect();
        println!("{}", prepared.report.as_ref().unwrap().describe());
        assert!(leaks.is_empty(), "user name still present: {leaks:#?}");
    }
}
