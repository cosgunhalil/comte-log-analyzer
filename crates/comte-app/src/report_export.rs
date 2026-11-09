//! Building the pasteable report (DESIGN §6.3) for the open log, with the
//! same redaction as saved sessions.

use comte_core::{
    Comparison, RedactionReport, Redactor, ReportInput, ReportStyle, parse_tags, render_report,
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
