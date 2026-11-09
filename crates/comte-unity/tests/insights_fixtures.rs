//! Insights (verdict, repeats, timings) on the Unity fixtures.

#[allow(dead_code)] // shared helpers; this test uses only `fixture`
mod common;

use std::fmt::Write as _;

use comte_core::{Insights, LogDocument, format_duration};
use comte_unity::{EditorParser, PlayerParser};

fn render(doc: &LogDocument) -> String {
    let insights = Insights::of(doc);
    let mut out = String::new();
    let v = &insights.verdict;
    writeln!(out, "verdict: {:?} | {}", v.kind, v.headline).unwrap();
    for detail in &v.details {
        writeln!(out, "  - {detail}").unwrap();
    }
    let at = |id: Option<u64>| {
        id.map_or("-".to_owned(), |id| {
            format!("L{}", doc.entry(id).unwrap().line_range.start)
        })
    };
    writeln!(out, "  -> {}", at(v.entry)).unwrap();
    writeln!(out, "repeated problems:").unwrap();
    for r in &insights.repeated_problems {
        writeln!(
            out,
            "  ×{} {:?} {} @ {:?} ({})",
            r.count,
            r.severity,
            r.headline,
            r.location,
            at(Some(r.entry))
        )
        .unwrap();
    }
    writeln!(out, "repeated messages:").unwrap();
    for r in &insights.repeated_messages {
        writeln!(out, "  ×{} {} ({})", r.count, r.headline, at(Some(r.entry))).unwrap();
    }
    writeln!(out, "slowest:").unwrap();
    for t in &insights.slowest {
        writeln!(
            out,
            "  {} {} (metric L{}, entry {})",
            t.kind.label(),
            format_duration(t.duration),
            t.line,
            at(t.entry)
        )
        .unwrap();
    }
    writeln!(out, "timings:").unwrap();
    for t in &insights.timings {
        writeln!(
            out,
            "  {}: {}× total {} avg {} max {}",
            t.kind.label(),
            t.count,
            format_duration(t.total),
            format_duration(t.average()),
            format_duration(t.max)
        )
        .unwrap();
    }
    out
}

#[test]
fn editor_insights() {
    let doc = common::fixture("editor_session.log", &mut EditorParser::new());
    insta::assert_snapshot!(render(&doc));
}

#[test]
fn player_insights() {
    let doc = common::fixture("player_session.log", &mut PlayerParser::new());
    insta::assert_snapshot!(render(&doc));
}
