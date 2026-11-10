//! The search box's query language.
//!
//! - `save failed`: entries containing *both* words (in any order, in any
//!   searchable field: message, details, stack frames, file paths).
//! - `"save failed"`: the exact phrase.
//! - `-licensing`: entries *not* containing the word (`-"two words"` too).
//! - Regex mode: the whole text is one regular expression.
//! - Case-insensitive unless asked otherwise.
//!
//! Words are compiled to escaped regexes, so highlighting finds the same
//! byte ranges matching found, even for non-ASCII text.

use std::ops::Range;

use comte_core::{LogEntry, markup};
use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone)]
pub enum Query {
    /// Nothing typed: everything matches, nothing is highlighted.
    Empty,
    /// Words and phrases: all of `include` and none of `exclude`.
    Terms {
        include: Vec<Regex>,
        exclude: Vec<Regex>,
    },
    /// Regex mode.
    Regex(Regex),
    /// An unfinished or invalid regex. Matches everything, so the list does
    /// not go blank while typing; the UI shows the error.
    Invalid(String),
}

impl Query {
    pub fn compile(text: &str, case_sensitive: bool, regex_mode: bool) -> Self {
        let text = text.trim();
        if text.is_empty() {
            return Query::Empty;
        }
        let build = |pattern: &str| {
            RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .build()
        };
        if regex_mode {
            return match build(text) {
                Ok(re) => Query::Regex(re),
                Err(err) => Query::Invalid(first_line(&err.to_string())),
            };
        }
        let mut include = Vec::new();
        let mut exclude = Vec::new();
        for (word, negated) in words(text) {
            let Ok(re) = build(&regex::escape(&word)) else {
                continue; // an escaped literal always compiles
            };
            if negated {
                exclude.push(re);
            } else {
                include.push(re);
            }
        }
        if include.is_empty() && exclude.is_empty() {
            Query::Empty
        } else {
            Query::Terms { include, exclude }
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Query::Empty)
    }

    /// Why the query can't be used (invalid regex), for the UI.
    pub fn error(&self) -> Option<&str> {
        match self {
            Query::Invalid(err) => Some(err),
            _ => None,
        }
    }

    /// Whether `entry` satisfies the query.
    pub fn matches_entry(&self, entry: &LogEntry) -> bool {
        match self {
            Query::Empty | Query::Invalid(_) => true,
            Query::Regex(re) => any_field(entry, |text| re.is_match(text)),
            Query::Terms { include, exclude } => {
                include
                    .iter()
                    .all(|re| any_field(entry, |text| re.is_match(text)))
                    && !exclude
                        .iter()
                        .any(|re| any_field(entry, |text| re.is_match(text)))
            }
        }
    }

    /// Byte ranges in `text` to highlight (what the user is looking for;
    /// excluded words are never highlighted). Sorted and merged.
    pub fn ranges(&self, text: &str) -> Vec<Range<usize>> {
        let regexes: Vec<&Regex> = match self {
            Query::Empty | Query::Invalid(_) => return Vec::new(),
            Query::Regex(re) => vec![re],
            Query::Terms { include, .. } => include.iter().collect(),
        };
        let mut ranges: Vec<Range<usize>> = regexes
            .iter()
            .flat_map(|re| re.find_iter(text).map(|m| m.range()))
            .filter(|r| !r.is_empty())
            .collect();
        ranges.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }
}

/// Calls `test` on every searchable field until one returns `true`.
/// Messages are searched without their rich-text tags.
fn any_field(entry: &LogEntry, mut test: impl FnMut(&str) -> bool) -> bool {
    test(&markup::plain(&entry.message))
        || entry.details.iter().any(|d| test(&markup::plain(d)))
        || entry
            .stack
            .iter()
            .any(|f| test(&f.text) || f.location.as_ref().is_some_and(|l| test(&l.to_string())))
        || entry.source.as_ref().is_some_and(|s| test(&s.to_string()))
}

