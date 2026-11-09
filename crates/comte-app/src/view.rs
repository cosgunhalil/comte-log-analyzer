//! The view model behind the entry list: which rows are visible for the
//! current filter, collapsed sections and duplicate collapsing.
//!
//! Pure functions over a [`LogDocument`], so they are unit-tested without a UI.

use std::collections::{HashMap, HashSet};

use comte_core::{Category, LogDocument, LogEntry, SectionKind, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Section(u32),
    Entry(u64),
}

/// The children of every section in line order, computed once per document.
#[derive(Debug, Default)]
pub struct DocIndex {
    root: Vec<Item>,
    children: Vec<Vec<Item>>,
}

impl DocIndex {
    pub fn new(doc: &LogDocument) -> Self {
        let mut items: Vec<(usize, u8, Item, Option<u32>)> =
            Vec::with_capacity(doc.sections().len() + doc.entries().len());
        items.extend(
            doc.sections()
                .iter()
                .map(|s| (s.line_range.start, 0, Item::Section(s.id), s.parent)),
        );
        items.extend(
            doc.entries()
                .iter()
                .map(|e| (e.line_range.start, 1, Item::Entry(e.id), e.section_id)),
        );
        // Stable sort: a section header comes before an entry on its first
        // line, and nested sections starting on the same line keep id order.
        items.sort_by_key(|(line, order, ..)| (*line, *order));

        let mut index = DocIndex {
            root: Vec::new(),
            children: vec![Vec::new(); doc.sections().len()],
        };
        for (_, _, item, parent) in items {
            match parent {
                Some(id) => index.children[id as usize].push(item),
                None => index.root.push(item),
            }
        }
        index
    }
}

/// What the user chose to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub show_info: bool,
    pub show_warnings: bool,
    pub show_errors: bool,
    /// Show engine chatter ([`LogEntry::is_engine_chatter`]).
    pub show_noise: bool,
    /// Merge entries with the same fingerprint within a section ("×42").
    pub collapse_duplicates: bool,
    /// Case-insensitive text search over messages, details, frames and files.
    pub search: String,
    pub hidden_categories: HashSet<Category>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            show_info: true,
            show_warnings: true,
            show_errors: true,
            show_noise: false,
            collapse_duplicates: true,
            search: String::new(),
            hidden_categories: HashSet::new(),
        }
    }
}

impl Filter {
    fn severity_visible(&self, severity: Severity) -> bool {
        match severity {
            Severity::Info => self.show_info,
            Severity::Warning => self.show_warnings,
            Severity::Error | Severity::Exception | Severity::Crash => self.show_errors,
        }
    }

    /// Whether `entry` is visible. `needle` is the lowercased search text.
    pub fn matches(&self, entry: &LogEntry, needle: &str) -> bool {
        if !self.severity_visible(entry.severity)
            || self.hidden_categories.contains(&entry.category)
            || (!self.show_noise && entry.is_engine_chatter())
        {
            return false;
        }
        needle.is_empty() || entry_contains(entry, needle)
    }
}

fn entry_contains(entry: &LogEntry, needle: &str) -> bool {
    let hit = |text: &str| text.to_lowercase().contains(needle);
    hit(&entry.message)
        || entry.details.iter().any(|d| hit(d))
        || entry
            .stack
            .iter()
            .any(|f| hit(&f.text) || f.location.as_ref().is_some_and(|l| hit(&l.path)))
        || entry.source.as_ref().is_some_and(|s| hit(&s.path))
}

/// One line of the entry list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Section {
        id: u32,
        depth: u16,
    },
    /// `repeats` > 1 when duplicates were collapsed into this row.
    Entry {
        id: u64,
        depth: u16,
        repeats: u32,
    },
}

/// The visible rows, in display order. Sections with nothing visible inside
/// are left out; collapsed sections show their header only.
pub fn build_rows(
    doc: &LogDocument,
    index: &DocIndex,
    filter: &Filter,
    collapsed: &HashSet<u32>,
) -> Vec<Row> {
    let mut builder = RowBuilder {
        doc,
        index,
        filter,
        collapsed,
        needle: filter.search.trim().to_lowercase(),
        rows: Vec::new(),
    };
    builder.level(&index.root, 0, true);
    builder.rows
}

