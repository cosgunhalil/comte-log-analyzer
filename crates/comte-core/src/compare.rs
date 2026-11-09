//! Comparing two logs (DESIGN §7), typically `Editor-prev.log` (A) with
//! `Editor.log` (B): which problems are new, which are gone, which stayed,
//! how timings moved, and what changed in the environment.
//!
//! Problems are matched with [`fingerprint::across_runs`]: same category,
//! same message (numbers and ids ignored), same file, any line.

use std::collections::HashMap;
use std::time::Duration;

use crate::document::LogDocument;
use crate::fingerprint;
use crate::insights::{Insights, TimingTotal};
use crate::model::{FactKey, LogEntry, MetricKind, Severity, SeverityCounts};

/// One problem as seen in both logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemDiff {
    pub severity: Severity,
    pub headline: String,
    pub location: Option<String>,
    pub count_a: usize,
    pub count_b: usize,
    /// First occurrence in each log, to jump to.
    pub entry_a: Option<u64>,
    pub entry_b: Option<u64>,
}

/// How one kind of timed operation moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingDiff {
    pub kind: MetricKind,
    pub a: Option<TimingTotal>,
    pub b: Option<TimingTotal>,
}

impl TimingDiff {
    /// Change of the average, in percent (`None` if either side is missing).
    pub fn change_percent(&self) -> Option<f64> {
        let a = self.a.as_ref()?.average().as_secs_f64();
        let b = self.b.as_ref()?.average().as_secs_f64();
        (a > 0.0).then(|| (b - a) / a * 100.0)
    }

    /// Clearly slower: average up by more than 25 % and by more than 100 ms.
    pub fn is_regression(&self) -> bool {
        let (Some(a), Some(b)) = (&self.a, &self.b) else {
            return false;
        };
        let (a, b) = (a.average(), b.average());
        b > a + Duration::from_millis(100) && self.change_percent().is_some_and(|p| p > 25.0)
    }

    /// Clearly faster, by the same thresholds.
    pub fn is_improvement(&self) -> bool {
        let (Some(a), Some(b)) = (&self.a, &self.b) else {
            return false;
        };
        let (a, b) = (a.average(), b.average());
        a > b + Duration::from_millis(100) && self.change_percent().is_some_and(|p| p < -25.0)
    }
}

/// `+79 %`, `-16 %`, and `±0 %` for changes that round to zero.
pub fn format_percent(percent: f64) -> String {
    if percent.round() == 0.0 {
        "±0 %".to_owned()
    } else {
        format!("{percent:+.0} %")
    }
}

/// An environment fact whose value differs (e.g. a Unity upgrade).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactDiff {
    pub key: FactKey,
    pub a: Option<String>,
    pub b: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub counts_a: SeverityCounts,
    pub counts_b: SeverityCounts,
    /// In B but not in A, worst and most frequent first.
    pub new: Vec<ProblemDiff>,
    /// In A but not in B.
    pub gone: Vec<ProblemDiff>,
    /// In both.
    pub common: Vec<ProblemDiff>,
    pub timings: Vec<TimingDiff>,
    pub facts: Vec<FactDiff>,
}

impl Comparison {
    pub fn of(a: &LogDocument, b: &LogDocument) -> Self {
        let groups_a = Groups::of(a);
        let groups_b = Groups::of(b);
        let mut new = Vec::new();
        let mut gone = Vec::new();
        let mut common = Vec::new();

        for (key, group_b) in &groups_b.ordered {
            match groups_a.get(*key) {
                Some(group_a) => common.push(diff(Some(group_a), Some(group_b))),
                None => new.push(diff(None, Some(group_b))),
            }
        }
        for (key, group_a) in &groups_a.ordered {
            if groups_b.get(*key).is_none() {
                gone.push(diff(Some(group_a), None));
            }
        }
        let order_b = |d: &ProblemDiff| d.entry_b.unwrap_or(u64::MAX);
        let order_a = |d: &ProblemDiff| d.entry_a.unwrap_or(u64::MAX);
        new.sort_by(|x, y| {
            y.severity
                .cmp(&x.severity)
                .then(y.count_b.cmp(&x.count_b))
                .then(order_b(x).cmp(&order_b(y)))
        });
        gone.sort_by(|x, y| {
            y.severity
                .cmp(&x.severity)
                .then(y.count_a.cmp(&x.count_a))
                .then(order_a(x).cmp(&order_a(y)))
        });
        common.sort_by(|x, y| {
            let growth = |d: &ProblemDiff| d.count_b as i64 - d.count_a as i64;
            y.severity
                .cmp(&x.severity)
                .then(growth(y).cmp(&growth(x)))
                .then(order_b(x).cmp(&order_b(y)))
        });

        Self {
            counts_a: *a.counts(),
            counts_b: *b.counts(),
            new,
            gone,
            common,
            timings: timing_diffs(&Insights::of(a).timings, &Insights::of(b).timings),
            facts: fact_diffs(a, b),
        }
    }

