//! Command-line mode: `--list` and `--summary <file>`.

use std::path::Path;
use std::time::SystemTime;

use comte_core::{LogSource, SourceRole, format_duration};

use crate::engines::engines;
use crate::loader::load;

pub fn list_sources() {
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

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

pub fn age(modified: SystemTime) -> String {
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

pub fn summarize(path: &Path) -> std::io::Result<()> {
    // Same loading as the app: detection, saved-session headers, plain text.
    let loaded = load(path, &path.display().to_string())?;
    let doc = &loaded.doc;
    let elapsed = loaded.elapsed;

    let counts = doc.counts();
    println!("file:     {}", path.display());
    println!("kind:     {}", loaded.kind_label);
    if let Some(session) = &loaded.session {
        println!("tags:     {}", session.tags.join(", "));
        if let Some(first) = session.note.lines().next() {
            println!("note:     {first}");
        }
    }
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