struct RowBuilder<'a> {
    doc: &'a LogDocument,
    index: &'a DocIndex,
    filter: &'a Filter,
    collapsed: &'a HashSet<u32>,
    needle: String,
    rows: Vec<Row>,
}

impl RowBuilder<'_> {
    /// Adds the rows for `items` (when `emit`) and returns how many entries
    /// are visible among them, including inside nested sections.
    fn level(&mut self, items: &[Item], depth: u16, emit: bool) -> usize {
        let index = self.index;
        let mut seen: HashMap<u64, usize> = HashMap::new();
        let mut visible = 0;
        for &item in items {
            match item {
                Item::Entry(id) => {
                    let Some(entry) = self.doc.entry(id) else {
                        continue;
                    };
                    if !self.filter.matches(entry, &self.needle) {
                        continue;
                    }
                    visible += 1;
                    if !emit {
                        continue;
                    }
                    if self.filter.collapse_duplicates {
                        if let Some(&row) = seen.get(&entry.fingerprint) {
                            if let Row::Entry { repeats, .. } = &mut self.rows[row] {
                                *repeats += 1;
                            }
                            continue;
                        }
                        seen.insert(entry.fingerprint, self.rows.len());
                    }
                    self.rows.push(Row::Entry {
                        id,
                        depth,
                        repeats: 1,
                    });
                }
                Item::Section(id) => {
                    let header = self.rows.len();
                    if emit {
                        self.rows.push(Row::Section { id, depth });
                    }
                    let open = emit && !self.collapsed.contains(&id);
                    let inner = self.level(&index.children[id as usize], depth + 1, open);
                    if inner == 0 {
                        self.rows.truncate(header);
                    }
                    visible += inner;
                }
            }
        }
        visible
    }
}

/// Sections collapsed when a log opens: those without warnings or errors,
/// except play mode, scene and crash sections, where the game's own logs and
/// the interesting events live. A collapsed header still shows its duration.
pub fn default_collapsed(doc: &LogDocument) -> HashSet<u32> {
    doc.sections()
        .iter()
        .filter(|s| {
            s.counts.errors() + s.counts.warning == 0
                && !matches!(
                    s.kind,
                    SectionKind::PlayMode | SectionKind::SceneLoad | SectionKind::Crash
                )
        })
        .map(|s| s.id)
        .collect()
}

/// Sections containing `section_id`, outermost first, including itself.
pub fn section_path(doc: &LogDocument, section_id: Option<u32>) -> Vec<u32> {
    let mut path = Vec::new();
    let mut current = section_id;
    while let Some(id) = current {
        path.push(id);
        current = doc.section(id).and_then(|s| s.parent);
    }
    path.reverse();
    path
}

/// The next (or previous) visible warning-or-worse entry after `from`,
/// wrapping around. Searches the whole document, not just expanded rows, so
/// problems inside collapsed sections are found too.
pub fn find_problem(
    doc: &LogDocument,
    filter: &Filter,
    from: Option<u64>,
    forward: bool,
) -> Option<u64> {
    let needle = filter.search.trim().to_lowercase();
    let is_problem = |e: &&LogEntry| e.severity >= Severity::Warning && filter.matches(e, &needle);
    let entries = doc.entries();
    let start = from.map(|id| id as usize);
    if forward {
        let after = start.map_or(0, |i| i + 1);
        entries[after.min(entries.len())..]
            .iter()
            .chain(entries.iter())
            .find(is_problem)
            .map(|e| e.id)
    } else {
        let before = start.unwrap_or(entries.len()).min(entries.len());
        entries[..before]
            .iter()
            .rev()
            .chain(entries.iter().rev())
            .find(is_problem)
            .map(|e| e.id)
    }
}

