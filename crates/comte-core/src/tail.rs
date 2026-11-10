//! Incremental parsing for live tail (DESIGN §2, "Threading").
//!
//! [`TailDocument`] takes raw bytes as they are appended to a log, in chunks of
//! any size, and keeps the parser's state between chunks. A chunk may end in
//! the middle of a line; that partial line waits for the rest.
//!
//! [`TailDocument::snapshot`] returns a finished [`LogDocument`] for display
//! without disturbing the ongoing parse: it finishes a *copy* of the parser,
//! so an entry still being built (e.g. a `Debug.Log` waiting for its stack
//! trace) shows up now and is completed correctly when more lines arrive.

use crate::document::LogDocument;
use crate::parser::{LogParser, ParseEvent};

pub struct TailDocument {
    parser: Box<dyn LogParser>,
    doc: LogDocument,
    events: Vec<ParseEvent>,
    /// Bytes of a line whose newline has not arrived yet.
    partial: Vec<u8>,
    /// The complete lines read so far (lossy UTF-8, no terminators).
    lines: Vec<String>,
}

impl std::fmt::Debug for TailDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TailDocument")
            .field("lines", &self.lines.len())
            .field("entries", &self.doc.entries().len())
            .field("partial_bytes", &self.partial.len())
            .finish()
    }
}

impl TailDocument {
    pub fn new(parser: Box<dyn LogParser>) -> Self {
        Self {
            parser,
            doc: LogDocument::new(),
            events: Vec::new(),
            partial: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// Adds an event that doesn't come from the lines, e.g. facts read from
    /// the start of a huge log that is followed from its end.
    pub fn apply(&mut self, event: ParseEvent) {
        self.doc.apply(event);
    }

    /// Feeds newly appended bytes. Returns how many complete lines they finished.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> usize {
        let before = self.lines.len();
        let mut rest = bytes;
        while let Some(newline) = rest.iter().position(|&b| b == b'\n') {
            self.partial.extend_from_slice(&rest[..newline]);
            rest = &rest[newline + 1..];
            let mut line = std::mem::take(&mut self.partial);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.push_line(&line);
        }
        self.partial.extend_from_slice(rest);
        self.lines.len() - before
    }

    fn push_line(&mut self, bytes: &[u8]) {
        let bytes = if self.lines.is_empty() {
            bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
        } else {
            bytes
        };
        let line = String::from_utf8_lossy(bytes).into_owned();
        let line_no = self.lines.len() + 1;
        self.parser.feed_line(line_no, &line, &mut self.events);
        self.doc.apply_all(self.events.drain(..));
        self.doc.set_line_count(line_no);
        self.lines.push(line);
    }

    /// A finished document of everything read so far. The partial last line
    /// (no newline yet) is left out until it is complete.
    pub fn snapshot(&self) -> LogDocument {
        let mut parser = self.parser.boxed_clone();
        let mut events = Vec::new();
        // The unfinished last line still tells the parser what is going on
        // (Unity writes `Start importing …` and adds the result to that same
        // line once the import is done). It is shown once it is complete.
        let partial_no = self.lines.len() + 1;
        if !self.partial.is_empty() {
            let line = String::from_utf8_lossy(&self.partial);
            parser.feed_line(partial_no, line.trim_end_matches('\r'), &mut events);
        }
        parser.finish(&mut events);
        // Half a line is not shown yet: nothing may start on it.
        events.retain(|event| match event {
            ParseEvent::Entry(entry) => entry.line_range.start < partial_no,
            ParseEvent::SectionStart(start) => start.line < partial_no,
            _ => true,
        });
        let mut doc = self.doc.clone();
        doc.apply_all(events);
        doc.set_line_count(self.lines.len());
        doc.finish();
        doc
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::model::{Category, LogEntry, Severity};
    use crate::parser::PlainTextParser;
    use crate::reader::parse_reader;

    /// Joins "A" + continuation lines starting with a space into one entry,
    /// so held-back state matters.
    #[derive(Debug, Clone, Default)]
    struct Grouping {
        current: Option<LogEntry>,
    }

    impl LogParser for Grouping {
        fn feed_line(&mut self, n: usize, line: &str, out: &mut Vec<ParseEvent>) {
            if let Some(rest) = line.strip_prefix(' ')
                && let Some(entry) = &mut self.current
            {
                entry.details.push(rest.to_owned());
                entry.line_range.end = n + 1;
                return;
            }
            if let Some(done) = self.current.take() {
                out.push(ParseEvent::Entry(done));
            }
            let severity = if line.contains("error") {
                Severity::Error
            } else {
                Severity::Info
            };
            self.current = Some(LogEntry::new(severity, Category::Other, line, n..n + 1));
        }
        fn finish(&mut self, out: &mut Vec<ParseEvent>) {
            if let Some(done) = self.current.take() {
                out.push(ParseEvent::Entry(done));
            }
        }
        fn boxed_clone(&self) -> Box<dyn LogParser> {
            Box::new(self.clone())
        }
    }

    const LOG: &str = "start\nerror: boom\n detail 1\n detail 2\nnext\n";

    #[test]
    fn chunked_input_matches_parsing_the_whole_file() {
        let whole = parse_reader(Cursor::new(LOG), &mut Grouping::default()).unwrap();
        for chunk_size in [1, 2, 3, 7, LOG.len()] {
            let mut tail = TailDocument::new(Box::new(Grouping::default()));
            for chunk in LOG.as_bytes().chunks(chunk_size) {
                tail.push_bytes(chunk);
            }
            let snapshot = tail.snapshot();
            assert_eq!(
                snapshot.entries(),
                whole.entries(),
                "chunk size {chunk_size}"
            );
            assert_eq!(snapshot.line_count(), 5);
        }
    }

    #[test]
    fn snapshot_shows_pending_entry_without_breaking_the_parse() {
        let mut tail = TailDocument::new(Box::new(Grouping::default()));
        tail.push_bytes(b"start\nerror: boom\n detail 1\n");
        let early = tail.snapshot();
        assert_eq!(early.entries().len(), 2, "pending error is visible now");
        assert_eq!(early.entries()[1].details, ["detail 1"]);

        // More continuation lines still join the same entry.
        tail.push_bytes(b" detail 2\nnext\n");
        let later = tail.snapshot();
        assert_eq!(later.entries().len(), 3);
        assert_eq!(later.entries()[1].details, ["detail 1", "detail 2"]);
        assert_eq!(later.counts().error, 1, "the error is not counted twice");
    }

    #[test]
    fn partial_line_waits_for_its_newline() {
        let mut tail = TailDocument::new(Box::new(PlainTextParser));
        assert_eq!(tail.push_bytes(b"one\ntw"), 1);
        assert_eq!(tail.snapshot().entries().len(), 1);
        assert_eq!(tail.push_bytes(b"o\r\n"), 1);
        assert_eq!(tail.lines(), ["one", "two"]);
    }

    #[test]
    fn bom_and_invalid_utf8_are_handled() {
        let mut tail = TailDocument::new(Box::new(PlainTextParser));
        tail.push_bytes(b"\xEF\xBB\xBFfirst\nbad \xFF byte\n");
        assert_eq!(tail.lines(), ["first", "bad \u{FFFD} byte"]);
    }
}
