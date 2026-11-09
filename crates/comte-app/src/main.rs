//! Comte Logger: a human-friendly log analyzer for game engines.
//!
//! - `comte-logger` opens the desktop UI (and the Unity Editor log, if found).
//! - `comte-logger <log file>` opens the UI with that file.
//! - `comte-logger --list` lists the logs found on this machine.
//! - `comte-logger --summary <log file>` prints a parse summary.

mod app;
mod cli;
mod devtools;
mod engines;
mod ide;
mod live;
mod loader;
mod overview;
mod save;
mod settings;
mod theme;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;

const USAGE: &str = "Usage:
  comte-logger                      open the desktop app
  comte-logger <log file>           open the desktop app with this file
  comte-logger --list               list logs found on this machine
  comte-logger --summary <log file> print a parse summary";

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("--help" | "-h") => {
            println!("Comte Logger {}\n{USAGE}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--list") => {
            cli::list_sources();
            ExitCode::SUCCESS
        }
        Some("--summary") => {
            let Some(path) = args.get(1) else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            let path = std::path::Path::new(path);
            match cli::summarize(path) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {}: {err}", path.display());
                    ExitCode::FAILURE
                }
            }
        }
        _ => run_gui(args.first().map(PathBuf::from)),
    }
}

fn run_gui(open: Option<PathBuf>) -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Comte Logger")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([960.0, 540.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Comte Logger",
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