    pub fn is_identical(&self) -> bool {
        self.new.is_empty()
            && self.gone.is_empty()
            && self.common.iter().all(|d| d.count_a == d.count_b)
    }
}

struct Group<'a> {
    first: &'a LogEntry,
    count: usize,
}

/// Warnings and worse, grouped by their across-runs fingerprint.
struct Groups<'a> {
    /// In order of first appearance.
    ordered: Vec<(u64, Group<'a>)>,
    index: HashMap<u64, usize>,
}

impl<'a> Groups<'a> {
    fn of(doc: &'a LogDocument) -> Self {
        let mut groups = Groups {
            ordered: Vec::new(),
            index: HashMap::new(),
        };
        for entry in doc
            .entries()
            .iter()
            .filter(|e| e.severity >= Severity::Warning)
        {
            let key = fingerprint::across_runs(entry);
            match groups.index.get(&key) {
                Some(&i) => groups.ordered[i].1.count += 1,
                None => {
                    groups.index.insert(key, groups.ordered.len());
                    groups.ordered.push((
                        key,
                        Group {
                            first: entry,
                            count: 1,
                        },
                    ));
                }
            }
        }
        groups
    }

    fn get(&self, key: u64) -> Option<&Group<'a>> {
        self.index.get(&key).map(|&i| &self.ordered[i].1)
    }
}

fn diff(a: Option<&Group>, b: Option<&Group>) -> ProblemDiff {
    let sample = b.or(a).expect("at least one side").first;
    ProblemDiff {
        severity: sample.severity,
        headline: sample.headline(90),
        location: sample.culprit_label(),
        count_a: a.map_or(0, |g| g.count),
        count_b: b.map_or(0, |g| g.count),
        entry_a: a.map(|g| g.first.id),
        entry_b: b.map(|g| g.first.id),
    }
}

