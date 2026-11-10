//! What a log says at a glance (DESIGN §5): a one-line verdict, the most
//! repeated problems and messages, and the slowest engine operations.
//!
//! Everything points back at an entry id, so the UI can jump to it.

use std::collections::HashMap;
use std::time::Duration;

use crate::activity::{AssetImport, Unfinished};
use crate::document::LogDocument;
use crate::model::{Category, LogEntry, MetricKind, MetricValue, SectionKind, Severity};

const TOP_REPEATED: usize = 5;
const TOP_SLOWEST: usize = 8;
const TOP_IMPORTS: usize = 10;
const TOP_PHASES: usize = 6;

/// One domain reload step, added up over every reload in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTotal {
    pub name: String,
    pub about: Option<String>,
    pub your_code: bool,
    pub total: Duration,
    /// Per reload (over the reloads that had a breakdown).
    pub average: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictKind {
    Crashed,
    /// The log ends in the middle of an operation (stuck, or killed).
    Stopped,
    Errors,
    Warnings,
    Clean,
}

/// The one-line answer to "how did this session go?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub kind: VerdictKind,
    /// "Crashed in Scene #3", "5 errors (4 different)", …
    pub headline: String,
    /// Supporting lines: the likely cause, the most frequent error, …
    pub details: Vec<String>,
    /// The entry the headline is about (the crash, the top error).
    pub entry: Option<u64>,
}

/// Entries sharing a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repeated {
    /// First occurrence.
    pub entry: u64,
    pub count: usize,
    pub severity: Severity,
    pub headline: String,
    pub location: Option<String>,
}

/// One timed operation (domain reload, asset refresh, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timed {
    pub kind: MetricKind,
    pub duration: Duration,
    pub line: usize,
    /// The entry on the metric's line, to jump to.
    pub entry: Option<u64>,
}

/// Totals per kind of timed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingTotal {
    pub kind: MetricKind,
    pub count: usize,
    pub total: Duration,
    pub max: Duration,
}

