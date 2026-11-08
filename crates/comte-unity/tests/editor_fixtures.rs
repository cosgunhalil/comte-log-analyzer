//! Snapshot tests: parse sanitized Editor.log fixtures and render the result
//! as a readable outline. Review snapshot changes with `cargo insta review`.

mod common;

use comte_core::LogDocument;
use comte_unity::EditorParser;

fn fixture(name: &str) -> LogDocument {
    common::fixture(name, &mut EditorParser::new())
}

#[test]
fn editor_session_outline() {
    let doc = fixture("editor_session.log");
    insta::assert_snapshot!(common::outline(&doc));
}

#[test]
fn editor_session_key_facts() {
    let doc = fixture("editor_session.log");
    let c = doc.counts();
    assert_eq!(c.exception, 2, "token exchange + missing reference");
    assert_eq!(c.error, 3, "LogError + CS0103 + failed compile");
    assert_eq!(c.warning, 2, "LogWarning + CS0618");

    let titles: Vec<_> = doc.sections().iter().map(|s| s.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Startup",
            "Domain reload",
            "Script compilation",
            "Domain reload",
            "Play mode #1",
            "Domain reload",
            "Script compilation",
            "Shutdown"
        ]
    );
    assert!(doc.sections().iter().all(|s| s.closed));
}