fn timing_diffs(a: &[TimingTotal], b: &[TimingTotal]) -> Vec<TimingDiff> {
    let mut kinds: Vec<MetricKind> = Vec::new();
    for kind in a.iter().chain(b).map(|t| t.kind) {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
        .into_iter()
        .map(|kind| TimingDiff {
            kind,
            a: a.iter().find(|t| t.kind == kind).cloned(),
            b: b.iter().find(|t| t.kind == kind).cloned(),
        })
        .collect()
}

/// Facts that differ. The start date always differs, so it is left out.
fn fact_diffs(a: &LogDocument, b: &LogDocument) -> Vec<FactDiff> {
    let mut keys: Vec<FactKey> = Vec::new();
    for fact in a.facts().iter().chain(b.facts()) {
        if fact.key != FactKey::StartDate && !keys.contains(&fact.key) {
            keys.push(fact.key.clone());
        }
    }
    keys.into_iter()
        .filter_map(|key| {
            let va = a.fact(&key).map(str::to_owned);
            let vb = b.fact(&key).map(str::to_owned);
            (va != vb).then_some(FactDiff { key, a: va, b: vb })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, Fact, Metric, SourceRef};
    use crate::parser::ParseEvent;

    fn problem(severity: Severity, message: &str, file: &str, line: u32, at: usize) -> ParseEvent {
        ParseEvent::Entry(
            LogEntry::new(severity, Category::UserLog, message, at..at + 1)
                .with_source(SourceRef::new(file).with_line(line)),
        )
    }

    fn doc(events: Vec<ParseEvent>) -> LogDocument {
        let mut doc = LogDocument::new();
        doc.apply_all(events);
        doc.finish();
        doc
    }

    #[test]
    fn new_gone_and_common_problems() {
        let a = doc(vec![
            problem(
                Severity::Error,
                "Save failed for slot 1",
                "Assets/Save.cs",
                77,
                1,
            ),
            problem(
                Severity::Warning,
                "Inventory almost full",
                "Assets/Inv.cs",
                58,
                2,
            ),
            problem(
                Severity::Warning,
                "Inventory almost full",
                "Assets/Inv.cs",
                58,
                3,
            ),
        ]);
        let b = doc(vec![
            // Same error, line moved after an edit, other slot number: still "common".
            problem(
                Severity::Error,
                "Save failed for slot 2",
                "Assets/Save.cs",
                81,
                1,
            ),
            problem(
                Severity::Error,
                "Save failed for slot 3",
                "Assets/Save.cs",
                81,
                2,
            ),
            problem(
                Severity::Exception,
                "NullReferenceException: boom",
                "Assets/Spawner.cs",
                57,
                3,
            ),
        ]);
        let c = Comparison::of(&a, &b);
        assert_eq!(c.new.len(), 1);
        assert_eq!(c.new[0].headline, "NullReferenceException");
        assert_eq!((c.new[0].count_a, c.new[0].count_b), (0, 1));
        assert_eq!(c.gone.len(), 1);
        assert_eq!(c.gone[0].headline, "Inventory almost full");
        assert_eq!(c.gone[0].count_a, 2);
        assert_eq!(c.common.len(), 1);
        assert_eq!((c.common[0].count_a, c.common[0].count_b), (1, 2));
        assert_eq!(
            c.common[0].location.as_deref(),
            Some("Save.cs:81"),
            "described as in B"
        );
        assert_eq!(
            (c.common[0].entry_a, c.common[0].entry_b),
            (Some(0), Some(0))
        );
        assert!(!c.is_identical());
    }

    #[test]
    fn a_log_compared_with_itself_is_identical() {
        let a = doc(vec![problem(Severity::Error, "boom", "Assets/A.cs", 1, 1)]);
        let c = Comparison::of(&a, &a);
        assert!(c.is_identical());
        assert!(c.facts.is_empty());
    }

    #[test]
    fn timing_changes_and_thresholds() {
        let reload = |ms, at| {
            ParseEvent::Metric(Metric::duration(
                MetricKind::DomainReload,
                Duration::from_millis(ms),
                at,
            ))
        };
        let refresh = |ms, at| {
            ParseEvent::Metric(Metric::duration(
                MetricKind::AssetRefresh,
                Duration::from_millis(ms),
                at,
            ))
        };
        let a = doc(vec![reload(1300, 1), reload(1500, 2), refresh(100, 3)]);
        let b = doc(vec![reload(2400, 1), reload(2600, 2), refresh(90, 3)]);
        let c = Comparison::of(&a, &b);
        let reload_diff = c
            .timings
            .iter()
            .find(|t| t.kind == MetricKind::DomainReload)
            .unwrap();
        assert!(reload_diff.is_regression(), "1.40 s → 2.50 s average");
        assert_eq!(reload_diff.change_percent().map(|p| p.round()), Some(79.0));
        let refresh_diff = c
            .timings
            .iter()
            .find(|t| t.kind == MetricKind::AssetRefresh)
            .unwrap();
        assert!(
            !refresh_diff.is_regression() && !refresh_diff.is_improvement(),
            "10 ms is noise"
        );

        let only_b = Comparison::of(&doc(vec![]), &b);
        assert!(
            only_b
                .timings
                .iter()
                .all(|t| t.a.is_none() && t.change_percent().is_none())
        );
    }

    #[test]
    fn percent_formatting() {
        assert_eq!(format_percent(79.4), "+79 %");
        assert_eq!(format_percent(-16.2), "-16 %");
        assert_eq!(format_percent(-0.4), "±0 %");
    }

    #[test]
    fn environment_changes_ignore_start_date() {
        let facts = |version: &str, date: &str| {
            doc(vec![
                ParseEvent::Fact(Fact::new(FactKey::EngineVersion, version, 1)),
                ParseEvent::Fact(Fact::new(FactKey::StartDate, date, 2)),
                ParseEvent::Fact(Fact::new(FactKey::Gpu, "RTX 2070", 3)),
            ])
        };
        let c = Comparison::of(
            &facts("6000.3.10f1", "2026-11-01"),
            &facts("6000.3.11f1", "2026-11-05"),
        );
        assert_eq!(
            c.facts,
            [FactDiff {
                key: FactKey::EngineVersion,
                a: Some("6000.3.10f1".to_owned()),
                b: Some("6000.3.11f1".to_owned()),
            }]
        );
    }

    #[test]
    fn across_runs_fingerprint_ignores_lines_but_not_files() {
        let at = |file: &str, line| {
            LogEntry::new(Severity::Error, Category::UserLog, "boom", 1..2)
                .with_source(SourceRef::new(file).with_line(line))
        };
        assert_eq!(
            fingerprint::across_runs(&at("Assets/A.cs", 1)),
            fingerprint::across_runs(&at("Assets/A.cs", 99))
        );
        assert_ne!(
            fingerprint::across_runs(&at("Assets/A.cs", 1)),
            fingerprint::across_runs(&at("Assets/B.cs", 1))
        );
    }
}
