//! Comparing a fixture with an edited copy of itself: what a developer sees
//! after fixing one error and introducing another between two Editor runs.

use std::io::Cursor;
use std::path::Path;

use comte_core::{Comparison, LogDocument, Severity, parse_reader};
use comte_unity::EditorParser;

fn parse(text: &str) -> LogDocument {
    parse_reader(Cursor::new(text.to_owned()), &mut EditorParser::new()).unwrap()
}

#[test]
fn fixed_and_introduced_problems_between_runs() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/editor_session.log");
    let before = std::fs::read_to_string(path).unwrap();
    // Run B: the CS0103 compile error is fixed (and the build succeeds),
    // the save error moved 4 lines down, and a new exception appears.
    let after = before
        .replace(
            "Assets/Scripts/Flow/GameRunner.cs(61,17): error CS0103: The name 'turnIndex' does not exist in the current context\n*** Tundra build failed (14.60 seconds)",
            "*** Tundra build success (9.10 seconds)",
        )
        .replace("SaveSystem.cs:77)", "SaveSystem.cs:81)")
        .replace("SaveSystem.cs Line: 77)", "SaveSystem.cs Line: 81)")
        .replace(
            "Unloading 3 Unused Serialized files (Serialized files now loaded: 0)\nLoaded scene 'Temp/__Backupscenes/0.backup'\n\tDeserialize:            5.529 ms",
            "IndexOutOfRangeException: Index was outside the bounds of the array.\n  at SampleGame.Week.Planner.Next () [0x00010] in C:/Users/dev/Projects/Sample Game/Assets/Scripts/Week/Planner.cs:33 \n\n(Filename: Assets/Scripts/Week/Planner.cs Line: 33)\n\nUnloading 3 Unused Serialized files (Serialized files now loaded: 0)\nLoaded scene 'Temp/__Backupscenes/0.backup'\n\tDeserialize:            5.529 ms",
        );
    assert_ne!(before, after, "the edits applied");

    let c = Comparison::of(&parse(&before), &parse(&after));
    let headlines = |list: &[comte_core::ProblemDiff]| {
        list.iter().map(|d| d.headline.clone()).collect::<Vec<_>>()
    };
    assert_eq!(headlines(&c.new), ["IndexOutOfRangeException"]);
    assert_eq!(c.new[0].location.as_deref(), Some("Planner.cs:33"));
    assert_eq!(
        headlines(&c.gone),
        [
            "CS0103: The name 'turnIndex' does not exist in the current context",
            "Script compilation failed after 14.60 s"
        ]
    );
    assert!(
        c.common
            .iter()
            .any(|d| d.headline.starts_with("Save failed")
                && d.location.as_deref() == Some("SaveSystem.cs:81")),
        "the save error moved lines but is the same problem"
    );
    assert!(c.common.iter().all(|d| d.severity >= Severity::Warning));
    assert!(c.facts.is_empty());
    assert_eq!(c.counts_b.exception, c.counts_a.exception + 1);
}
