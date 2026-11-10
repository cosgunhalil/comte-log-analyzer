//! The streaming parser contract.
//!
//! Parsers are fed one line at a time and push zero or more [`ParseEvent`]s.
//! Because one entry can span many lines, parsers keep state between calls and
//! only emit an entry once they know it is complete. The same parser serves a
//! whole file, a file too large for memory, and live tail.

use std::time::Duration;

use crate::activity::{AssetImport, BuildReport, Hint, ReloadProfile, Unfinished};
use crate::model::{Category, Fact, LogEntry, Metric, SectionKind, Severity};

/// Opening of a timeline section. [`crate::LogDocument`] assigns its id,
/// parent and depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionStart {
    pub kind: SectionKind,
    pub title: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseEvent {
    /// A complete entry.
    Entry(LogEntry),
    /// Opens a (possibly nested) section; following entries belong to it.
    SectionStart(SectionStart),
    /// Closes the innermost open section.
    SectionEnd {
        line: usize,
        duration: Option<Duration>,
    },
    /// Environment information for the info card.
    Fact(Fact),
    /// A measurement for the insights panel.
    Metric(Metric),
    /// An asset import that finished, with its duration.
    Import(AssetImport),
    /// A build's size report.
    BuildReport(BuildReport),
    /// Sent from `finish`: an operation that started and never ended.
    Unfinished(Unfinished),
    /// A known problem, explained.
    Hint(Hint),
    /// Where a domain reload spent its time.
    ReloadProfile(ReloadProfile),
}

pub trait LogParser: Send {
    /// Feed one line (without its line terminator). `line_no` is 1-based.
    fn feed_line(&mut self, line_no: usize, line: &str, out: &mut Vec<ParseEvent>);

    /// End of input: flush any entry still being built.
    fn finish(&mut self, out: &mut Vec<ParseEvent>);

    /// A copy of this parser in its current state. Live tail uses it to show
    /// a finished snapshot while the original keeps reading new lines.
    fn boxed_clone(&self) -> Box<dyn LogParser>;
}

/// Fallback parser for files no engine recognizes: every non-blank line
/// becomes an `Info` / `Other` entry.
#[derive(Debug, Default, Clone)]
pub struct PlainTextParser;

impl LogParser for PlainTextParser {
    fn feed_line(&mut self, line_no: usize, line: &str, out: &mut Vec<ParseEvent>) {
        let text = line.trim_end();
        if !text.trim().is_empty() {
            out.push(ParseEvent::Entry(LogEntry::new(
                Severity::Info,
                Category::Other,
                text,
                line_no..line_no + 1,
            )));
        }
    }

    fn finish(&mut self, _out: &mut Vec<ParseEvent>) {}

    fn boxed_clone(&self) -> Box<dyn LogParser> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_parser_skips_blank_lines() {
        let mut parser = PlainTextParser;
        let mut out = Vec::new();
        parser.feed_line(1, "hello", &mut out);
        parser.feed_line(2, "   ", &mut out);
        parser.feed_line(3, "world  ", &mut out);
        parser.finish(&mut out);
        assert_eq!(out.len(), 2);
        let ParseEvent::Entry(second) = &out[1] else {
            panic!("expected entry")
        };
        assert_eq!(second.message, "world");
        assert_eq!(second.line_range, 3..4);
    }
}
