//! Snapshot and rule tests for the Player.log parser (DESIGN §4.2).
//!
//! `player_session.log` is sanitized from real Unity 2019.4 Player logs. Its
//! native crash block is reconstructed from Unity's Windows crash format,
//! because none of the sample logs this was built from contained one.

mod common;

use std::io::Cursor;

use comte_core::{
    Category, FactKey, FrameKind, LogDocument, MetricKind, SectionKind, Severity, parse_reader,
};
use comte_unity::PlayerParser;

fn fixture(name: &str) -> LogDocument {
    common::fixture(name, &mut PlayerParser::new())
}

fn parse(text: &str) -> LogDocument {
    parse_reader(Cursor::new(text.to_owned()), &mut PlayerParser::new()).unwrap()
}

#[test]
fn player_session_outline() {
    let doc = fixture("player_session.log");
    insta::assert_snapshot!(common::outline(&doc));
}

#[test]
fn player_session_key_facts() {
    let doc = fixture("player_session.log");

    assert_eq!(doc.fact(&FactKey::EngineVersion), Some("2019.4.40f1"));
    assert_eq!(doc.fact(&FactKey::GamePath), Some("C:/Games/Sample Game"));
    assert_eq!(
        doc.fact(&FactKey::Gpu),
        Some("NVIDIA GeForce RTX 2070 Super")
    );
    assert_eq!(
        doc.fact(&FactKey::CrashReport),
        Some("C:/Users/player/AppData/Local/Temp/Sample Studio/Sample Game/Crashes")
    );

    let titles: Vec<_> = doc
        .sections()
        .iter()
        .map(|s| (s.title.as_str(), s.depth))
        .collect();
    assert_eq!(
        titles,
        [
            ("Startup", 0),
            ("Domain reload", 1),
            ("Scene #1", 0),
            ("Scene #2", 0),
            ("Scene #3", 0),
            ("Crash", 1),
        ],
        "the crash happened inside scene #3"
    );

    let c = doc.counts();
    assert_eq!(c.crash, 1);
    assert_eq!(c.exception, 3, "Steam init, ArgumentNull, NullReference");
    assert_eq!(c.warning, 1);
}

#[test]
fn release_debug_log_without_stack_is_a_user_log() {
    let doc = parse(
        "SAVING TO DISC! \n(Filename: C:\\buildslave\\unity\\build\\Runtime/Export/Debug/Debug.bindings.h Line: 39)\n\n",
    );
    let e = &doc.entries()[0];
    assert_eq!(e.category, Category::UserLog);
    assert_eq!(e.severity, Severity::Info, "no stack trace, so no severity");
    assert_eq!(e.message, "SAVING TO DISC!");
}

#[test]
fn engine_message_with_filename_stays_engine() {
    let doc = parse("Curl error 42: Callback aborted\n \n(Filename:  Line: 867)\n");
    assert_eq!(doc.entries().len(), 1);
    assert_eq!(doc.entries()[0].category, Category::Other);
    assert_eq!(doc.entries()[0].line_range, 1..4);
}

#[test]
fn blank_line_still_ends_a_plain_entry_without_filename() {
    let doc = parse("first\n\n  indented after blank\n");
    assert_eq!(
        doc.entries().len(),
        2,
        "indented line after a blank is not a continuation"
    );
}

#[test]
fn debug_bindings_path_is_not_a_culprit() {
    let doc = parse(
        "SAVING TO DISC! \n(Filename: C:\\buildslave\\unity\\build\\Runtime/Export/Debug/Debug.bindings.h Line: 39)\n",
    );
    assert!(doc.entries()[0].culprit().is_none());
}

#[test]
fn log_exception_does_not_repeat_the_top_frame() {
    let doc = fixture("player_session.log");
    let e = doc
        .entries()
        .iter()
        .find(|e| e.message.starts_with("ArgumentNullException"))
        .unwrap();
    let saves = e
        .stack
        .iter()
        .filter(|f| f.text == "SavefileHelper.SaveLoginSession(LoginSession)")
        .count();
    assert_eq!(saves, 1);
}

