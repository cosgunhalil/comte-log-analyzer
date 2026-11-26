//! Player builds in an Editor log, read from `fixtures/editor_build.log`.
//!
//! That fixture is made of real Unity 6000.3.11f1 lines: the sanitized Editor
//! header, then an incremental Android build from a batch-mode build log
//! (no sizes) and a clean Android build from an interactive Editor.log. Asset
//! and package names are replaced with neutral ones, keeping sizes and
//! percentages; the clean build's asset list (1,575 assets) is trimmed to 50.

use std::path::Path;
use std::time::Duration;

use comte_core::{BuildReport, LogDocument, LogKind, TailDocument, parse_reader, parse_size};
use comte_unity::{EditorParser, detect_kind};

fn fixture_text() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/editor_build.log"),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

fn parse(text: &str) -> LogDocument {
    parse_reader(
        std::io::Cursor::new(text.to_owned()),
        &mut EditorParser::new(),
    )
    .unwrap()
}

fn builds() -> Vec<BuildReport> {
    parse(&fixture_text()).build_reports().to_vec()
}

#[test]
fn the_fixture_is_an_editor_log_with_two_builds() {
    let text = fixture_text();
    let head: Vec<&str> = text.lines().take(64).collect();
    assert_eq!(detect_kind(&head), Some(LogKind::Editor));
    assert_eq!(builds().len(), 2);
}

#[test]
fn an_incremental_build_has_no_sizes_but_still_says_what_it_was() {
    let build = &builds()[0];
    assert_eq!(build.target.as_deref(), Some("Android"));
    assert_eq!(
        build.result.as_deref(),
        Some("Success"),
        "from 'Build Finished, Result: Success.'"
    );
    assert!(build.sizes_unavailable);
    assert!(!build.has_sizes());
    assert!(build.categories.is_empty() && build.assets.is_empty());
    // The step timings come from the ##utp PlayerBuildInfo line.
    assert!(build.steps.len() > 5, "{:?}", build.steps);
    assert_eq!(build.steps[0].name, "Preprocess Player");
    assert!(build.duration.is_some(), "the PlayerBuildInfo total");
    assert!(
        build.started_at.is_none(),
        "batch logs don't print the times"
    );
    assert!(build.started_line.is_some() && build.line > build.started_line.unwrap());
}

#[test]
fn a_clean_build_lists_every_size_and_its_identity() {
    let build = &builds()[1];
    assert_eq!(build.target.as_deref(), Some("Android"));
    assert_eq!(build.result.as_deref(), Some("Succeeded"));
    assert_eq!(build.duration, Some(Duration::from_millis(554_039)));
    assert_eq!(build.started_at.as_deref(), Some("11/24/2026 11:24:23 PM"));
    assert_eq!(build.finished_at.as_deref(), Some("11/24/2026 11:33:37 PM"));
    assert_eq!(
        build.label(),
        "Android build · 11/24/2026 11:24:23 PM · Succeeded · 9 min 14 s"
    );
    assert!(!build.sizes_unavailable);

    assert_eq!(build.total, parse_size("1.2 gb"));
    assert_eq!(build.user_assets, parse_size("590.2 mb"));
    let categories: Vec<&str> = build.categories.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        categories,
        [
            "Textures",
            "Meshes",
            "Animations",
            "Sounds",
            "Shaders",
            "Other Assets",
            "Levels",
            "File headers"
        ],
        "Unity 6 lists no Scripts or Included DLLs rows; Total User Assets is the total"
    );
    assert_eq!(build.categories[0].bytes, parse_size("575.0 mb").unwrap());

    assert_eq!(build.assets.len(), 50, "every listed asset is kept");
    assert_eq!(build.assets[0].name, "Assets/Game/Textures/item_0001.png");
    assert_eq!(build.assets[0].bytes, parse_size("20.0 mb").unwrap());
    assert!(build.assets.iter().any(|a| a.name.starts_with("Packages/")));
    assert_eq!(
        build.assets.last().unwrap().name,
        "Assets/Game/Resources/Data/item_0050.json"
    );
    assert!(
        build
            .steps
            .iter()
            .any(|s| s.name == "Postprocess built player")
    );
}

