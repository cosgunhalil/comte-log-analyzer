//! Shared helpers for fixture snapshot tests.

use std::fmt::Write as _;
use std::path::Path;

use comte_core::{
    Category, FrameKind, LogDocument, LogEntry, LogParser, MetricValue, Severity, format_duration,
    parse_file,
};

pub fn fixture(name: &str, parser: &mut dyn LogParser) -> LogDocument {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    parse_file(&path, parser).expect("fixture readable")
}

/// Renders a document the way the UI will roughly show it.
pub fn outline(doc: &LogDocument) -> String {
    let mut out = String::new();
    writeln!(out, "== facts").unwrap();
    for fact in doc.facts() {
        writeln!(
            out,
            "  {}: {}  (L{})",
            fact.key.label(),
            fact.value,
            fact.line
        )
        .unwrap();
    }
    writeln!(out, "== metrics").unwrap();
    for metric in doc.metrics() {
        let value = match metric.value {
            MetricValue::Duration(d) => format_duration(d),
            MetricValue::Count(c) => c.to_string(),
        };
        writeln!(
            out,
            "  {}: {value}  (L{})",
            metric.kind.label(),
            metric.line
        )
        .unwrap();
    }
    let c = doc.counts();
    writeln!(
        out,
        "== timeline  ({} lines, {} entries, {} errors, {} warnings)",
        doc.line_count(),
        doc.entries().len(),
        c.errors(),
        c.warning
    )
    .unwrap();
    render_level(doc, None, 0, &mut out);
    out
}

fn render_level(doc: &LogDocument, section: Option<u32>, depth: usize, out: &mut String) {
    enum Item<'a> {
        Section(&'a comte_core::Section),
        Entry(&'a LogEntry),
    }
    let mut items: Vec<(usize, Item)> = doc
        .entries_in(section)
        .map(|e| (e.line_range.start, Item::Entry(e)))
        .collect();
    let children: Vec<_> = match section {
        Some(id) => doc.child_sections(id).collect(),
        None => doc.root_sections().collect(),
    };
    items.extend(
        children
            .into_iter()
            .map(|s| (s.line_range.start, Item::Section(s))),
    );
    items.sort_by_key(|(line, _)| *line);

    let pad = "  ".repeat(depth);
    for (_, item) in items {
        match item {
            Item::Section(s) => {
                let duration = s
                    .duration
                    .map(|d| format!(" {}", format_duration(d)))
                    .unwrap_or_default();
                writeln!(
                    out,
                    "{pad}▾ {} [L{}-{}]{duration}  errors={} warnings={}",
                    s.title,
                    s.line_range.start,
                    s.line_range.end - 1,
                    s.counts.errors(),
                    s.counts.warning
                )
                .unwrap();
                render_level(doc, Some(s.id), depth + 1, out);
            }
            Item::Entry(e) => render_entry(e, &pad, out),
        }
    }
}

fn render_entry(e: &LogEntry, pad: &str, out: &mut String) {
    let icon = match e.severity {
        Severity::Info => "·",
        Severity::Warning => "⚠",
        Severity::Error => "⛔",
        Severity::Exception => "⛔",
        Severity::Crash => "💥",
    };
    let noise = if e.category.is_noise() {
        " (noise)"
    } else {
        ""
    };
    let culprit = e
        .culprit()
        .map(|c| format!("  @ {}", c.short()))
        .unwrap_or_default();
    let more = if e.details.is_empty() {
        String::new()
    } else {
        format!("  +{} lines", e.details.len())
    };
    writeln!(
        out,
        "{pad}{icon} [{}] L{}-{} {}{culprit}{more}{noise}",
        e.category.label(),
        e.line_range.start,
        e.line_range.end - 1,
        truncate(&e.message, 90),
    )
    .unwrap();
    if e.severity >= Severity::Warning || e.category == Category::UserLog {
        for detail in e
            .details
            .iter()
            .filter(|d| e.category == Category::Exception || d.starts_with("Rethrow"))
        {
            writeln!(out, "{pad}    | {}", truncate(detail, 90)).unwrap();
        }
        for frame in &e.stack {
            let kind = match frame.kind {
                FrameKind::User => "USER",
                FrameKind::Package => "pkg ",
                FrameKind::Engine => "eng ",
                FrameKind::Native => "nat ",
            };
            let at = frame
                .location
                .as_ref()
                .map(|l| format!("  ({l})"))
                .unwrap_or_default();
            writeln!(out, "{pad}    {kind} {}{at}", frame.text).unwrap();
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}
