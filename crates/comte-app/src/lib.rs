//! Comte Log Analyzer: a human-friendly log analyzer for game engines.
//!
//! This library backs two programs:
//! - `comte-log-analyzer` (`src/main.rs`): the desktop app. On Windows it is a GUI
//!   program, so no console window opens next to it.
//! - `comte` (`src/bin/comte.rs`): the command line (`--list`, `--summary`,
//!   `--compare`, `--report`, `--save-build`, `--compare-builds`).

mod app;
mod build_history;
mod build_view;
mod cli;
mod compare_view;
mod devtools;
mod engines;
mod files;
mod history;
mod icon;
mod ide;
mod live;
mod loader;
mod overview;
mod pack;
mod pins;
mod report_export;
mod save;
mod search;
mod settings;
mod strip;
mod theme;
mod view;
mod zip;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use eframe::egui;

const USAGE: &str = "Usage:
  comte --list                      list the logs found on this machine
  comte --summary <log file>        print a parse summary
  comte --compare <A> <B>           print what changed from log A to log B
  comte --report <log file> [--slack] [--no-redact]
                                    print a Markdown (or Slack) report
  comte --save-build <log file> [--note <text>] [--to <folder>]
                                    save the log's latest build with sizes to
                                    the build history (redacted)
  comte --compare-builds <A> <B> [--no-redact]
                                    print what changed from build A to build B
                                    (Editor logs or saved builds; the latest
                                    build with sizes of each) as Markdown
  comte --compare-builds <log file> --last-saved
                                    the same, against the newest saved build of
                                    the same project

The desktop app is comte-log-analyzer [log file].";

/// Opens the desktop app, optionally with a log file.
pub fn run_gui(open: Option<PathBuf>) -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Comte Log Analyzer")
            .with_app_id("comte-log-analyzer")
            .with_icon(icon::window_icon())
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([960.0, 540.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Comte Log Analyzer",
        options,
        Box::new(move |cc| Ok(Box::new(app::ComteApp::new(cc, open)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The `comte` command line. `args` excludes the program name.
pub fn run_cli(args: &[std::ffi::OsString]) -> ExitCode {
    let path = |i: usize| args.get(i).map(Path::new);
    let report_error = |result: std::io::Result<()>| match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    };
    match args.first().and_then(|a| a.to_str()) {
        Some("--list") => {
            cli::list_sources();
            ExitCode::SUCCESS
        }
        Some("--summary") => match path(1) {
            Some(file) => report_error(cli::summarize(file)),
            None => usage_error(),
        },
        Some("--compare") => match (path(1), path(2)) {
            (Some(a), Some(b)) => report_error(cli::compare(a, b)),
            _ => usage_error(),
        },
        Some("--report") => match path(1) {
            Some(file) => {
                let flag = |name: &str| args.iter().skip(2).any(|a| a == name);
                report_error(cli::report(file, flag("--slack"), !flag("--no-redact")))
            }
            None => usage_error(),
        },
        Some("--save-build") => match path(1) {
            Some(file) => {
                let value = |name: &str| {
                    args.iter()
                        .position(|a| a == name)
                        .and_then(|i| args.get(i + 1))
                        .map(|v| v.to_string_lossy().into_owned())
                };
                report_error(cli::save_build(
                    file,
                    &value("--note").unwrap_or_default(),
                    value("--to").map(PathBuf::from),
                ))
            }
            None => usage_error(),
        },
        Some("--compare-builds") => {
            let flag = |name: &str| args.iter().skip(2).any(|a| a == name);
            match (path(1), path(2)) {
                (Some(b), _) if flag("--last-saved") => {
                    report_error(cli::compare_builds(None, b, !flag("--no-redact")))
                }
                (Some(a), Some(b)) if !a.as_os_str().to_string_lossy().starts_with("--") => {
                    report_error(cli::compare_builds(Some(a), b, !flag("--no-redact")))
                }
                _ => usage_error(),
            }
        }
        Some("--version" | "-V") => {
            println!("comte {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            println!("Comte Log Analyzer {}\n{USAGE}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown option: {other}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::FAILURE
}