impl TimingTotal {
    pub fn average(&self) -> Duration {
        self.total / u32::try_from(self.count.max(1)).unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insights {
    pub verdict: Verdict,
    /// Warnings and worse, most frequent first (only those seen twice or more).
    pub repeated_problems: Vec<Repeated>,
    /// Every distinct warning-or-worse problem, worst first, then most
    /// frequent, then earliest (for reports).
    pub problems: Vec<Repeated>,
    /// Any message worth reading (engine chatter excluded), most frequent first.
    pub repeated_messages: Vec<Repeated>,
    /// The slowest timed operations, slowest first.
    pub slowest: Vec<Timed>,
    /// Per-kind totals, in a fixed order.
    pub timings: Vec<TimingTotal>,
    /// The slowest asset imports, slowest first.
    pub slowest_imports: Vec<AssetImport>,
    /// How many imports finished, and how long they took together.
    pub imports: (usize, Duration),
    /// The most repeated message of any kind, engine chatter included: what
    /// fills a runaway log.
    pub loudest: Option<Repeated>,
    /// Where domain reload time goes: the costliest leaf steps, costliest first.
    pub reload_phases: Vec<PhaseTotal>,
}

impl Insights {
    pub fn of(doc: &LogDocument) -> Self {
        let repeated_problems = repeated(doc, |e| e.severity >= Severity::Warning, 2);
        let problems = all_problems(doc);
        // Summaries ("Domain reload took …") are operations, covered by timings.
        let repeated_messages = repeated(doc, |e| !e.is_engine_chatter() && !e.summary, 2);
        let timed = timed_metrics(doc);
        let mut slowest = timed.clone();
        slowest.sort_by(|a, b| b.duration.cmp(&a.duration).then(a.line.cmp(&b.line)));
        slowest.truncate(TOP_SLOWEST);
        let mut slowest_imports = doc.imports().to_vec();
        slowest_imports.sort_by(|a, b| b.duration.cmp(&a.duration).then(a.line.cmp(&b.line)));
        slowest_imports.truncate(TOP_IMPORTS);
        let imports = (
            doc.imports().len(),
            doc.imports().iter().map(|i| i.duration).sum(),
        );
        Self {
            verdict: verdict(doc),
            repeated_problems,
            problems,
            repeated_messages,
            slowest,
            timings: totals(&timed),
            slowest_imports,
            imports,
            loudest: repeated(doc, |_| true, 2).into_iter().next(),
            reload_phases: reload_phases(doc),
        }
    }
}

fn verdict(doc: &LogDocument) -> Verdict {
    let counts = doc.counts();
    if let Some(crash) = doc.entries().iter().find(|e| e.severity == Severity::Crash) {
        return crash_verdict(doc, crash);
    }
    // A log that ends mid-operation answers "why is it stuck?" before
    // anything else; errors are mentioned as a lead.
    if let Some(unfinished) = doc.unfinished() {
        return stopped_verdict(doc, unfinished);
    }
    if counts.errors() > 0 {
        let errors: Vec<&LogEntry> = doc
            .entries()
            .iter()
            .filter(|e| e.severity.is_error_like())
            .collect();
        let groups = group_by_fingerprint(errors.iter().copied());
        let distinct = groups.len();
        let headline = if distinct < errors.len() {
            format!(
                "{} {} ({distinct} different)",
                errors.len(),
                plural(errors.len(), "error")
            )
        } else {
            format!("{} {}", errors.len(), plural(errors.len(), "error"))
        };
        let mut details = Vec::new();
        if let Some(failed) = doc
            .entries()
            .iter()
            .rev()
            .find(|e| e.category == Category::Build && e.severity.is_error_like())
        {
            let compiler_errors = doc
                .entries()
                .iter()
                .filter(|e| e.category == Category::Compiler && e.severity.is_error_like())
                .count();
            details.push(format!(
                "{} ({compiler_errors} compiler {})",
                failed.message,
                plural(compiler_errors, "error")
            ));
        }
        // A repeated error is the loudest lead; without repeats, the first
        // error is usually the root cause of the ones after it.
        let (top, count) = groups
            .iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.id.cmp(&a.0.id)))
            .map(|(e, c)| (*e, *c))
            .expect("errors exist");
        let (top, count, label) = if count > 1 {
            (top, count, "Most frequent")
        } else {
            (
                groups[0].0,
                1,
                if groups.len() > 1 {
                    "First error"
                } else {
                    "Error"
                },
            )
        };
        let at = top
            .culprit_label()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default();
        let times = if count > 1 {
            format!(" ×{count}")
        } else {
            String::new()
        };
        details.push(format!("{label}: {}{times}{at}", top.headline(70)));
        return Verdict {
            kind: VerdictKind::Errors,
            headline,
            details,
            entry: Some(top.id),
        };
    }
    if counts.warning > 0 {
        let first = doc
            .entries()
            .iter()
            .find(|e| e.severity == Severity::Warning);
        return Verdict {
            kind: VerdictKind::Warnings,
            headline: format!(
                "No errors, {} {}",
                counts.warning,
                plural(counts.warning as usize, "warning")
            ),
            details: first
                .map(|e| format!("First: {}", e.headline(70)))
                .into_iter()
                .collect(),
            entry: first.map(|e| e.id),
        };
    }
    Verdict {
        kind: VerdictKind::Clean,
        headline: "Clean: no warnings or errors".to_owned(),
        details: Vec::new(),
        entry: None,
    }
}

fn reload_phases(doc: &LogDocument) -> Vec<PhaseTotal> {
    let profiles = doc.reload_profiles();
    let mut totals: Vec<PhaseTotal> = Vec::new();
    for phase in profiles.iter().flat_map(|p| p.leaves()) {
        match totals.iter_mut().find(|t| t.name == phase.name) {
            Some(total) => total.total += phase.duration,
            None => totals.push(PhaseTotal {
                name: phase.name.clone(),
                about: phase.about.clone(),
                your_code: phase.your_code,
                total: phase.duration,
                average: Duration::ZERO,
            }),
        }
    }
    let reloads = u32::try_from(profiles.len().max(1)).unwrap_or(u32::MAX);
    for total in &mut totals {
        total.average = total.total / reloads;
    }
    totals.retain(|t| !t.total.is_zero());
    totals.sort_by_key(|t| std::cmp::Reverse(t.total));
    totals.truncate(TOP_PHASES);
    totals
}

