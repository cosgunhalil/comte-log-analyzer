//! Comte Logger desktop application.
//!
//! The egui UI arrives in M5. Until then this binary is a small smoke test:
//! `comte-logger <log file>` detects the log kind, parses it and prints a summary.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use comte_core::{LogKind, LogParser, PlainTextParser, format_duration, parse_file};
use comte_unity::EditorParser;
use comte_unity::detect::{DETECT_LINES, detect_kind};

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        println!(
            "Comte Logger {}\nThe desktop UI arrives in M5.\nUsage: comte-logger <log file>",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::SUCCESS;
    };

    match summarize(Path::new(&path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}: {err}", Path::new(&path).display());
            ExitCode::FAILURE
        }
    }
}

fn summarize(path: &Path) -> std::io::Result<()> {
    let head: Vec<String> = BufReader::new(File::open(path)?)
        .lines()
        .take(DETECT_LINES)
        .map_while(Result::ok)
        .collect();
    let head: Vec<&str> = head.iter().map(String::as_str).collect();
    let kind = detect_kind(&head);

    // The Player parser arrives in M3; until then Player logs use plain text.
    let mut parser: Box<dyn LogParser> = match kind {
        Some(LogKind::Editor) => Box::new(EditorParser::new()),
        _ => Box::new(PlainTextParser),
    };
    let started = Instant::now();
    let doc = parse_file(path, parser.as_mut())?;
    let elapsed = started.elapsed();

    let counts = doc.counts();
    println!("file:     {}", path.display());
    println!("kind:     Unity {}", kind.map_or("unknown", |k| k.label()));
    println!(
        "parsed:   {} lines -> {} entries in {}",
        doc.line_count(),
        doc.entries().len(),
        format_duration(elapsed)
    );
    println!(
        "severity: {} errors, {} warnings, {} info",
        counts.errors(),
        counts.warning,
        counts.info
    );
    for fact in doc.facts() {
        println!("fact:     {}: {}", fact.key.label(), fact.value);
    }
    println!("sections:");
    for section in doc.sections() {
        let duration = section
            .duration
            .map(|d| format!(" ({})", format_duration(d)))
            .unwrap_or_default();
        println!(
            "  {}{} L{}-{}{duration}  errors={} warnings={}",
            "  ".repeat(section.depth as usize),
            section.title,
            section.line_range.start,
            section.line_range.end - 1,
            section.counts.errors(),
            section.counts.warning
        );
    }
    println!("problems:");
    for entry in doc
        .entries()
        .iter()
        .filter(|e| e.severity.is_error_like() || e.severity == comte_core::Severity::Warning)
    {
        let culprit = entry
            .culprit()
            .map(|c| format!(" @ {}", c.short()))
            .unwrap_or_default();
        let message: String = entry.message.chars().take(100).collect();
        println!(
            "  L{:<6} {:<9} {message}{culprit}",
            entry.line_range.start,
            entry.severity.label()
        );
    }
    Ok(())
}
