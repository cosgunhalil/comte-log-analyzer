//! A short, pasteable report of a log (DESIGN §6.3): verdict, note and tags,
//! top problems with where they happened and a trimmed stack, timings,
//! environment, timeline, and optionally what changed since another log.
//!
//! Two styles:
//! - [`ReportStyle::Markdown`] for Jira, GitHub, Confluence (headings, tables).
//! - [`ReportStyle::Slack`] for Slack's `mrkdwn`, which has no headings and no
//!   tables: headings become bold lines, tables become bullet lines.
//!
//! Log text always goes into code spans or code blocks, so characters like
//! `|`, `*` or `_` in messages can't break the formatting.

use crate::compare::{Comparison, ProblemDiff, format_percent};
use crate::document::LogDocument;
use crate::insights::{Insights, VerdictKind};
use crate::model::{FactKey, FrameKind, LogEntry, Severity, format_duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReportStyle {
    #[default]
    Markdown,
    Slack,
}

impl ReportStyle {
    pub fn label(self) -> &'static str {
        match self {
            ReportStyle::Markdown => "Markdown (Jira, GitHub, Confluence)",
            ReportStyle::Slack => "Slack",
        }
    }
}

/// Everything a report is made from.
#[derive(Debug, Clone, Copy)]
pub struct ReportInput<'a> {
    pub title: &'a str,
    /// "Unity Editor", "Unity Player", …
    pub kind_label: &'a str,
    pub doc: &'a LogDocument,
    pub insights: &'a Insights,
    pub note: &'a str,
    pub tags: &'a [String],
    /// Shown in the header, e.g. `2026-11-09 14:05`.
    pub generated_at: &'a str,
    /// Title of log A and the comparison of A with this log.
    pub comparison: Option<(&'a str, &'a Comparison)>,
    /// How many problems get their own section.
    pub max_problems: usize,
}

/// Stack frames shown per problem; the rest is summarized.
const MAX_FRAMES: usize = 8;

pub fn render_report(input: &ReportInput<'_>, style: ReportStyle) -> String {
    let mut out = Out::new(style);
    let doc = input.doc;

    out.h1(&format!("Comte Logger report: {}", input.title));
    let mut meta = vec![out.bold(input.kind_label)];
    if let Some(version) = doc.fact(&FactKey::EngineVersion) {
        meta.push(format!("Unity {version}"));
    }
    meta.push(format!("{} lines", doc.line_count()));
    meta.push(format!("generated {}", input.generated_at));
    out.para(&meta.join(" · "));

    // Verdict
    let verdict = &input.insights.verdict;
    let icon = match verdict.kind {
        VerdictKind::Crashed => "💥",
        VerdictKind::Errors => "⛔",
        VerdictKind::Warnings => "⚠️",
        VerdictKind::Clean => "✅",
    };
    let mut quote = vec![format!("{icon} {}", out.bold(&verdict.headline))];
    quote.extend(verdict.details.iter().cloned());
    out.quote(&quote);

    if !input.note.trim().is_empty() || !input.tags.is_empty() {
        out.h2("Note");
        if !input.note.trim().is_empty() {
            for paragraph in input.note.trim().split("\n\n") {
                out.para(paragraph.trim());
            }
        }
        if !input.tags.is_empty() {
            let tags: Vec<String> = input.tags.iter().map(|t| code(t)).collect();
            out.para(&format!("{} {}", out.bold("Tags:"), tags.join(" ")));
        }
    }

    // Counts
    let c = doc.counts();
    out.h2("Problems");
    let mut parts = Vec::new();
    for (count, singular, plural) in [
        (c.crash, "crash", "crashes"),
        (c.exception, "exception", "exceptions"),
        (c.error, "error", "errors"),
        (c.warning, "warning", "warnings"),
    ] {
        if count > 0 {
            parts.push(format!(
                "{count} {}",
                if count == 1 { singular } else { plural }
            ));
        }
    }
    if parts.is_empty() {
        out.para("No warnings or errors.");
    } else {
        let distinct = input.insights.problems.len();
        out.para(&format!(
            "{} ({distinct} distinct {})",
            parts.join(", "),
            if distinct == 1 { "problem" } else { "problems" }
        ));
    }

    // Top problems, worst first
    let shown = input.insights.problems.iter().take(input.max_problems);
    for (i, problem) in shown.enumerate() {
        let Some(entry) = doc.entry(problem.entry) else {
            continue;
        };
        let times = if problem.count > 1 {
            format!(" ×{}", problem.count)
        } else {
            String::new()
        };
        let at = problem
            .location
            .as_ref()
            .map(|l| format!(" at {}", code(l)))
            .unwrap_or_default();
        out.h3(&format!(
            "{}. {} {}{times}{at}",
            i + 1,
            severity_icon(problem.severity),
            code(&problem.headline)
        ));
        problem_details(&mut out, doc, entry);
    }
    let hidden = input
        .insights
        .problems
        .len()
        .saturating_sub(input.max_problems);
    if hidden > 0 {
        out.para(&format!(
            "…and {hidden} more {} (open the log in Comte Logger for all of them).",
            if hidden == 1 { "problem" } else { "problems" }
        ));
    }

    if let Some((a_title, comparison)) = input.comparison {
        comparison_section(&mut out, a_title, comparison);
    }

    // Timings
    if !input.insights.timings.is_empty() {
        out.h2("Timings");
        let rows = input
            .insights
            .timings
            .iter()
            .map(|t| {
                vec![
                    t.kind.label().to_owned(),
                    t.count.to_string(),
                    format_duration(t.total),
                    format_duration(t.average()),
                    format_duration(t.max),
                ]
            })
            .collect();
        out.table(&["Operation", "Count", "Total", "Average", "Slowest"], rows);
    }

    // Environment
    let mut seen = Vec::new();
    let facts: Vec<Vec<String>> = doc
        .facts()
        .iter()
        .filter(|f| {
            let first = !seen.contains(&f.key);
            seen.push(f.key.clone());
            first
        })
        .map(|f| vec![f.key.label().to_owned(), f.value.clone()])
        .collect();
    if !facts.is_empty() {
        out.h2("Environment");
        out.table(&["Fact", "Value"], facts);
    }

    // Timeline
    let roots: Vec<_> = doc.root_sections().collect();
    if !roots.is_empty() {
        out.h2("Timeline");
        for section in roots {
            let mut line = section.title.clone();
            if let Some(d) = section.duration {
                line.push_str(&format!(" ({})", format_duration(d)));
            }
            let (errors, warnings) = (section.counts.errors(), section.counts.warning);
            if errors + warnings > 0 {
                line.push_str(&format!(
                    ": {errors} {}, {warnings} {}",
                    if errors == 1 { "error" } else { "errors" },
                    if warnings == 1 { "warning" } else { "warnings" }
                ));
            }
            out.bullet(&line);
        }
    }

    out.para(&format!(
        "_Generated by Comte Logger {} from lines 1–{}._",
        env!("CARGO_PKG_VERSION"),
        doc.line_count()
    ));
    out.finish()
}

