//! Building the pasteable report (DESIGN §6.3) for the open log, with the
//! same redaction as saved sessions.

use comte_core::{
    Comparison, FactKey, Pin, RedactionReport, Redactor, ReportInput, ReportStyle, parse_tags,
    render_report,
};

use crate::loader::Loaded;

/// Choices in the report dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOptions {
    pub style: ReportStyle,
    pub note: String,
    /// Comma-separated.
    pub tags: String,
    pub redact: bool,
    pub include_comparison: bool,
    pub max_problems: usize,
    /// Pins of the open log, listed under "Pinned entries".
    pub pins: Vec<Pin>,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            style: ReportStyle::Markdown,
            note: String::new(),
            tags: String::new(),
            redact: true,
            include_comparison: true,
            max_problems: 5,
            pins: Vec::new(),
        }
    }
}

/// The report text and, when redacted, what was replaced.
#[derive(Debug, Clone)]
pub struct BuiltReport {
    pub text: String,
    pub redaction: Option<RedactionReport>,
}

pub fn build_report(
    loaded: &Loaded,
    comparison: Option<(&str, &Comparison)>,
    options: &ReportOptions,
    generated_at: &str,
    user_names: &[&str],
) -> BuiltReport {
    let tags = parse_tags(&options.tags);
    let text = render_report(
        &ReportInput {
            title: &loaded.title,
            kind_label: &loaded.kind_label,
            doc: &loaded.doc,
            insights: &loaded.insights,
            note: &options.note,
            tags: &tags,
            generated_at,
            comparison: comparison.filter(|_| options.include_comparison),
            max_problems: options.max_problems,
            pins: &options.pins,
        },
        options.style,
    );
    if !options.redact {
        return BuiltReport {
            text,
            redaction: None,
        };
    }
    // Learn user names from the whole log, then mask the report line by line.
    let mut redactor = Redactor::for_lines(&loaded.raw, user_names);
    let mut redacted: Vec<String> = text
        .lines()
        .map(|line| redactor.redact_line(line).into_owned())
        .collect();
    if text.ends_with('\n') {
        redacted.push(String::new());
    }
    BuiltReport {
        text: redacted.join("\n"),
        redaction: Some(redactor.report().clone()),
    }
}