/// Row showing entry `id`: the entry itself, or the row its duplicates were
/// collapsed into.
pub fn row_of_entry(rows: &[Row], doc: &LogDocument, id: u64) -> Option<usize> {
    let target = doc.entry(id)?;
    rows.iter()
        .position(|row| matches!(row, Row::Entry { id: row_id, .. } if *row_id == id))
        .or_else(|| {
            rows.iter().position(|row| match row {
                Row::Entry { id: row_id, .. } => doc.entry(*row_id).is_some_and(|e| {
                    e.fingerprint == target.fingerprint && e.section_id == target.section_id
                }),
                Row::Section { .. } => false,
            })
        })
}

/// Short "where" label for an entry row (see [`LogEntry::culprit_label`]).
pub fn culprit_label(entry: &LogEntry) -> Option<String> {
    entry.culprit_label()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use comte_core::{
        FrameKind, LogEntry, ParseEvent, SectionKind, SectionStart, SourceRef, StackFrame,
    };

    use super::*;

    fn doc(events: Vec<ParseEvent>) -> LogDocument {
        let mut doc = LogDocument::new();
        doc.apply_all(events);
        doc.finish();
        doc
    }

    fn e(severity: Severity, category: Category, message: &str, line: usize) -> ParseEvent {
        ParseEvent::Entry(LogEntry::new(severity, category, message, line..line + 1))
    }

    fn start(title: &str, line: usize) -> ParseEvent {
        ParseEvent::SectionStart(SectionStart {
            kind: SectionKind::PlayMode,
            title: title.to_owned(),
            line,
        })
    }

    fn end(line: usize) -> ParseEvent {
        ParseEvent::SectionEnd {
            line,
            duration: Some(Duration::from_secs(1)),
        }
    }

    fn sample() -> LogDocument {
        doc(vec![
            e(Severity::Info, Category::DomainReload, "Mono: chatter", 1),
            start("Play mode #1", 2),
            e(Severity::Info, Category::UserLog, "Loaded 3 items", 3),
            e(Severity::Info, Category::UserLog, "Loaded 7 items", 4),
            e(Severity::Error, Category::UserLog, "Save failed", 5),
            end(6),
            start("Play mode #2", 7),
            e(Severity::Info, Category::Licensing, "license ok", 8),
            end(9),
            e(
                Severity::Warning,
                Category::Compiler,
                "CS0618: obsolete",
                10,
            ),
        ])
    }

    fn render(rows: &[Row], doc: &LogDocument) -> Vec<String> {
        rows.iter()
            .map(|row| match *row {
                Row::Section { id, depth } => {
                    format!(
                        "{}[{}]",
                        "  ".repeat(depth as usize),
                        doc.section(id).unwrap().title
                    )
                }
                Row::Entry { id, depth, repeats } => {
                    let x = if repeats > 1 {
                        format!(" ×{repeats}")
                    } else {
                        String::new()
                    };
                    format!(
                        "{}{}{x}",
                        "  ".repeat(depth as usize),
                        doc.entry(id).unwrap().message
                    )
                }
            })
            .collect()
    }

    #[test]
    fn default_view_hides_chatter_collapses_duplicates_and_empty_sections() {
        let doc = sample();
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter::default(),
            &HashSet::new(),
        );
        assert_eq!(
            render(&rows, &doc),
            [
                "[Play mode #1]",
                "  Loaded 3 items ×2",
                "  Save failed",
                "CS0618: obsolete",
            ],
            "chatter hidden; Play mode #2 only had noise so it disappears"
        );
    }

    #[test]
    fn noise_and_no_collapse_show_everything() {
        let doc = sample();
        let filter = Filter {
            show_noise: true,
            collapse_duplicates: false,
            ..Filter::default()
        };
        let rows = build_rows(&doc, &DocIndex::new(&doc), &filter, &HashSet::new());
        assert_eq!(
            rows.len(),
            8,
            "1 chatter + 2 headers + 4 entries + 1 warning"
        );
    }

    #[test]
    fn collapsed_section_keeps_header_only() {
        let doc = sample();
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter::default(),
            &HashSet::from([0]),
        );
        assert_eq!(render(&rows, &doc), ["[Play mode #1]", "CS0618: obsolete"]);
    }

    #[test]
    fn search_and_severity_filters() {
        let doc = sample();
        let index = DocIndex::new(&doc);
        let search = Filter {
            search: "  SAVE ".to_owned(),
            ..Filter::default()
        };
        assert_eq!(
            render(&build_rows(&doc, &index, &search, &HashSet::new()), &doc),
            ["[Play mode #1]", "  Save failed"]
        );
        let errors_only = Filter {
            show_info: false,
            show_warnings: false,
            ..Filter::default()
        };
        assert_eq!(
            render(
                &build_rows(&doc, &index, &errors_only, &HashSet::new()),
                &doc
            ),
            ["[Play mode #1]", "  Save failed"]
        );
    }

    #[test]
    fn search_looks_into_frames_and_files() {
        let entry =
            LogEntry::new(Severity::Error, Category::Exception, "boom", 1..2).with_stack(vec![
                StackFrame::new("Spawner.Tick()", FrameKind::User)
                    .with_location(SourceRef::new("Assets/Enemies/Spawner.cs").with_line(9)),
            ]);
        let filter = Filter::default();
        assert!(filter.matches(&entry, "spawner.tick"));
        assert!(filter.matches(&entry, "enemies/"));
        assert!(!filter.matches(&entry, "player"));
    }

    #[test]
    fn problem_navigation_wraps_and_respects_filter() {
        let doc = sample();
        let filter = Filter::default();
        // Problems: "Save failed" (id 3) and "CS0618" (id 5).
        assert_eq!(find_problem(&doc, &filter, None, true), Some(3));
        assert_eq!(find_problem(&doc, &filter, Some(3), true), Some(5));
        assert_eq!(find_problem(&doc, &filter, Some(5), true), Some(3), "wraps");
        assert_eq!(
            find_problem(&doc, &filter, Some(3), false),
            Some(5),
            "wraps back"
        );
        let errors_only = Filter {
            show_warnings: false,
            ..Filter::default()
        };
        assert_eq!(find_problem(&doc, &errors_only, Some(3), true), Some(3));
    }

    #[test]
    fn collapsed_duplicate_maps_to_its_representative_row() {
        let doc = sample();
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter::default(),
            &HashSet::new(),
        );
        // id 2 ("Loaded 7 items") was merged into id 1's row.
        assert_eq!(row_of_entry(&rows, &doc, 2), Some(1));
        assert_eq!(section_path(&doc, Some(0)), [0]);
    }

    #[test]
    fn quiet_technical_sections_start_collapsed() {
        let doc = doc(vec![
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::DomainReload,
                title: "Domain reload".to_owned(),
                line: 1,
            }),
            e(Severity::Info, Category::DomainReload, "x", 2),
            end(3),
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::Compile,
                title: "Script compilation".to_owned(),
                line: 4,
            }),
            e(Severity::Error, Category::Compiler, "CS0103", 5),
            end(6),
            start("Play mode #1", 7),
            end(8),
        ]);
        assert_eq!(default_collapsed(&doc), HashSet::from([0]));
    }

    #[test]
    fn culprit_label_falls_back_to_first_user_frame() {
        let entry =
            LogEntry::new(Severity::Exception, Category::Exception, "x", 1..2).with_stack(vec![
                StackFrame::new("GameObject.SetActive(Boolean)", FrameKind::Engine),
                StackFrame::new("Gun.StopShooting()", FrameKind::User),
            ]);
        assert_eq!(culprit_label(&entry).as_deref(), Some("Gun.StopShooting()"));
    }

    #[test]
    fn index_orders_sections_before_entries_on_the_same_line() {
        let doc = doc(vec![
            start("Play mode #1", 1),
            e(
                Severity::Info,
                Category::PlayMode,
                "Entering play mode (#1)",
                1,
            ),
        ]);
        let rows = build_rows(
            &doc,
            &DocIndex::new(&doc),
            &Filter {
                show_noise: true,
                ..Filter::default()
            },
            &HashSet::new(),
        );
        assert_eq!(
            render(&rows, &doc),
            ["[Play mode #1]", "  Entering play mode (#1)"]
        );
    }
}