fn stopped_verdict(doc: &LogDocument, unfinished: &Unfinished) -> Verdict {
    let mut details = vec![match unfinished.running_for {
        Some(d) => format!(
            "Started on line {} and still not finished {} later, at the log's last line. {}",
            unfinished.line,
            crate::model::format_duration(d),
            unfinished.hint
        ),
        None => format!(
            "Started on line {} and never finished. {}",
            unfinished.line, unfinished.hint
        ),
    }];
    match unfinished.also.as_slice() {
        [] => {}
        [one] => details.push(format!("Also still running: {one}")),
        [first, rest @ ..] => details.push(format!(
            "Also still running: {first} and {} more",
            rest.len()
        )),
    }
    if let Some(error) = doc
        .entries()
        .iter()
        .rev()
        .find(|e| e.severity.is_error_like())
    {
        details.push(format!("Last error before it: {}", error.headline(70)));
    }
    Verdict {
        kind: VerdictKind::Stopped,
        headline: unfinished.headline.clone(),
        details,
        entry: doc.entry_at_line(unfinished.line).map(|e| e.id),
    }
}

fn crash_verdict(doc: &LogDocument, crash: &LogEntry) -> Verdict {
    // Where: the section around the Crash section ("Scene #3", "Play mode #2").
    let place = enclosing_titles(doc, crash.section_id)
        .into_iter()
        .rev()
        .find(|(kind, _)| *kind != SectionKind::Crash)
        .map(|(_, title)| title);
    let headline = match &place {
        Some(title) => format!("Crashed in {title}"),
        None => "Crashed".to_owned(),
    };
    let mut details = Vec::new();
    // Likely cause: the last exception or error logged before the crash.
    if let Some(cause) = doc.entries()[..crash.id as usize]
        .iter()
        .rev()
        .find(|e| e.severity.is_error_like())
    {
        let at = cause
            .culprit_label()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default();
        details.push(format!("Last error before it: {}{at}", cause.headline(70)));
    }
    if let Some(frame) = crash
        .stack
        .iter()
        .find(|f| f.kind == crate::model::FrameKind::User)
    {
        details.push(format!("Crashed in game code: {}", frame.text));
    } else if let Some(frame) = crash.stack.first() {
        details.push(format!("Top native frame: {}", frame.text));
    }
    // What the crash looks like (in a GPU driver, in game code, …).
    if let Some(hint) = doc.hints_for(crash).next() {
        details.push(hint.title.clone());
    }
    Verdict {
        kind: VerdictKind::Crashed,
        headline,
        details,
        entry: Some(crash.id),
    }
}

/// (kind, title) of the sections containing `section`, outermost first.
fn enclosing_titles(doc: &LogDocument, section: Option<u32>) -> Vec<(SectionKind, String)> {
    let mut titles = Vec::new();
    let mut current = section;
    while let Some(id) = current {
        let Some(s) = doc.section(id) else { break };
        titles.push((s.kind, s.title.clone()));
        current = s.parent;
    }
    titles.reverse();
    titles
}

/// Groups entries by fingerprint, keeping the first occurrence; in order of
/// first appearance.
fn group_by_fingerprint<'a>(
    entries: impl Iterator<Item = &'a LogEntry>,
) -> Vec<(&'a LogEntry, usize)> {
    let mut index: HashMap<u64, usize> = HashMap::new();
    let mut groups: Vec<(&LogEntry, usize)> = Vec::new();
    for entry in entries {
        match index.get(&entry.fingerprint) {
            Some(&i) => groups[i].1 += 1,
            None => {
                index.insert(entry.fingerprint, groups.len());
                groups.push((entry, 1));
            }
        }
    }
    groups
}

