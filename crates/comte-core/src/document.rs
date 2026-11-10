//! [`LogDocument`]: the parsed, navigable form of a log.

use crate::fingerprint;
use crate::model::{Fact, FactKey, LogEntry, Metric, Section, SeverityCounts};
use crate::parser::ParseEvent;

/// Collects [`ParseEvent`]s into entries, a section tree, facts and metrics.
///
/// The document assigns entry ids, section ids/parents/depths, each entry's
/// section and fingerprint, and keeps severity counts per section and overall.
/// It can be built incrementally, which is what live tail relies on.
#[derive(Debug, Default, Clone)]
pub struct LogDocument {
    entries: Vec<LogEntry>,
    sections: Vec<Section>,
    facts: Vec<Fact>,
    metrics: Vec<Metric>,
    counts: SeverityCounts,
    /// Stack of currently open section ids, innermost last.
    open_sections: Vec<u32>,
    line_count: usize,
}

impl LogDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: ParseEvent) {
        match event {
            ParseEvent::Entry(entry) => self.push_entry(entry),
            ParseEvent::SectionStart(start) => {
                let id = self.sections.len() as u32;
                self.sections.push(Section {
                    id,
                    parent: self.open_sections.last().copied(),
                    depth: self.open_sections.len() as u16,
                    kind: start.kind,
                    title: start.title,
                    line_range: start.line..start.line + 1,
                    duration: None,
                    counts: SeverityCounts::default(),
                    closed: false,
                });
                self.open_sections.push(id);
            }
            ParseEvent::SectionEnd { line, duration } => {
                // A stray end (parser saw a closing marker without an opening one)
                // is ignored rather than corrupting the tree.
                if let Some(id) = self.open_sections.pop() {
                    let section = &mut self.sections[id as usize];
                    section.line_range.end = section.line_range.end.max(line + 1);
                    section.duration = duration;
                    section.closed = true;
                }
            }
            ParseEvent::Fact(fact) => self.facts.push(fact),
            ParseEvent::Metric(metric) => self.metrics.push(metric),
        }
    }

    pub fn apply_all(&mut self, events: impl IntoIterator<Item = ParseEvent>) {
        for event in events {
            self.apply(event);
        }
    }

    /// Records how many raw lines have been read so far.
    pub fn set_line_count(&mut self, line_count: usize) {
        self.line_count = self.line_count.max(line_count);
    }

    /// End of input: closes any section still open at the last line.
    /// Safe to call more than once.
    pub fn finish(&mut self) {
        let end = self.line_count + 1;
        while let Some(id) = self.open_sections.pop() {
            let section = &mut self.sections[id as usize];
            section.line_range.end = section.line_range.end.max(end);
            section.closed = true;
        }
    }

    fn push_entry(&mut self, mut entry: LogEntry) {
        entry.id = self.entries.len() as u64;
        entry.section_id = self.open_sections.last().copied();
        entry.fingerprint = fingerprint::of_entry(&entry);

        self.counts.add(entry.severity);
        for &id in &self.open_sections {
            let section = &mut self.sections[id as usize];
            section.counts.add(entry.severity);
            section.line_range.end = section.line_range.end.max(entry.line_range.end);
        }
        self.line_count = self.line_count.max(entry.line_range.end.saturating_sub(1));
        self.entries.push(entry);
    }

    // -- accessors ----------------------------------------------------------

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    pub fn entry(&self, id: u64) -> Option<&LogEntry> {
        self.entries.get(usize::try_from(id).ok()?)
    }

    /// The entry whose first line is `line` (entries are in line order).
    pub fn entry_starting_at(&self, line: usize) -> Option<&LogEntry> {
        let i = self.entries.partition_point(|e| e.line_range.start < line);
        self.entries.get(i).filter(|e| e.line_range.start == line)
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn section(&self, id: u32) -> Option<&Section> {
        self.sections.get(id as usize)
    }

    /// Top-level sections, in order.
    pub fn root_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|s| s.parent.is_none())
    }

    /// Direct child sections of `id`, in order.
    pub fn child_sections(&self, id: u32) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(move |s| s.parent == Some(id))
    }

    /// Entries whose innermost section is `section_id` (`None` = outside any section).
    pub fn entries_in(&self, section_id: Option<u32>) -> impl Iterator<Item = &LogEntry> {
        self.entries
            .iter()
            .filter(move |e| e.section_id == section_id)
    }

    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    /// First value recorded for a fact.
    pub fn fact(&self, key: &FactKey) -> Option<&str> {
        self.facts
            .iter()
            .find(|f| &f.key == key)
            .map(|f| f.value.as_str())
    }

    pub fn metrics(&self) -> &[Metric] {
        &self.metrics
    }

    pub fn counts(&self) -> &SeverityCounts {
        &self.counts
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.sections.is_empty() && self.facts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::{Category, MetricKind, SectionKind, Severity};
    use crate::parser::SectionStart;

    fn entry(severity: Severity, line: usize, message: &str) -> ParseEvent {
        ParseEvent::Entry(LogEntry::new(
            severity,
            Category::UserLog,
            message,
            line..line + 1,
        ))
    }

    fn start(kind: SectionKind, title: &str, line: usize) -> ParseEvent {
        ParseEvent::SectionStart(SectionStart {
            kind,
            title: title.to_owned(),
            line,
        })
    }

    #[test]
    fn finds_entries_by_their_first_line() {
        let mut doc = LogDocument::new();
        doc.apply_all([
            entry(Severity::Info, 1, "a"),
            ParseEvent::Entry(LogEntry::new(Severity::Error, Category::UserLog, "b", 2..7)),
            entry(Severity::Info, 7, "c"),
        ]);
        assert_eq!(doc.entry_starting_at(2).map(|e| e.id), Some(1));
        assert_eq!(doc.entry_starting_at(7).map(|e| e.id), Some(2));
        assert!(
            doc.entry_starting_at(3).is_none(),
            "inside an entry, not its start"
        );
        assert!(doc.entry_starting_at(99).is_none());
    }

    #[test]
    fn assigns_ids_sections_and_fingerprints() {
        let mut doc = LogDocument::new();
        doc.apply_all([
            entry(Severity::Info, 1, "boot"),
            start(SectionKind::PlayMode, "Play mode #1", 2),
            entry(Severity::Warning, 3, "careful 1"),
            entry(Severity::Warning, 4, "careful 2"),
            ParseEvent::SectionEnd {
                line: 5,
                duration: None,
            },
            entry(Severity::Info, 6, "after"),
        ]);
        doc.finish();

        let ids: Vec<_> = doc.entries().iter().map(|e| e.id).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
        assert_eq!(doc.entry(0).unwrap().section_id, None);
        assert_eq!(doc.entry(1).unwrap().section_id, Some(0));
        assert_eq!(doc.entry(3).unwrap().section_id, None);
        assert_eq!(
            doc.entry(1).unwrap().fingerprint,
            doc.entry(2).unwrap().fingerprint,
            "messages differing only by a number collapse together"
        );
        assert_eq!(doc.entries_in(Some(0)).count(), 2);
        assert_eq!(doc.entries_in(None).count(), 2);
    }

    #[test]
    fn nested_sections_propagate_counts_to_ancestors() {
        let mut doc = LogDocument::new();
        doc.apply_all([
            start(SectionKind::PlayMode, "Play mode #1", 1),
            entry(Severity::Warning, 2, "w"),
            start(SectionKind::DomainReload, "Domain reload", 3),
            entry(Severity::Error, 4, "e"),
            ParseEvent::SectionEnd {
                line: 5,
                duration: Some(Duration::from_millis(1825)),
            },
            entry(Severity::Exception, 6, "x"),
        ]);
        doc.finish();

        let play = doc.section(0).unwrap();
        let reload = doc.section(1).unwrap();
        assert_eq!(reload.parent, Some(0));
        assert_eq!(reload.depth, 1);
        assert_eq!(reload.counts.errors(), 1);
        assert_eq!(reload.duration, Some(Duration::from_millis(1825)));
        assert_eq!(reload.line_range, 3..6);
        assert_eq!(play.counts.warning, 1);
        assert_eq!(play.counts.errors(), 2, "includes the nested domain reload");
        assert_eq!(doc.entry(1).unwrap().section_id, Some(1));
        assert_eq!(doc.entry(2).unwrap().section_id, Some(0));
        assert_eq!(doc.root_sections().count(), 1);
        assert_eq!(doc.child_sections(0).count(), 1);
        assert_eq!(doc.counts().total(), 3);
    }

    #[test]
    fn finish_closes_open_sections_at_last_line_and_is_idempotent() {
        let mut doc = LogDocument::new();
        doc.apply(start(SectionKind::Crash, "Crash", 10));
        doc.apply(entry(Severity::Crash, 12, "Crash!!!"));
        doc.set_line_count(20);
        doc.finish();
        doc.finish();
        let crash = doc.section(0).unwrap();
        assert!(crash.closed);
        assert_eq!(crash.line_range, 10..21);
        assert_eq!(crash.counts.crash, 1);
    }

    #[test]
    fn stray_section_end_is_ignored() {
        let mut doc = LogDocument::new();
        doc.apply(ParseEvent::SectionEnd {
            line: 3,
            duration: None,
        });
        doc.apply(entry(Severity::Info, 4, "ok"));
        assert!(doc.sections().is_empty());
        assert_eq!(doc.entries().len(), 1);
    }

    #[test]
    fn facts_and_metrics_are_collected() {
        let mut doc = LogDocument::new();
        doc.apply(ParseEvent::Fact(Fact::new(
            FactKey::EngineVersion,
            "6000.3.11f1",
            2,
        )));
        doc.apply(ParseEvent::Metric(Metric::duration(
            MetricKind::AssetRefresh,
            Duration::from_millis(3530),
            347,
        )));
        assert_eq!(doc.fact(&FactKey::EngineVersion), Some("6000.3.11f1"));
        assert_eq!(doc.fact(&FactKey::Gpu), None);
        assert_eq!(
            doc.metrics()[0].as_duration(),
            Some(Duration::from_millis(3530))
        );
        assert!(!doc.is_empty());
    }
}