/// A short plain-text summary for a bug report: what the log is, its
/// verdict, problem counts and the environment (Unity version, OS, GPU, …).
/// Redacted the same way as reports, since it's meant to be pasted elsewhere.
pub fn system_summary(loaded: &Loaded, user_names: &[&str]) -> String {
    let lines = summary_lines(loaded);
    let mut redactor = Redactor::for_lines(&loaded.raw, user_names);
    lines
        .iter()
        .map(|line| redactor.redact_line(line).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The summary's lines, not redacted.
pub fn summary_lines(loaded: &Loaded) -> Vec<String> {
    let doc = &loaded.doc;
    let verdict = &loaded.insights.verdict;
    let mut lines = vec![format!("{} ({})", loaded.title, loaded.kind_label)];
    let mut verdict_line = format!("Verdict: {}", verdict.headline);
    if let Some(detail) = verdict.details.first() {
        verdict_line.push_str(&format!(". {detail}"));
    }
    lines.push(verdict_line);
    let c = doc.counts();
    let mut problems = Vec::new();
    for (count, singular, plural) in [
        (c.crash, "crash", "crashes"),
        (c.exception, "exception", "exceptions"),
        (c.error, "error", "errors"),
        (c.warning, "warning", "warnings"),
    ] {
        if count > 0 {
            problems.push(format!(
                "{count} {}",
                if count == 1 { singular } else { plural }
            ));
        }
    }
    lines.push(format!(
        "Problems: {}",
        if problems.is_empty() {
            "none".to_owned()
        } else {
            problems.join(", ")
        }
    ));
    let mut seen = Vec::new();
    for fact in doc.facts() {
        if fact.key == FactKey::Mod {
            continue; // listed together below
        }
        if !seen.contains(&fact.key) {
            seen.push(fact.key.clone());
            lines.push(format!("{}: {}", fact.key.label(), fact.value));
        }
    }
    let mods = mod_names(doc);
    if !mods.is_empty() {
        lines.push(format!("Mods ({}): {}", mods.len(), mods.join(", ")));
    }
    lines.push(format!("Lines: {}", doc.line_count()));
    lines
}

/// The mods a mod loader loaded, in load order, without repeats.
pub fn mod_names(doc: &comte_core::LogDocument) -> Vec<&str> {
    let mut mods: Vec<&str> = Vec::new();
    for fact in doc.facts().iter().filter(|f| f.key == FactKey::Mod) {
        if !mods.contains(&fact.value.as_str()) {
            mods.push(&fact.value);
        }
    }
    mods
}

/// The Markdown comparison of two builds, redacted with the names learned
/// from both logs (the earlier build's log may be a saved build).
pub fn build_comparison_markdown(
    before: (&str, &comte_core::BuildReport, &[String]),
    after: (&str, &comte_core::BuildReport, &[String]),
    user_names: &[&str],
    redact: bool,
) -> String {
    let text =
        comte_core::BuildComparison::of(before.1, after.1).to_markdown(before.0, after.0, 20);
    if !redact {
        return text;
    }
    let lines: Vec<&str> = before.2.iter().chain(after.2).map(String::as_str).collect();
    let mut redactor = Redactor::for_lines(&lines, user_names);
    let mut out: Vec<String> = text
        .lines()
        .map(|line| redactor.redact_line(line).into_owned())
        .collect();
    if text.ends_with('\n') {
        out.push(String::new());
    }
    out.join("\n")
}

/// `2026-11-09_14-05_Editor.md`
pub fn report_file_name(title: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    crate::save::default_file_name(title, now).replace(".log", ".md")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::loader::load;

    fn fixture(name: &str) -> Loaded {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../comte-unity/tests/fixtures")
            .join(name);
        load(&path, name).unwrap()
    }

    /// The build fixture, with one asset given an absolute path that names a
    /// user (real build reports list project paths, but nothing stops an
    /// asset path from carrying a user folder).
    fn build_log_with_a_user_path() -> Loaded {
        // Tests run in parallel: each call gets its own file.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../comte-unity/tests/fixtures/editor_build.log"),
        )
        .unwrap()
        .replace(
            "Assets/Game/Textures/item_0001.png",
            "C:/Users/jdoe.smith/Art/item_0001.png",
        );
        let path = std::env::temp_dir().join(format!(
            "comte-build-redact-{}-{}.log",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, text).unwrap();
        let loaded = load(&path, "Editor.log").unwrap();
        let _ = std::fs::remove_file(&path);
        loaded
    }

    #[test]
    fn build_sizes_in_reports_are_redacted() {
        let loaded = build_log_with_a_user_path();
        let build = loaded.doc.build_reports().last().unwrap();
        assert_eq!(
            build.assets[0].name,
            "C:/Users/jdoe.smith/Art/item_0001.png"
        );

        let report = build_report(&loaded, None, &ReportOptions::default(), "now", &[]);
        assert!(report.text.contains("## Build size"), "{}", report.text);
        assert!(
            report
                .text
                .contains("**Android build · 11/24/2026 11:24:23 PM · Succeeded · 9 min 14 s** (the latest of 2 builds in this log)"),
            "{}",
            report.text
        );
        assert!(report.text.contains("C:/Users/<user>/Art/item_0001.png"));
        assert!(!report.text.contains("jdoe"), "{}", report.text);

        let plain = build_report(
            &loaded,
            None,
            &ReportOptions {
                redact: false,
                ..ReportOptions::default()
            },
            "now",
            &[],
        );
        assert!(
            plain.text.contains("jdoe.smith"),
            "unredacted only when asked"
        );
    }

    #[test]
    fn saved_build_logs_are_redacted() {
        let loaded = build_log_with_a_user_path();
        let prepared = crate::save::prepare(
            &loaded,
            &crate::view::Filter::default(),
            &crate::save::SaveRequest::default(),
            "now".to_owned(),
            &[],
        );
        let text = prepared.lines.join("\n");
        assert!(text.contains("C:/Users/<user>/Art/item_0001.png"));
        assert!(!text.contains("jdoe"));
        // Redaction keeps the report parseable: the saved build reads the same.
        assert!(text.contains("Build completed with a result of 'Succeeded'"));
    }

    #[test]
    fn a_report_for_an_incremental_build_names_the_last_clean_one() {
        // Keep only the first (incremental) build of the fixture and a copy of
        // the clean one before it: the latest build has no sizes.
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../comte-unity/tests/fixtures/editor_build.log"),
        )
        .unwrap()
        .replace("\r\n", "\n");
        let split = text.find("BuildPlayer: start building target 13").unwrap();
        let second = split
            + 1
            + text[split + 1..]
                .find("BuildPlayer: start building target 13")
                .unwrap();
        let reordered = format!(
            "{}{}{}",
            &text[..split],
            &text[second..],
            &text[split..second]
        );
        let path =
            std::env::temp_dir().join(format!("comte-build-order-{}.log", std::process::id()));
        std::fs::write(&path, reordered).unwrap();
        let loaded = load(&path, "Editor.log").unwrap();
        let _ = std::fs::remove_file(&path);

        let report = build_report(&loaded, None, &ReportOptions::default(), "now", &[]);
        assert!(
            report
                .text
                .contains("**Android build · Success · 3 min 04 s**"),
            "{}",
            report.text
        );
        assert!(report.text.contains("No sizes for this build"));
        assert!(report.text.contains(
            "Sizes from the last clean build, Android build · 11/24/2026 11:24:23 PM · Succeeded · 9 min 14 s:"
        ));
    }

    #[test]
    fn build_comparisons_are_redacted() {
        let before = fixture("editor_build.log");
        let after = build_log_with_a_user_path();
        let (a, b) = (
            &before.doc.build_reports()[1],
            &after.doc.build_reports()[1],
        );
        let text =
            build_comparison_markdown(("A", a, &before.raw), ("B", b, &after.raw), &[], true);
        // The renamed asset is gone from one place and new in the other.
        assert!(
            text.contains("`C:/Users/<user>/Art/item_0001.png`"),
            "{text}"
        );
        assert!(!text.contains("jdoe"), "{text}");
        let plain =
            build_comparison_markdown(("A", a, &before.raw), ("B", b, &after.raw), &[], false);
        assert!(plain.contains("jdoe.smith"));
    }

    #[test]
    fn redacted_report_masks_names_found_in_the_log() {
        let loaded = fixture("player_session.log");
        let built = build_report(&loaded, None, &ReportOptions::default(), "now", &[]);
        // The crash-report folder fact contains the player's Windows name.
        assert!(built.text.contains(r"C:/Users/<user>/AppData/Local/Temp"));
        assert!(!built.text.contains("Users/player"));
        assert!(built.redaction.unwrap().total > 0);

        let plain = build_report(
            &loaded,
            None,
            &ReportOptions {
                redact: false,
                ..ReportOptions::default()
            },
            "now",
            &[],
        );
        assert!(plain.text.contains("Users/player"));
        assert!(plain.redaction.is_none());
    }

    #[test]
    fn comparison_is_included_only_when_asked() {
        let a = fixture("editor_session.log");
        let b = fixture("player_session.log");
        let comparison = Comparison::of(&a.doc, &b.doc);
        let with = build_report(
            &b,
            Some(("Editor-prev.log", &comparison)),
            &ReportOptions::default(),
            "now",
            &[],
        );
        assert!(with.text.contains("## Changes since Editor-prev.log"));
        let without = build_report(
            &b,
            Some(("Editor-prev.log", &comparison)),
            &ReportOptions {
                include_comparison: false,
                ..ReportOptions::default()
            },
            "now",
            &[],
        );
        assert!(!without.text.contains("Changes since"));
    }

    #[test]
    fn system_summary_for_bug_reports_is_redacted() {
        let loaded = fixture("player_session.log");
        let summary = system_summary(&loaded, &[]);
        let lines: Vec<&str> = summary.lines().collect();
        assert_eq!(lines[0], "player_session.log (Unity Player)");
        assert!(lines[1].starts_with("Verdict: Crashed in Scene #3. Last error before it:"));
        assert_eq!(lines[2], "Problems: 1 crash, 3 exceptions, 1 warning");
        assert!(lines.contains(&"Engine version: 2019.4.40f1"));
        assert!(lines.contains(&"GPU: NVIDIA GeForce RTX 2070 Super"));
        assert!(
            lines.contains(&"Crash report: C:/Users/<user>/AppData/Local/Temp/Sample Studio/Sample Game/Crashes"),
            "{summary}"
        );
        assert_eq!(lines.last(), Some(&"Lines: 90"));
    }

    #[test]
    fn report_names_end_in_md() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 9, 14, 5, 0)
            .unwrap();
        assert_eq!(
            report_file_name("Editor.log", &now),
            "2026-11-09_14-05_Editor.md"
        );
    }
}