#[test]
fn uploading_crash_report_is_not_a_crash() {
    let doc = parse(
        "Uploading Crash Report\nNullReferenceException: boom\n  at A.B () [0x0] in <abc>:0 \n",
    );
    assert_eq!(doc.counts().crash, 0);
    assert_eq!(
        doc.entries()[0].message,
        "Exception reported to Unity Cloud Diagnostics"
    );
    assert_eq!(doc.entries()[1].severity, Severity::Exception);
}

#[test]
fn release_frames_without_paths_are_classified_by_namespace() {
    let doc = fixture("player_session.log");
    let e = doc
        .entries()
        .iter()
        .find(|e| e.message.starts_with("ArgumentNullException"))
        .unwrap();
    let frames: Vec<_> = e.stack.iter().map(|f| (f.text.as_str(), f.kind)).collect();
    assert_eq!(frames[0], ("BinaryWriter.Write(String)", FrameKind::Engine));
    assert_eq!(
        frames[1],
        (
            "BinaryExtensions.Write(BinaryWriter, LoginSession)",
            FrameKind::User
        )
    );
    assert!(frames.contains(&("RefreshSession(Exception) [lambda]", FrameKind::User)));
    assert!(frames.contains(&("CreateRequestAndRetry() [async]", FrameKind::User)));
    assert!(
        !frames.iter().any(|(t, _)| t.contains("DebugLogHandler")),
        "LogException plumbing dropped"
    );
}

#[test]
fn windows_native_crash_frames() {
    let doc = fixture("player_session.log");
    let crash = doc
        .entries()
        .iter()
        .find(|e| e.severity == Severity::Crash)
        .unwrap();
    assert_eq!(crash.message, "Native crash");
    assert_eq!(crash.stack.len(), 5);
    assert_eq!(
        crash.stack[0].kind,
        FrameKind::User,
        "IL2CPP GameAssembly frame"
    );
    assert_eq!(
        crash.stack[1].text,
        "ScriptingInvocation::Invoke (UnityPlayer)"
    );
    assert_eq!(
        crash.stack[1].location.as_ref().unwrap().short(),
        "ScriptingInvocation.cpp:281"
    );
    assert_eq!(crash.stack[4].kind, FrameKind::Native);
}

#[test]
fn posix_signal_crash_without_end_marker() {
    let doc = parse(
        "UnloadTime: 1.0 ms
Received signal SIGSEGV
Obtained 3 stack frames.
#0  0x007f1a2b3c4d5e in SampleGame_Enemy_Update
#1  0x007f1a2b3c0000 in mono_runtime_invoke
#2  0x007f1a2b3c1111 in (Unknown)
",
    );
    let crash = doc
        .entries()
        .iter()
        .find(|e| e.severity == Severity::Crash)
        .unwrap();
    assert_eq!(crash.message, "Native crash: Received signal SIGSEGV");
    assert_eq!(crash.stack.len(), 3);
    assert_eq!(crash.stack[0].text, "SampleGame_Enemy_Update");
    assert_eq!(crash.details, ["Obtained 3 stack frames."]);
    let section = doc.section(crash.section_id.unwrap()).unwrap();
    assert_eq!(section.kind, SectionKind::Crash);
}

#[test]
fn scene_sections_and_metrics() {
    let doc = parse(
        "Begin MonoManager ReloadAssembly
- Completed reload, in  2.500 seconds
UnloadTime: 9.5 ms
first scene log
UnloadTime: 120.0 ms
second scene log
",
    );
    let kinds: Vec<_> = doc.metrics().iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            MetricKind::Startup,
            MetricKind::SceneUnload,
            MetricKind::SceneUnload
        ]
    );
    let scene2 = doc
        .sections()
        .iter()
        .find(|s| s.title == "Scene #2")
        .unwrap();
    let last = doc.entries().last().unwrap();
    assert_eq!(last.message, "second scene log");
    assert_eq!(last.section_id, Some(scene2.id));
    let startup = &doc.sections()[0];
    assert_eq!(startup.kind, SectionKind::Startup);
    assert_eq!(startup.line_range, 1..3);
}
