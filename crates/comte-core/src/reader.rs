//! Line-oriented reading of log files.
//!
//! Unity logs are *mostly* UTF-8 but can contain invalid bytes (native crash
//! output, odd encodings from plugins), use CRLF or LF, and may start with a
//! BOM. [`LineReader`] handles all of that and tracks the byte offset, so live
//! tail can resume exactly where the last read stopped.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use crate::document::LogDocument;
use crate::parser::LogParser;

const READ_BUFFER: usize = 1 << 20; // 1 MiB

#[derive(Debug)]
pub struct LineReader<R> {
    inner: R,
    raw: Vec<u8>,
    text: String,
    /// Number of the last line returned (1-based); 0 before the first read.
    line_no: usize,
    /// Byte offset just after the last line returned.
    offset: u64,
}

impl<R: BufRead> LineReader<R> {
    pub fn new(inner: R) -> Self {
        Self::resume(inner, 0, 0)
    }

    /// Continue reading a file whose first `line_no` lines / `offset` bytes
    /// were already consumed (live tail).
    pub fn resume(inner: R, line_no: usize, offset: u64) -> Self {
        Self {
            inner,
            raw: Vec::with_capacity(256),
            text: String::with_capacity(256),
            line_no,
            offset,
        }
    }

    /// Next line without its terminator, as `(line_no, text)`.
    /// Invalid UTF-8 is replaced with `U+FFFD`.
    pub fn next_line(&mut self) -> io::Result<Option<(usize, &str)>> {
        self.raw.clear();
        let read = self.inner.read_until(b'\n', &mut self.raw)?;
        if read == 0 {
            return Ok(None);
        }
        self.offset += read as u64;
        self.line_no += 1;

        let mut bytes = self.raw.as_slice();
        if let Some(rest) = bytes.strip_suffix(b"\n") {
            bytes = rest;
        }
        if let Some(rest) = bytes.strip_suffix(b"\r") {
            bytes = rest;
        }
        if self.line_no == 1
            && let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF")
        {
            bytes = rest;
        }

        self.text.clear();
        self.text.push_str(&String::from_utf8_lossy(bytes));
        Ok(Some((self.line_no, &self.text)))
    }

    pub fn line_no(&self) -> usize {
        self.line_no
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }
}

/// Parses everything `reader` yields into a finished [`LogDocument`].
pub fn parse_reader(reader: impl BufRead, parser: &mut dyn LogParser) -> io::Result<LogDocument> {
    let mut lines = LineReader::new(reader);
    let mut doc = LogDocument::new();
    let mut events = Vec::new();

    while let Some((line_no, line)) = lines.next_line()? {
        parser.feed_line(line_no, line, &mut events);
        doc.apply_all(events.drain(..));
    }
    parser.finish(&mut events);
    doc.apply_all(events.drain(..));
    doc.set_line_count(lines.line_no());
    doc.finish();
    Ok(doc)
}

/// The first `max_lines` lines of a file (lossy UTF-8, no terminators),
/// used to detect what kind of log it is.
pub fn read_head(path: impl AsRef<Path>, max_lines: usize) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let mut lines = LineReader::new(BufReader::new(file));
    let mut head = Vec::with_capacity(max_lines);
    while head.len() < max_lines {
        match lines.next_line()? {
            Some((_, line)) => head.push(line.to_owned()),
            None => break,
        }
    }
    Ok(head)
}

/// Opens and parses a log file.
pub fn parse_file(path: impl AsRef<Path>, parser: &mut dyn LogParser) -> io::Result<LogDocument> {
    let file = File::open(path)?;
    parse_reader(BufReader::with_capacity(READ_BUFFER, file), parser)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::parser::PlainTextParser;

    fn collect(bytes: &[u8]) -> Vec<(usize, String)> {
        let mut reader = LineReader::new(Cursor::new(bytes));
        let mut out = Vec::new();
        while let Some((n, line)) = reader.next_line().unwrap() {
            out.push((n, line.to_owned()));
        }
        out
    }

    #[test]
    fn handles_crlf_lf_bom_and_missing_final_newline() {
        let lines = collect(b"\xEF\xBB\xBFfirst\r\nsecond\nthird");
        assert_eq!(
            lines,
            [
                (1, "first".to_owned()),
                (2, "second".to_owned()),
                (3, "third".to_owned())
            ]
        );
    }

    #[test]
    fn replaces_invalid_utf8_instead_of_failing() {
        let lines = collect(b"ok\nbad \xFF byte\n");
        assert_eq!(lines[1].1, "bad \u{FFFD} byte");
    }

    #[test]
    fn tracks_offset_for_resume() {
        let data = b"one\ntwo\nthree\n";
        let mut reader = LineReader::new(Cursor::new(&data[..]));
        reader.next_line().unwrap();
        reader.next_line().unwrap();
        assert_eq!(reader.offset(), 8);
        assert_eq!(reader.line_no(), 2);

        let mut resumed = LineReader::resume(Cursor::new(&data[8..]), 2, 8);
        let (n, line) = resumed.next_line().unwrap().unwrap();
        assert_eq!((n, line), (3, "three"));
        assert_eq!(resumed.offset(), data.len() as u64);
    }

    #[test]
    fn read_head_stops_at_limit_and_at_eof() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let head = read_head(path, 2).unwrap();
        assert_eq!(head, ["[package]", "name = \"comte-core\""]);
        assert!(read_head(path, 10_000).unwrap().len() < 10_000);
        assert!(read_head("no/such/file.log", 5).is_err());
    }

    #[test]
    fn parse_reader_builds_a_finished_document() {
        let doc = parse_reader(Cursor::new("a\n\nb\n"), &mut PlainTextParser).unwrap();
        assert_eq!(doc.entries().len(), 2);
        assert_eq!(doc.line_count(), 3);
        assert_eq!(doc.entries()[1].line_range, 3..4);
    }
}
