//! Comte Logger desktop application.
//!
//! The egui UI arrives in M5. Until then this binary is a small smoke test:
//!
//! - `comte-logger --list` lists the logs found on this machine.
//! - `comte-logger <log file>` detects the log kind, parses it and prints a summary.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Instant, SystemTime};

use comte_core::{
    DETECT_LINES, EngineSupport, LogParser, LogSource, PlainTextParser, SourceRole, detect_engine,
    format_duration, parse_file, read_head,
};
use comte_unity::UnitySupport;

const USAGE: &str = "Usage:\n  comte-logger --list        list logs found on this machine\n  comte-logger <log file>    parse a log and print a summary";

/// Every engine Comte Logger understands. Detection tries them in order.
fn engines() -> Vec<Box<dyn EngineSupport>> {
    vec![Box::new(UnitySupport)]
}

fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        println!(
            "Comte Logger {}\nThe desktop UI arrives in M5.\n{USAGE}",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::SUCCESS;
    };

    if arg == "--list" {
        list_sources();
        return ExitCode::SUCCESS;
    }

    match summarize(Path::new(&arg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}: {err}", Path::new(&arg).display());
            ExitCode::FAILURE
        }
    }
}

fn list_sources() {
    for engine in engines() {
        let sources = engine.locator().discover();
        println!("{} ({} logs)", engine.id(), sources.len());
        for source in &sources {
            print_source(source);
        }
    }
}

fn print_source(source: &LogSource) {
    let role = match source.role {
        SourceRole::Current => "",
        SourceRole::Previous => " (previous)",
        SourceRole::Other => " (other)",
    };
    let status = match (source.size, source.modified) {
        (Some(size), modified) => format!(
            "{:>9}  {}",
            human_size(size),
            modified.map(age).unwrap_or_default()
        ),
        (None, _) => "  missing".to_owned(),
    };
    println!(
        "  {:<7} {:<55} {status}",
        source.kind.label(),
        format!("{}{role}", source.title)
    );
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

fn age(modified: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match secs {
        s if s < 60 => "just now".to_owned(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

fn summarize(path: &Path) -> std::io::Result<()> {
    let head = read_head(path, DETECT_LINES)?;
    let head: Vec<&str> = head.iter().map(String::as_str).collect();
    let engines = engines();
    let detected = detect_engine(&engines, &head);
    let kind = detected.map(|(_, kind)| kind);

    let mut parser: Box<dyn LogParser> = match detected {
        Some((engine, kind)) => engine.new_parser(kind),
        None => Box::new(PlainTextParser),
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
