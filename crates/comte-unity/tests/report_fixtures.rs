//! Full reports for the fixtures, in both styles. Review changes with
//! `cargo insta review`.

#[allow(dead_code)] // shared helpers; this test uses only `fixture`
mod common;

use comte_core::{Insights, LogParser, ReportInput, ReportStyle, render_report};
use comte_unity::{EditorParser, PlayerParser};

fn report(name: &str, parser: &mut dyn LogParser, style: ReportStyle) -> String {
    let doc = common::fixture(name, parser);
    let insights = Insights::of(&doc);
    let tags = vec!["crash".to_owned(), "scene 3".to_owned()];
    render_report(
        &ReportInput {
            title: name,
            kind_label: "Unity",
            doc: &doc,
            insights: &insights,
            note: "Happens when the gun is disabled mid-shot.\n\nRepro: start a run, die while shooting.",
            tags: &tags,
            generated_at: "2026-11-09 14:05",
            comparison: None,
            max_problems: 3,
            pins: &[],
        },
        style,
    )
    // The footer names the app version; keep snapshots stable across releases.
    .replace(
        &format!("Comte Log Analyzer {}", env!("CARGO_PKG_VERSION")),
        "Comte Log Analyzer <version>",
    )
}

#[test]
fn player_report_markdown() {
    insta::assert_snapshot!(report(
        "player_session.log",
        &mut PlayerParser::new(),
        ReportStyle::Markdown
    ));
}

#[test]
fn player_report_slack() {
    insta::assert_snapshot!(report(
        "player_session.log",
        &mut PlayerParser::new(),
        ReportStyle::Slack
    ));
}

#[test]
fn editor_report_markdown() {
    insta::assert_snapshot!(report(
        "editor_session.log",
        &mut EditorParser::new(),
        ReportStyle::Markdown
    ));
}