#[test]
fn each_report_is_one_entry_with_a_summary() {
    let doc = parse(&fixture_text());
    let messages: Vec<&str> = doc
        .entries()
        .iter()
        .filter(|e| e.message.starts_with("Build report"))
        .map(|e| e.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "Build report: no sizes (Unity lists them only after a clean build)",
            "Build report: 1.20 GB complete build, largest asset Assets/Game/Textures/item_0001.png (20.0 MB)",
        ]
    );
    let clean = doc.build_reports()[1].line;
    let entry = doc.entry_starting_at(clean).expect("the report entry");
    assert!(
        entry.line_range.len() > 60,
        "categories and assets fold into it"
    );
}

#[test]
fn a_log_that_ends_during_a_build_still_shows_it() {
    let text = fixture_text();
    let cut = text.find("DisplayProgressNotification: Build Successful\nBuild Finished, Result: Success.\n##utp:{\"type\":\"PlayerBuildInfo\",\"version\":2,\"phase\":\"Immediate\",\"time\":1795552417701").unwrap();
    let doc = parse(&text[..cut]);
    let build = doc.build_reports().last().unwrap();
    assert!(build.result.is_none());
    assert!(build.label().ends_with("not finished"), "{}", build.label());
    assert_eq!(build.assets.len(), 50, "the sizes were already read");
}

#[test]
fn live_tail_reads_builds_the_same_way() {
    let text = fixture_text();
    let mut tail = TailDocument::new(Box::new(EditorParser::new()));
    for chunk in text.as_bytes().chunks(997) {
        tail.push_bytes(chunk);
    }
    assert_eq!(
        tail.snapshot().build_reports(),
        parse(&text).build_reports()
    );
}

/// The fixture's clean build after a change to the project: one texture got
/// bigger, one smaller, a new one was added and a data file removed.
fn next_build_text() -> String {
    fixture_text()
        .replace("Textures               575.0 mb", "Textures               608.0 mb")
        .replace("Total User Assets      590.2 mb", "Total User Assets      623.2 mb")
        .replace(
            " 20.0 mb\t 1.6% Assets/Game/Textures/item_0001.png",
            " 48.0 mb\t 3.8% Assets/Game/Textures/item_0001.png",
        )
        .replace(
            " 16.0 mb\t 1.3% Assets/Game/Textures/item_0002.png",
            " 8.0 mb\t 0.6% Assets/Game/Textures/item_0002.png\n 13.0 mb\t 1.0% Assets/Game/Textures/item_0051.png",
        )
        .replace(" 0.1 kb\t 0.0% Assets/Game/Resources/Data/item_0050.json\n", "")
}

#[test]
fn two_clean_builds_compare_by_asset() {
    let before = builds()[1].clone();
    let after = parse(&next_build_text()).build_reports()[1].clone();
    assert_eq!(after.assets.len(), 50, "one added, one removed");

    let c = comte_core::BuildComparison::of(&before, &after);
    assert!(c.comparable());
    assert_eq!(c.headline(), "Your build grew by 33.0 MB");
    let responsible: Vec<(&str, String)> = c
        .responsible()
        .iter()
        .map(|r| (r.name.as_str(), comte_core::format_delta(r.delta())))
        .collect();
    assert_eq!(
        responsible,
        [
            ("Assets/Game/Textures/item_0001.png", "+28.0 MB".to_owned()),
            ("Assets/Game/Textures/item_0051.png", "+13.0 MB".to_owned()),
        ]
    );
    assert_eq!(c.shrank[0].name, "Assets/Game/Textures/item_0002.png");
    assert_eq!(
        c.removed[0].name,
        "Assets/Game/Resources/Data/item_0050.json"
    );
    assert_eq!(c.unchanged, 47);
    assert_eq!(c.categories[0].name, "Textures");
    assert_eq!(
        comte_core::format_delta(c.categories[0].delta()),
        "+33.0 MB"
    );

    // Compared the other way round, it shrank because of the same assets.
    let back = comte_core::BuildComparison::of(&after, &before);
    assert_eq!(back.headline(), "Your build shrank by 33.0 MB");
    assert_eq!(
        back.responsible()[0].name,
        "Assets/Game/Textures/item_0001.png"
    );
}
