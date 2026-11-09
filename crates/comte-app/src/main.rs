//! `comte-logger`: the Comte Logger desktop app.
//!
//! `comte-logger [log file]` opens the app (with the Unity Editor log when no
//! file is given). The command line lives in the separate `comte` program.

// A GUI program on Windows: no console window next to the app.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    comte_app::run_gui(open)
}