fn problem_details(out: &mut Out, doc: &LogDocument, entry: &LogEntry) {
    let mut facts = Vec::new();
    let path: Vec<String> = section_titles(doc, entry.section_id);
    if !path.is_empty() {
        facts.push(format!("During {}", path.join(" › ")));
    }
    facts.push(format!("first at line {}", entry.line_range.start));
    out.para(&facts.join(" · "));

    let mut block = vec![entry.message.clone()];
    // A native crash's details are symbol-loader chatter with local paths.
    if entry.severity != Severity::Crash {
        block.extend(entry.details.iter().take(6).cloned());
    }
    block.extend(trimmed_stack(entry));
    out.code_block(&block);
}

/// User frames (with their location) plus short summaries of the engine and
/// package frames around them, at most about [`MAX_FRAMES`] lines.
fn trimmed_stack(entry: &LogEntry) -> Vec<String> {
    let summary = |n: usize| {
        format!(
            "  … {n} engine/package {}",
            if n == 1 { "frame" } else { "frames" }
        )
    };
    let mut lines = Vec::new();
    let mut skipped = 0;
    for (i, frame) in entry.stack.iter().enumerate() {
        if lines.len() >= MAX_FRAMES {
            let remaining = entry.stack.len() - i + skipped;
            lines.push(format!("  … {remaining} more frames"));
            return lines;
        }
        if frame.kind == FrameKind::User {
            if skipped > 0 {
                lines.push(summary(skipped));
                skipped = 0;
            }
            let at = frame
                .location
                .as_ref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            lines.push(format!("  at {}{at}", frame.text));
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        lines.push(summary(skipped));
    }
    lines
}

fn section_titles(doc: &LogDocument, mut section: Option<u32>) -> Vec<String> {
    let mut titles = Vec::new();
    while let Some(id) = section {
        let Some(s) = doc.section(id) else { break };
        titles.push(s.title.clone());
        section = s.parent;
    }
    titles.reverse();
    titles
}

fn comparison_section(out: &mut Out, a_title: &str, c: &Comparison) {
    out.h2(&format!("Changes since {a_title}"));
    out.para(&format!(
        "Errors {} → {}, warnings {} → {}.",
        c.counts_a.errors(),
        c.counts_b.errors(),
        c.counts_a.warning,
        c.counts_b.warning
    ));
    let line = |d: &ProblemDiff, counts: String| {
        let at = d
            .location
            .as_ref()
            .map(|l| format!(" at {}", code(l)))
            .unwrap_or_default();
        format!(
            "{} {}{at} {counts}",
            severity_icon(d.severity),
            code(&d.headline)
        )
    };
    if !c.new.is_empty() {
        out.para(&out.bold(&format!("New ({})", c.new.len())));
        for d in &c.new {
            out.bullet(&line(d, format!("×{}", d.count_b)));
        }
    }
    if !c.gone.is_empty() {
        out.para(&out.bold(&format!("Gone ({})", c.gone.len())));
        for d in &c.gone {
            out.bullet(&line(d, format!("(was ×{})", d.count_a)));
        }
    }
    let grew: Vec<&ProblemDiff> = c.common.iter().filter(|d| d.count_b > d.count_a).collect();
    if !grew.is_empty() {
        out.para(&out.bold("More frequent"));
        for d in grew {
            out.bullet(&line(d, format!("×{} → ×{}", d.count_a, d.count_b)));
        }
    }
    let slower: Vec<String> = c
        .timings
        .iter()
        .filter(|t| t.is_regression() || t.is_improvement())
        .filter_map(|t| {
            let (a, b) = (t.a.as_ref()?, t.b.as_ref()?);
            Some(format!(
                "{}: {} → {} ({}){}",
                t.kind.label(),
                format_duration(a.average()),
                format_duration(b.average()),
                format_percent(t.change_percent()?),
                if t.is_regression() {
                    " slower"
                } else {
                    " faster"
                }
            ))
        })
        .collect();
    if !slower.is_empty() {
        out.para(&out.bold("Timing changes"));
        for line in slower {
            out.bullet(&line);
        }
    }
    for fact in &c.facts {
        out.bullet(&format!(
            "{} changed: {} → {}",
            fact.key.label(),
            fact.a.as_deref().unwrap_or("—"),
            fact.b.as_deref().unwrap_or("—")
        ));
    }
}

fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Crash => "💥",
        Severity::Error | Severity::Exception => "⛔",
        Severity::Warning => "⚠️",
        Severity::Info => "ℹ️",
    }
}

