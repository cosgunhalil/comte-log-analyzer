//! Bookmarks: entries you marked (B or 🔖), kept per log file while the app runs.
//!
//! A bookmark is the entry's **first line number**, not its id, so it survives
//! a reload of the same file and live-tail updates (lines only get appended).
//! When the file is replaced (Unity restarted), its bookmarks are cleared.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use comte_core::{LogDocument, LogEntry};

#[derive(Debug, Default)]
pub struct Bookmarks {
    by_file: HashMap<PathBuf, BTreeSet<usize>>,
}

impl Bookmarks {
    /// Adds or removes the bookmark on `line`; returns whether it is now set.
    pub fn toggle(&mut self, file: &Path, line: usize) -> bool {
        let lines = self.by_file.entry(file.to_owned()).or_default();
        if lines.remove(&line) {
            false
        } else {
            lines.insert(line);
            true
        }
    }

    pub fn contains(&self, file: &Path, line: usize) -> bool {
        self.by_file
            .get(file)
            .is_some_and(|lines| lines.contains(&line))
    }

    /// Bookmarked lines of `file`, in order.
    pub fn lines(&self, file: &Path) -> Vec<usize> {
        self.by_file
            .get(file)
            .map(|lines| lines.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn count(&self, file: &Path) -> usize {
        self.by_file.get(file).map_or(0, BTreeSet::len)
    }

    pub fn clear(&mut self, file: &Path) {
        self.by_file.remove(file);
    }

    /// The next (or previous) bookmarked line after `from`, wrapping around.
    pub fn next(&self, file: &Path, from: Option<usize>, forward: bool) -> Option<usize> {
        let lines = self.by_file.get(file)?;
        if forward {
            let after = from.map_or(0, |l| l + 1);
            lines
                .range(after..)
                .next()
                .or_else(|| lines.iter().next())
                .copied()
        } else {
            let before = from.unwrap_or(usize::MAX);
            lines
                .range(..before)
                .next_back()
                .or_else(|| lines.iter().next_back())
                .copied()
        }
    }
}

/// The entry that starts on `line`.
pub fn entry_starting_at(doc: &LogDocument, line: usize) -> Option<u64> {
    let entries = doc.entries();
    // Entries are in line order; fall back to a scan just in case.
    let i = entries.partition_point(|e| e.line_range.start < line);
    entries
        .get(i)
        .filter(|e| e.line_range.start == line)
        .or_else(|| entries.iter().find(|e| e.line_range.start == line))
        .map(|e| e.id)
}

/// `Editor.log:120 [Error] NullReferenceException: …`: where an entry is and
/// what it says, in one line ("Copy reference").
pub fn entry_reference(title: &str, entry: &LogEntry) -> String {
    let message = comte_core::markup::plain(&entry.message);
    let first = message.lines().next().unwrap_or_default().trim();
    format!(
        "{title}:{} [{}] {first}",
        entry.line_range.start,
        entry.severity.label()
    )
}

#[cfg(test)]
mod tests {
    use comte_core::{Category, ParseEvent, Severity};

    use super::*;

    #[test]
    fn toggle_count_and_wrap_around_navigation() {
        let file = Path::new("Editor.log");
        let mut marks = Bookmarks::default();
        assert!(marks.toggle(file, 40));
        assert!(marks.toggle(file, 10));
        assert!(marks.toggle(file, 25));
        assert_eq!(marks.lines(file), [10, 25, 40]);
        assert!(!marks.toggle(file, 25), "toggling again removes it");
        assert_eq!(marks.count(file), 2);
        assert!(marks.contains(file, 40) && !marks.contains(Path::new("other.log"), 40));

        assert_eq!(marks.next(file, None, true), Some(10));
        assert_eq!(marks.next(file, Some(10), true), Some(40));
        assert_eq!(marks.next(file, Some(40), true), Some(10), "wraps");
        assert_eq!(marks.next(file, Some(10), false), Some(40), "wraps back");
        assert_eq!(marks.next(file, Some(30), false), Some(10));
        assert_eq!(marks.next(Path::new("none.log"), None, true), None);

        marks.clear(file);
        assert_eq!(marks.count(file), 0);
    }

    #[test]
    fn finds_entries_by_first_line() {
        let mut doc = LogDocument::new();
        for (line, len) in [(1, 1), (2, 5), (7, 1)] {
            doc.apply(ParseEvent::Entry(LogEntry::new(
                Severity::Info,
                Category::UserLog,
                "x",
                line..line + len,
            )));
        }
        assert_eq!(entry_starting_at(&doc, 2), Some(1));
        assert_eq!(entry_starting_at(&doc, 7), Some(2));
        assert_eq!(
            entry_starting_at(&doc, 3),
            None,
            "inside an entry, not its start"
        );
    }

    #[test]
    fn reference_is_one_plain_line() {
        let entry = LogEntry::new(
            Severity::Error,
            Category::UserLog,
            "<color=red>Save failed</color>: disk full\nat SaveSystem.Write ()",
            120..122,
        );
        assert_eq!(
            entry_reference("Editor.log", &entry),
            format!(
                "Editor.log:120 [{}] Save failed: disk full",
                Severity::Error.label()
            )
        );
    }
}