fn all_problems(doc: &LogDocument) -> Vec<Repeated> {
    let mut groups = group_by_fingerprint(
        doc.entries()
            .iter()
            .filter(|e| e.severity >= Severity::Warning),
    );
    groups.sort_by(|a, b| {
        b.0.severity
            .cmp(&a.0.severity)
            .then(b.1.cmp(&a.1))
            .then(a.0.id.cmp(&b.0.id))
    });
    groups
        .into_iter()
        .map(|(entry, count)| Repeated {
            entry: entry.id,
            count,
            severity: entry.severity,
            headline: entry.headline(90),
            location: entry.culprit_label(),
        })
        .collect()
}

fn repeated(
    doc: &LogDocument,
    keep: impl Fn(&LogEntry) -> bool,
    min_count: usize,
) -> Vec<Repeated> {
    let mut groups = group_by_fingerprint(doc.entries().iter().filter(|e| keep(e)));
    groups.retain(|(_, count)| *count >= min_count);
    // Most frequent first; ties: worse severity, then earlier.
    groups.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.0.severity.cmp(&a.0.severity))
            .then(a.0.id.cmp(&b.0.id))
    });
    groups
        .into_iter()
        .take(TOP_REPEATED)
        .map(|(entry, count)| Repeated {
            entry: entry.id,
            count,
            severity: entry.severity,
            headline: entry.headline(90),
            location: entry.culprit_label(),
        })
        .collect()
}

fn timed_metrics(doc: &LogDocument) -> Vec<Timed> {
    doc.metrics()
        .iter()
        .filter_map(|m| match m.value {
            MetricValue::Duration(duration) => Some(Timed {
                kind: m.kind,
                duration,
                line: m.line,
                entry: entry_at_line(doc, m.line),
            }),
            MetricValue::Count(_) => None,
        })
        .collect()
}

/// The entry whose lines include `line` (entries are in line order).
pub fn entry_at_line(doc: &LogDocument, line: usize) -> Option<u64> {
    let entries = doc.entries();
    let after = entries.partition_point(|e| e.line_range.start <= line);
    entries[..after]
        .iter()
        .rev()
        .take(64) // overlapping ranges are short; don't scan the whole log
        .find(|e| e.line_range.contains(&line))
        .map(|e| e.id)
}

