//! Pins: entries you marked (P or 📌), each with an optional note
//! ("the save breaks here"), kept per log file.
//!
//! A pin is the entry's **first line number**, not its id, so it survives a
//! reload of the same file and live-tail updates (lines only get appended).
//! When the file is replaced (Unity restarted), its pins are cleared. Saved
//! sessions keep their pins (and notes) in the header, and reports list them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use comte_core::{LogEntry, Pin};

#[derive(Debug, Default)]
pub struct Pins {
    /// File → (first line → note).
    by_file: HashMap<PathBuf, BTreeMap<usize, String>>,
}

impl Pins {
    /// Pins `line`, or removes its pin; returns whether it is now pinned.
    pub fn toggle(&mut self, file: &Path, line: usize) -> bool {
        let pins = self.by_file.entry(file.to_owned()).or_default();
        if pins.remove(&line).is_some() {
            false
        } else {
            pins.insert(line, String::new());
            true
        }
    }

    pub fn contains(&self, file: &Path, line: usize) -> bool {
        self.by_file
            .get(file)
            .is_some_and(|pins| pins.contains_key(&line))
    }

    /// The pin's note, when `line` is pinned.
    pub fn note(&self, file: &Path, line: usize) -> Option<&str> {
        self.by_file.get(file)?.get(&line).map(String::as_str)
    }

    /// Sets the note of a pinned line (does nothing if it isn't pinned).
    pub fn set_note(&mut self, file: &Path, line: usize, note: &str) {
        if let Some(slot) = self.by_file.get_mut(file).and_then(|p| p.get_mut(&line)) {
            note.clone_into(slot);
        }
    }

    /// Pins of `file`, in line order.
    pub fn list(&self, file: &Path) -> Vec<Pin> {
        self.by_file
            .get(file)
            .map(|pins| {
                pins.iter()
                    .map(|(&line, note)| Pin {
                        line,
                        note: note.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn count(&self, file: &Path) -> usize {
        self.by_file.get(file).map_or(0, BTreeMap::len)
    }

    pub fn clear(&mut self, file: &Path) {
        self.by_file.remove(file);
    }

    /// Pins read from a saved session, unless the file already has pins
    /// (made since it was opened, which are newer).
    pub fn seed(&mut self, file: &Path, pins: &[Pin]) {
        if pins.is_empty() || self.count(file) > 0 {
            return;
        }
        let map = pins.iter().map(|p| (p.line, p.note.clone())).collect();
        self.by_file.insert(file.to_owned(), map);
    }

    /// The next (or previous) pinned line after `from`, wrapping around.
    pub fn next(&self, file: &Path, from: Option<usize>, forward: bool) -> Option<usize> {
        let pins = self.by_file.get(file)?;
        if forward {
            let after = from.map_or(0, |l| l + 1);
            pins.range(after..)
                .next()
                .or_else(|| pins.iter().next())
                .map(|(&l, _)| l)
        } else {
            let before = from.unwrap_or(usize::MAX);
            pins.range(..before)
                .next_back()
                .or_else(|| pins.iter().next_back())
                .map(|(&l, _)| l)
        }
    }
}

/// Maps pins from the open log to a saved file's body. `saved_lines` are the
/// original line numbers of the lines that go into the file, in order; pins
/// whose line is left out are dropped.
pub fn pins_for_saved_lines(pins: &[Pin], saved_lines: &[usize]) -> Vec<Pin> {
    pins.iter()
        .filter_map(|pin| {
            let at = saved_lines.binary_search(&pin.line).ok()?;
            Some(Pin {
                line: at + 1,
                note: pin.note.clone(),
            })
        })
        .collect()
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
    use comte_core::{Category, Severity};

    use super::*;

    #[test]
    fn toggle_notes_and_wrap_around_navigation() {
        let file = Path::new("Editor.log");
        let mut pins = Pins::default();
        assert!(pins.toggle(file, 40));
        assert!(pins.toggle(file, 10));
        assert!(pins.toggle(file, 25));
        pins.set_note(file, 10, "save breaks here");
        pins.set_note(file, 99, "not pinned: ignored");
        assert_eq!(pins.note(file, 10), Some("save breaks here"));
        assert_eq!(pins.note(file, 99), None);
        assert!(!pins.toggle(file, 25), "toggling again removes it");
        assert_eq!(
            pins.list(file),
            [
                Pin {
                    line: 10,
                    note: "save breaks here".to_owned()
                },
                Pin {
                    line: 40,
                    note: String::new()
                },
            ]
        );
        assert!(pins.contains(file, 40) && !pins.contains(Path::new("other.log"), 40));

        assert_eq!(pins.next(file, None, true), Some(10));
        assert_eq!(pins.next(file, Some(10), true), Some(40));
        assert_eq!(pins.next(file, Some(40), true), Some(10), "wraps");
        assert_eq!(pins.next(file, Some(10), false), Some(40), "wraps back");
        assert_eq!(pins.next(file, Some(30), false), Some(10));
        assert_eq!(pins.next(Path::new("none.log"), None, true), None);

        pins.clear(file);
        assert_eq!(pins.count(file), 0);
    }

    #[test]
    fn seeding_from_a_session_keeps_newer_pins() {
        let file = Path::new("crash.log");
        let saved = [Pin {
            line: 3,
            note: "from the file".to_owned(),
        }];
        let mut pins = Pins::default();
        pins.seed(file, &saved);
        assert_eq!(pins.note(file, 3), Some("from the file"));
        pins.toggle(file, 3);
        pins.toggle(file, 8);
        pins.seed(file, &saved);
        assert_eq!(pins.list(file).len(), 1, "pins made since opening win");
        assert!(pins.contains(file, 8));
    }

    #[test]
    fn pins_follow_lines_into_a_filtered_save() {
        let pins = [
            Pin {
                line: 12,
                note: "a".to_owned(),
            },
            Pin {
                line: 30,
                note: "left out".to_owned(),
            },
            Pin {
                line: 50,
                note: "b".to_owned(),
            },
        ];
        let saved = [3, 12, 13, 50, 51];
        assert_eq!(
            pins_for_saved_lines(&pins, &saved),
            [
                Pin {
                    line: 2,
                    note: "a".to_owned()
                },
                Pin {
                    line: 4,
                    note: "b".to_owned()
                },
            ]
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