/// `text` as an inline code span, safe even if it contains backticks.
fn code(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

/// Builds the report text for one style.
struct Out {
    style: ReportStyle,
    text: String,
}

impl Out {
    fn new(style: ReportStyle) -> Self {
        Self {
            style,
            text: String::new(),
        }
    }

    fn bold(&self, text: &str) -> String {
        match self.style {
            ReportStyle::Markdown => format!("**{text}**"),
            ReportStyle::Slack => format!("*{text}*"),
        }
    }

    fn block(&mut self, text: &str) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(text);
        self.text.push('\n');
    }

    fn heading(&mut self, level: usize, text: &str) {
        let line = match self.style {
            ReportStyle::Markdown => format!("{} {text}", "#".repeat(level)),
            ReportStyle::Slack => self.bold(text),
        };
        self.block(&line);
    }

    fn h1(&mut self, text: &str) {
        self.heading(1, text);
    }

    fn h2(&mut self, text: &str) {
        self.heading(2, text);
    }

    fn h3(&mut self, text: &str) {
        self.heading(3, text);
    }

    fn para(&mut self, text: &str) {
        self.block(text);
    }

    fn quote(&mut self, lines: &[String]) {
        let text: Vec<String> = lines.iter().map(|l| format!("> {l}")).collect();
        self.block(&text.join("\n"));
    }

    fn bullet(&mut self, text: &str) {
        // Consecutive bullets form one list (no blank line between them).
        let item = format!("- {text}\n");
        if self.text.ends_with("\n")
            && self
                .text
                .lines()
                .last()
                .is_some_and(|l| l.starts_with("- "))
        {
            self.text.push_str(&item);
        } else {
            if !self.text.is_empty() {
                self.text.push('\n');
            }
            self.text.push_str(&item);
        }
    }

    fn table(&mut self, header: &[&str], rows: Vec<Vec<String>>) {
        match self.style {
            ReportStyle::Markdown => {
                let cell = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");
                let mut lines = vec![
                    format!(
                        "| {} |",
                        header
                            .iter()
                            .map(|h| cell(h))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ),
                    format!("|{}", "---|".repeat(header.len())),
                ];
                for row in rows {
                    lines.push(format!(
                        "| {} |",
                        row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
                    ));
                }
                self.block(&lines.join("\n"));
            }
            ReportStyle::Slack => {
                let named = header.iter().any(|h| !h.is_empty());
                for row in rows {
                    let line = match row.split_first() {
                        Some((first, rest)) if named => {
                            let values: Vec<String> = header[1..]
                                .iter()
                                .zip(rest)
                                .map(|(h, v)| format!("{} {v}", h.to_lowercase()))
                                .collect();
                            format!("{}: {}", self.bold(first), values.join(", "))
                        }
                        Some((first, rest)) => format!("{}: {}", first, rest.join(" ")),
                        None => continue,
                    };
                    self.bullet(&line);
                }
            }
        }
    }

    fn code_block(&mut self, lines: &[String]) {
        // A line of ``` inside would end the block early; soften it.
        let body: Vec<String> = lines.iter().map(|l| l.replace("```", "'''")).collect();
        self.block(&format!("```\n{}\n```", body.join("\n")));
    }

    fn finish(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SectionKind;
    use crate::model::{Category, SourceRef, StackFrame};
    use crate::parser::{ParseEvent, SectionStart};

    fn sample() -> LogDocument {
        let mut doc = LogDocument::new();
        doc.apply_all([
            ParseEvent::SectionStart(SectionStart {
                kind: SectionKind::PlayMode,
                title: "Play mode #1".to_owned(),
                line: 1,
            }),
            ParseEvent::Entry(
                LogEntry::new(
                    Severity::Exception,
                    Category::Exception,
                    "NullReferenceException: a | b *c*",
                    2..5,
                )
                .with_stack(vec![
                    StackFrame::new("GameObject.SetActive(Boolean)", FrameKind::Engine),
                    StackFrame::new("Gun.Stop()", FrameKind::User)
                        .with_location(SourceRef::new("Assets/Gun.cs").with_line(9)),
                    StackFrame::new("Zenject.Run()", FrameKind::Package),
                    StackFrame::new("Zenject.Run2()", FrameKind::Package),
                ]),
            ),
        ]);
        doc.finish();
        doc
    }

    fn render(style: ReportStyle, note: &str, tags: &[String]) -> String {
        let doc = sample();
        let insights = Insights::of(&doc);
        render_report(
            &ReportInput {
                title: "Editor.log",
                kind_label: "Unity Editor",
                doc: &doc,
                insights: &insights,
                note,
                tags,
                generated_at: "2026-11-09 14:05",
                comparison: None,
                max_problems: 5,
            },
            style,
        )
    }

    #[test]
    fn markdown_report_has_verdict_problem_and_trimmed_stack() {
        let md = render(
            ReportStyle::Markdown,
            "Breaks when the gun is disabled.",
            &["crash".to_owned()],
        );
        assert!(md.starts_with("# Comte Logger report: Editor.log\n"));
        assert!(md.contains("> ⛔ **1 error**"));
        assert!(md.contains("## Note\n\nBreaks when the gun is disabled.\n\n**Tags:** `crash`"));
        assert!(md.contains("### 1. ⛔ `NullReferenceException` at `Gun.cs:9`"));
        assert!(md.contains("During Play mode #1 · first at line 2"));
        assert!(md.contains(
            "```\nNullReferenceException: a | b *c*\n  … 1 engine/package frame\n  at Gun.Stop() (Assets/Gun.cs:9)\n  … 2 engine/package frames\n```"
        ));
        assert!(md.contains("- Play mode #1: 1 error, 0 warnings"));
    }

    #[test]
    fn slack_report_has_no_headings_or_tables() {
        let slack = render(ReportStyle::Slack, "", &[]);
        assert!(slack.starts_with("*Comte Logger report: Editor.log*\n"));
        assert!(
            !slack
                .lines()
                .any(|l| l.starts_with('#') || l.starts_with('|'))
        );
        assert!(!slack.contains("## Note"), "no note section without a note");
        assert!(slack.contains("> ⛔ *1 error*"));
    }

    #[test]
    fn code_spans_survive_backticks_and_tables_escape_pipes() {
        assert_eq!(code("a`b"), "`` a`b ``");
        assert_eq!(code("plain"), "`plain`");
        let mut out = Out::new(ReportStyle::Markdown);
        out.table(&["k", "v"], vec![vec!["a|b".to_owned(), "c".to_owned()]]);
        assert!(out.finish().contains("| a\\|b | c |"));
    }

    #[test]
    fn long_stacks_are_cut_with_a_count() {
        let frames: Vec<StackFrame> = (0..20)
            .map(|i| StackFrame::new(format!("Game.F{i}()"), FrameKind::User))
            .collect();
        let entry = LogEntry::new(Severity::Error, Category::UserLog, "x", 1..2).with_stack(frames);
        let lines = trimmed_stack(&entry);
        assert_eq!(lines.len(), MAX_FRAMES + 1);
        assert_eq!(lines.last().unwrap(), "  … 12 more frames");
    }

    #[test]
    fn bullets_group_into_one_list() {
        let mut out = Out::new(ReportStyle::Markdown);
        out.para("Intro");
        out.bullet("one");
        out.bullet("two");
        out.para("After");
        assert_eq!(out.finish(), "Intro\n\n- one\n- two\n\nAfter\n");
    }
}