/// Splits on whitespace, keeping `"quoted phrases"` together; a leading `-`
/// (not alone) negates the word.
fn words(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut negated = false;
        if c == '-' {
            chars.next();
            match chars.peek() {
                Some(next) if !next.is_whitespace() => negated = true,
                _ => {
                    out.push(("-".to_owned(), false)); // a lone "-" is a word
                    continue;
                }
            }
        }
        let mut word = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        if !word.is_empty() {
            out.push((word, negated));
        }
    }
    out
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|l| l.starts_with("error:"))
        .or_else(|| text.lines().next())
        .unwrap_or("invalid regular expression")
        .trim_start_matches("error:")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use comte_core::{Category, FrameKind, Severity, SourceRef, StackFrame};

    use super::*;

    fn entry(message: &str) -> LogEntry {
        LogEntry::new(Severity::Error, Category::UserLog, message, 1..2).with_stack(vec![
            StackFrame::new("SaveSystem.Write(int)", FrameKind::User)
                .with_location(SourceRef::new("Assets/Scripts/Save/SaveSystem.cs").with_line(77)),
        ])
    }

    fn q(text: &str) -> Query {
        Query::compile(text, false, false)
    }

    #[test]
    fn words_are_and_matched_across_fields() {
        let e = entry("Save failed: disk quota exceeded");
        assert!(q("save failed").matches_entry(&e));
        assert!(
            q("quota savesystem.write").matches_entry(&e),
            "message + frame"
        );
        assert!(q("SaveSystem.cs:77").matches_entry(&e), "file:line");
        assert!(
            !q("save loaded").matches_entry(&e),
            "every word must appear"
        );
        assert!(q("").matches_entry(&e) && q("   ").is_empty());
    }

    #[test]
    fn phrases_and_exclusions() {
        let e = entry("Save failed: disk quota exceeded");
        assert!(q("\"disk quota\"").matches_entry(&e));
        assert!(
            !q("\"quota disk\"").matches_entry(&e),
            "a phrase keeps its order"
        );
        assert!(!q("save -quota").matches_entry(&e));
        assert!(q("save -licensing").matches_entry(&e));
        assert!(!q("-\"disk quota\"").matches_entry(&e));
        assert!(
            q("-licensing").matches_entry(&e),
            "only exclusions: everything else"
        );
        assert_eq!(
            words("a - b"),
            [
                ("a".into(), false),
                ("-".into(), false),
                ("b".into(), false)
            ]
        );
    }

    #[test]
    fn case_and_regex_modes() {
        let e = entry("Save failed for slot 2");
        assert!(
            !Query::compile("save", true, false).matches_entry(&e),
            "case-sensitive"
        );
        assert!(Query::compile("Save", true, false).matches_entry(&e));
        assert!(Query::compile(r"slot \d+$", false, true).matches_entry(&e));
        assert!(!Query::compile(r"^slot", false, true).matches_entry(&e));
        // Regex characters are literal in word mode.
        assert!(!q("slot.2").matches_entry(&e));
        assert!(q("slot").matches_entry(&e));
    }

    #[test]
    fn invalid_regex_reports_and_matches_everything() {
        let query = Query::compile("slot (", false, true);
        assert!(query.error().is_some());
        assert!(query.matches_entry(&entry("anything")));
        assert!(query.ranges("slot (").is_empty());
    }

    #[test]
    fn rich_text_tags_are_not_searched() {
        let e = entry("<color='cyan'>Starting new race</color>");
        assert!(q("\"Starting new race\"").matches_entry(&e));
        assert!(
            !q("cyan").matches_entry(&e),
            "the tag is styling, not content"
        );
    }

    #[test]
    fn highlight_ranges_are_merged_and_ignore_exclusions() {
        let text = "Save failed, save again";
        assert_eq!(q("save").ranges(text), [0..4, 13..17]);
        assert_eq!(q("save -again").ranges(text), [0..4, 13..17]);
        assert_eq!(q("sav save").ranges(text), [0..4, 13..17], "overlaps merge");
        assert_eq!(
            Query::compile(r"a\w+", false, true).ranges("a ab abc"),
            [2..4, 5..8]
        );
        // Case-insensitive matching keeps byte ranges valid for non-ASCII text.
        let ranges = q("çağ").ranges("Bir ÇAĞ önce");
        assert_eq!(&"Bir ÇAĞ önce"[ranges[0].clone()], "ÇAĞ");
    }
}
