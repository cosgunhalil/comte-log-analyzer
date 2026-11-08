//! Saved sessions (DESIGN §6.1): a plain `.log` file that starts with a
//! commented header (source, note, tags, …) followed by original log lines.
//!
//! ```text
//! # ==== Comte Logger session ====================================
//! # comte_version: 0.1.0
//! # source:        Player.log (Flanne › MinutesTillDawn)
//! # source_path:   C:\Users\<user>\AppData\LocalLow\Flanne\MinutesTillDawn\Player.log
//! # kind:          unity-player
//! # saved_at:      2026-11-08T14:05:00+03:00
//! # unity:         2019.4.40f1
//! # content:       filtered (errors and warnings, noise hidden)
//! # redacted:      yes (12 values)
//! # tags:          crash, steam
//! # --- note -----------------------------------------------------
//! # > Crashes when Steam is not running.
//! # ==============================================================
//! <original log lines>
//! ```
//!
//! Any text editor can read it; Comte Logger reads the header back on open.
//! Note lines are prefixed with `# > ` so a note can never end the header early.

use std::io::{self, Write};

use crate::source::{EngineId, LogKind};

const OPENING: &str = "# ==== Comte Logger session ";
const NOTE_MARKER: &str = "# --- note ";
const CLOSING: &str = "# ======";
const NOTE_PREFIX: &str = "# > ";
const RULE_WIDTH: usize = 64;