fn totals(timed: &[Timed]) -> Vec<TimingTotal> {
    const ORDER: [MetricKind; 6] = [
        MetricKind::Startup,
        MetricKind::Compile,
        MetricKind::DomainReload,
        MetricKind::AssetRefresh,
        MetricKind::SceneLoad,
        MetricKind::SceneUnload,
    ];
    ORDER
        .iter()
        .filter_map(|&kind| {
            let durations: Vec<Duration> = timed
                .iter()
                .filter(|t| t.kind == kind)
                .map(|t| t.duration)
                .collect();
            (!durations.is_empty()).then(|| TimingTotal {
                kind,
                count: durations.len(),
                total: durations.iter().sum(),
                max: durations.iter().copied().max().unwrap_or_default(),
            })
        })
        .collect()
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FrameKind, Metric, SourceRef, StackFrame};
    use crate::parser::{ParseEvent, SectionStart};

    fn entry(severity: Severity, category: Category, message: &str, line: usize) -> LogEntry {
        LogEntry::new(severity, category, message, line..line + 1)
    }

    fn doc(events: Vec<ParseEvent>) -> LogDocument {
        let mut doc = LogDocument::new();
        doc.apply_all(events);
        doc.finish();
        doc
    }

    #[test]
    fn a_log_ending_mid_operation_says_where_it_stopped() {
        let insights = Insights::of(&doc(vec![
            ParseEvent::Entry(entry(Severity::Error, Category::UserLog, "Save failed", 2)),
            ParseEvent::Entry(entry(
                Severity::Info,
                Category::AssetPipeline,
                "Start importing Assets/Ship.fbx",
                5,
            )),
            ParseEvent::Import(AssetImport {
                path: "Assets/a.png".to_owned(),
                duration: Duration::from_millis(20),
                line: 3,
            }),
            ParseEvent::Import(AssetImport {
                path: "Assets/b.png".to_owned(),
                duration: Duration::from_millis(900),
                line: 4,
            }),
            ParseEvent::Unfinished(Unfinished {
                headline: "Stopped while importing Assets/Ship.fbx".to_owned(),
                hint: "Move it out of Assets to confirm.".to_owned(),
                subject: Some("Assets/Ship.fbx".to_owned()),
                line: 5,
                also: vec!["importing Assets/c.png".to_owned()],
                running_for: None,
            }),
        ]));
        let v = &insights.verdict;
        assert_eq!(v.kind, VerdictKind::Stopped, "stopping outranks errors");
        assert_eq!(v.headline, "Stopped while importing Assets/Ship.fbx");
        assert_eq!(
            v.details,
            [
                "Started on line 5 and never finished. Move it out of Assets to confirm.",
                "Also still running: importing Assets/c.png",
                "Last error before it: Save failed",
            ]
        );
        assert_eq!(v.entry, Some(1), "points at the entry on the start line");
        assert_eq!(insights.imports, (2, Duration::from_millis(920)));
        assert_eq!(insights.slowest_imports[0].path, "Assets/b.png");
    }

    #[test]
    fn clean_and_warning_verdicts() {
        let clean = Insights::of(&doc(vec![ParseEvent::Entry(entry(
            Severity::Info,
            Category::UserLog,
            "hello",
            1,
        ))]));
        assert_eq!(clean.verdict.kind, VerdictKind::Clean);
        assert_eq!(clean.verdict.headline, "Clean: no warnings or errors");

        let warned = Insights::of(&doc(vec![ParseEvent::Entry(entry(
            Severity::Warning,
            Category::Compiler,
            "CS0618: 'WWW' is obsolete",
            3,
        ))]));
        assert_eq!(warned.verdict.kind, VerdictKind::Warnings);
        assert_eq!(warned.verdict.headline, "No errors, 1 warning");
        assert_eq!(warned.verdict.details, ["First: CS0618: 'WWW' is obsolete"]);
    }

    #[test]
    fn error_verdict_names_the_most_frequent_error() {
        let nre = |line| {
            ParseEvent::Entry(
                entry(
                    Severity::Exception,
                    Category::Exception,
                    "NullReferenceException: Object reference not set",
                    line,
                )
                .with_source(SourceRef::new("Assets/Ui/Overlay.cs").with_line(209)),
            )
        };
        let insights = Insights::of(&doc(vec![
            ParseEvent::Entry(entry(Severity::Error, Category::UserLog, "Save failed", 1)),
            nre(2),
            nre(3),
            nre(4),
        ]));
        let v = &insights.verdict;
        assert_eq!(v.kind, VerdictKind::Errors);
        assert_eq!(v.headline, "4 errors (2 different)");
        assert_eq!(
            v.details,
            ["Most frequent: NullReferenceException ×3 at Overlay.cs:209"]
        );
        assert_eq!(v.entry, Some(1));
        assert_eq!(
            insights.repeated_problems.len(),
            1,
            "Save failed happened once"
        );
        assert_eq!(insights.repeated_problems[0].count, 3);
        let all: Vec<_> = insights
            .problems
            .iter()
            .map(|p| (p.headline.as_str(), p.count))
            .collect();
        assert_eq!(
            all,
            [("NullReferenceException", 3), ("Save failed", 1)],
            "exceptions rank above errors"
        );
    }

    #[test]
    fn without_repeats_the_first_error_leads() {
        let insights = Insights::of(&doc(vec![
            ParseEvent::Entry(entry(
                Severity::Exception,
                Category::Exception,
                "TokenException: expired",
                1,
            )),
            ParseEvent::Entry(entry(Severity::Error, Category::UserLog, "Save failed", 2)),
            ParseEvent::Entry(
                entry(
                    Severity::Info,
                    Category::DomainReload,
                    "Domain reload took 1.30 s",
                    3,
                )
                .as_summary(),
            ),
            ParseEvent::Entry(
                entry(
                    Severity::Info,
                    Category::DomainReload,
                    "Domain reload took 2.50 s",
                    4,
                )
                .as_summary(),
            ),
        ]));
        assert_eq!(insights.verdict.headline, "2 errors");
        assert_eq!(insights.verdict.details, ["First error: TokenException"]);
        assert_eq!(insights.verdict.entry, Some(0));
        assert!(
            insights.repeated_messages.is_empty(),
            "summaries are not repeated messages"
        );
    }

    #[test]
    fn crash_verdict_names_place_and_cause() {
        let crash = entry(Severity::Crash, Category::Crash, "Native crash", 20).with_stack(vec![
            StackFrame::new("Spawner_Spawn (GameAssembly)", FrameKind::User),
        ]);
        let insights = Insights::of(&doc(vec![
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::SceneLoad,
                title: "Scene #3".to_owned(),
                line: 10,
            }),
            ParseEvent::Entry(
                entry(
                    Severity::Exception,
                    Category::Exception,
                    "NullReferenceException: boom",
                    12,
                )
                .with_stack(vec![StackFrame::new("Gun.StopShooting()", FrameKind::User)]),
            ),
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::Crash,
                title: "Crash".to_owned(),
                line: 20,
            }),
            ParseEvent::Entry(crash),
        ]));
        let v = &insights.verdict;
        assert_eq!(v.kind, VerdictKind::Crashed);
        assert_eq!(v.headline, "Crashed in Scene #3");
        assert_eq!(
            v.details,
            [
                "Last error before it: NullReferenceException at Gun.StopShooting()",
                "Crashed in game code: Spawner_Spawn (GameAssembly)"
            ]
        );
        assert_eq!(v.entry, Some(1));
    }

    #[test]
    fn slowest_operations_and_totals() {
        let insights = Insights::of(&doc(vec![
            ParseEvent::Entry(entry(
                Severity::Info,
                Category::DomainReload,
                "Domain reload took 1.30 s",
                5,
            )),
            ParseEvent::Metric(Metric::duration(
                MetricKind::DomainReload,
                Duration::from_millis(1300),
                5,
            )),
            ParseEvent::Metric(Metric::duration(
                MetricKind::DomainReload,
                Duration::from_millis(7250),
                40,
            )),
            ParseEvent::Metric(Metric::duration(
                MetricKind::AssetRefresh,
                Duration::from_millis(3530),
                20,
            )),
            ParseEvent::Metric(Metric::count(MetricKind::LoadedObjects, 3000, 21)),
        ]));
        let order: Vec<_> = insights
            .slowest
            .iter()
            .map(|t| t.duration.as_millis())
            .collect();
        assert_eq!(order, [7250, 3530, 1300]);
        assert_eq!(
            insights.slowest[2].entry,
            Some(0),
            "metric line maps to its entry"
        );
        assert_eq!(insights.slowest[0].entry, None);
        let reload = &insights.timings[0];
        assert_eq!(reload.kind, MetricKind::DomainReload);
        assert_eq!((reload.count, reload.max), (2, Duration::from_millis(7250)));
        assert_eq!(reload.average(), Duration::from_millis(4275));
        assert_eq!(insights.timings.len(), 2, "counts are not timings");
    }

    #[test]
    fn headlines_shorten_exceptions_and_long_messages() {
        let e = entry(
            Severity::Exception,
            Category::Exception,
            "System.IO.IOException: disk full",
            1,
        );
        assert_eq!(e.headline(40), "IOException");
        let generic = entry(
            Severity::Exception,
            Category::Exception,
            "System.Exception: Steam is not running",
            1,
        );
        assert_eq!(
            generic.headline(40),
            "System.Exception: Steam is not running"
        );
        let long = entry(Severity::Info, Category::UserLog, &"a".repeat(50), 1);
        assert_eq!(long.headline(10).chars().count(), 10);
        let colon = entry(
            Severity::Error,
            Category::Compiler,
            "CS0103: The name 'x' does not exist",
            1,
        );
        assert_eq!(colon.headline(80), "CS0103: The name 'x' does not exist");
    }
}
