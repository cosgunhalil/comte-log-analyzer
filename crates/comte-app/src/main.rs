//! Comte Logger desktop application.
//!
//! The egui UI arrives in M5. Until then this binary is a small smoke test:
//! `comte-logger <log file>` detects the log kind and prints a summary.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::ExitCode;

use comte_core::{PlainTextParser, parse_file};
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
        .filter_map(Result::ok)
        .collect();
    let head: Vec<&str> = head.iter().map(String::as_str).collect();
    let kind = detect_kind(&head).map_or("unknown", |k| k.label());

    // Unity parsers arrive in M2/M3; plain text keeps the pipeline exercised.
    let doc = parse_file(path, &mut PlainTextParser)?;
    println!("file:    {}", path.display());
    println!("kind:    Unity {kind}");
    println!("lines:   {}", doc.line_count());
    println!("entries: {}", doc.entries().len());
    Ok(())
}