/// What a saved file contains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionContent {
    /// Every line of the original log.
    #[default]
    Full,
    /// Only the lines of the entries that were visible, with a description of
    /// the filter that was active.
    Filtered(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionHeader {
    pub comte_version: String,
    /// Display title of the original source, e.g. `Editor.log`.
    pub source: String,
    pub source_path: String,
    /// Which parser reads the body, e.g. `unity-editor` (see [`kind_token`]).
    pub kind: String,
    /// When the session was saved (RFC 3339, local time with offset).
    pub saved_at: String,
    pub engine_version: Option<String>,
    pub content: SessionContent,
    /// `Some(n)` when `n` sensitive values were replaced before saving.
    pub redacted: Option<usize>,
    pub tags: Vec<String>,
    pub note: String,
}

/// `unity-editor`, `unity-player`, …: identifies the parser for a body.
pub fn kind_token(engine: EngineId, kind: LogKind) -> String {
    format!(
        "{}-{}",
        engine.label().to_lowercase(),
        kind.label().to_lowercase()
    )
}

/// Inverse of [`kind_token`] for the given engines.
pub fn parse_kind_token(token: &str) -> Option<(EngineId, LogKind)> {
    [EngineId::Unity]
        .into_iter()
        .flat_map(|engine| [LogKind::Editor, LogKind::Player].map(|kind| (engine, kind)))
        .find(|(engine, kind)| kind_token(*engine, *kind) == token)
}

/// Splits a comma-separated tag list, trimming and dropping empty or
/// duplicate tags (case-insensitive).
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

/// Writes the header, then `lines`, each followed by a newline.
pub fn write_session<'a>(
    out: &mut impl Write,
    header: &SessionHeader,
    lines: impl IntoIterator<Item = &'a str>,
) -> io::Result<()> {
    let rule = |out: &mut dyn Write, start: &str| -> io::Result<()> {
        let fill = RULE_WIDTH.saturating_sub(start.chars().count());
        let line = format!("{start}{}", "=".repeat(fill));
        writeln!(out, "{}", line.trim_end())
    };
    rule(out, OPENING)?;
    let field = |out: &mut dyn Write, key: &str, value: &str| -> io::Result<()> {
        writeln!(out, "# {:<15}{}", format!("{key}:"), single_line(value))
    };
    field(out, "comte_version", &header.comte_version)?;
    field(out, "source", &header.source)?;
    field(out, "source_path", &header.source_path)?;
    field(out, "kind", &header.kind)?;
    field(out, "saved_at", &header.saved_at)?;
    if let Some(version) = &header.engine_version {
        field(out, "unity", version)?;
    }
    let content = match &header.content {
        SessionContent::Full => "full".to_owned(),
        SessionContent::Filtered(description) => format!("filtered ({description})"),
    };
    field(out, "content", &content)?;
    let redacted = match header.redacted {
        Some(count) => format!("yes ({count} values)"),
        None => "no".to_owned(),
    };
    field(out, "redacted", &redacted)?;
    field(out, "tags", &header.tags.join(", "))?;
    let note_rule = format!(
        "{NOTE_MARKER}{}",
        "-".repeat(RULE_WIDTH - NOTE_MARKER.len())
    );
    writeln!(out, "{note_rule}")?;
    for line in header.note.lines() {
        writeln!(out, "{NOTE_PREFIX}{}", line.trim_end())?;
    }
    writeln!(out, "# {}", "=".repeat(RULE_WIDTH - 2))?;
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

/// Reads a session header from the first lines of a file. Returns the header
/// and the number of header lines (the body starts after them), or `None` if
/// the file is not a saved session.
pub fn parse_header<S: AsRef<str>>(lines: &[S]) -> Option<(SessionHeader, usize)> {
    let first = lines.first()?.as_ref();
    if !first.starts_with(OPENING) {
        return None;
    }
    let mut header = SessionHeader::default();
    let mut in_note = false;
    let mut note: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(1) {
        let line = line.as_ref();
        if line.starts_with(CLOSING) {
            header.note = note.join("\n");
            return Some((header, index + 1));
        }
        if line.starts_with(NOTE_MARKER) {
            in_note = true;
            continue;
        }
        if in_note {
            if let Some(text) = line.strip_prefix(NOTE_PREFIX) {
                note.push(text);
            } else if line == NOTE_PREFIX.trim_end() {
                note.push("");
            }
            continue;
        }
        let Some((key, value)) = line.strip_prefix("# ").and_then(|l| l.split_once(':')) else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "comte_version" => header.comte_version = value.to_owned(),
            "source" => header.source = value.to_owned(),
            "source_path" => header.source_path = value.to_owned(),
            "kind" => header.kind = value.to_owned(),
            "saved_at" => header.saved_at = value.to_owned(),
            "unity" => header.engine_version = Some(value.to_owned()),
            "content" => {
                header.content = match value
                    .strip_prefix("filtered (")
                    .and_then(|v| v.strip_suffix(')'))
                {
                    Some(description) => SessionContent::Filtered(description.to_owned()),
                    None if value.starts_with("filtered") => {
                        SessionContent::Filtered(String::new())
                    }
                    None => SessionContent::Full,
                };
            }
            "redacted" => {
                header.redacted = value.strip_prefix("yes").map(|rest| {
                    rest.trim()
                        .trim_start_matches('(')
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0)
                });
            }
            "tags" => header.tags = parse_tags(value),
            _ => {}
        }
    }
    None // no closing rule: not a complete header
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SessionHeader {
        SessionHeader {
            comte_version: "0.1.0".to_owned(),
            source: "Player.log (Flanne › MinutesTillDawn)".to_owned(),
            source_path: r"C:\Users\<user>\AppData\LocalLow\Flanne\MinutesTillDawn\Player.log"
                .to_owned(),
            kind: kind_token(EngineId::Unity, LogKind::Player),
            saved_at: "2026-11-08T14:05:00+03:00".to_owned(),
            engine_version: Some("2019.4.40f1".to_owned()),
            content: SessionContent::Filtered("errors and warnings, noise hidden".to_owned()),
            redacted: Some(12),
            tags: vec!["crash".to_owned(), "steam".to_owned()],
            note:
                "Crashes when Steam is not running.\n\n# ===== not the end\nRepro: start from Epic."
                    .to_owned(),
        }
    }

    fn render(header: &SessionHeader, body: &[&str]) -> String {
        let mut out = Vec::new();
        write_session(&mut out, header, body.iter().copied()).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn round_trip_keeps_every_field_and_the_body() {
        let header = sample();
        let text = render(
            &header,
            &[
                "Initialize engine version: 2019.4.40f1",
                "UnloadTime: 9.5 ms",
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        let (parsed, body_start) = parse_header(&lines).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(
            &lines[body_start..],
            [
                "Initialize engine version: 2019.4.40f1",
                "UnloadTime: 9.5 ms"
            ]
        );
    }

    #[test]
    fn header_is_readable_as_text() {
        let text = render(&sample(), &[]);
        assert!(text.starts_with("# ==== Comte Logger session ====="));
        assert!(text.contains("# kind:          unity-player\n"));
        assert!(text.contains("# redacted:      yes (12 values)\n"));
        assert!(text.contains("# > Crashes when Steam is not running.\n"));
        assert!(
            text.lines()
                .all(|l| l.chars().count() <= 80 || !l.starts_with("# =="))
        );
    }

    #[test]
    fn full_unredacted_session_without_note() {
        let header = SessionHeader {
            content: SessionContent::Full,
            redacted: None,
            tags: Vec::new(),
            note: String::new(),
            engine_version: None,
            ..sample()
        };
        let text = render(&header, &["a"]);
        let lines: Vec<&str> = text.lines().collect();
        let (parsed, start) = parse_header(&lines).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(&lines[start..], ["a"]);
    }

    #[test]
    fn plain_logs_and_truncated_headers_are_not_sessions() {
        assert!(parse_header(&["[Licensing::Module] hi"]).is_none());
        assert!(parse_header::<&str>(&[]).is_none());
        assert!(
            parse_header(&["# ==== Comte Logger session =====", "# kind: unity-editor"]).is_none()
        );
    }

    #[test]
    fn kind_tokens_and_tags() {
        assert_eq!(kind_token(EngineId::Unity, LogKind::Editor), "unity-editor");
        assert_eq!(
            parse_kind_token("unity-player"),
            Some((EngineId::Unity, LogKind::Player))
        );
        assert_eq!(parse_kind_token("unreal-editor"), None);
        assert_eq!(
            parse_tags(" crash, perf,,Crash , build 1.2 "),
            ["crash", "perf", "build 1.2"]
        );
    }
}
