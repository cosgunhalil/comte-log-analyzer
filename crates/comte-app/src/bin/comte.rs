//! `comte`: the Comte Logger command line.
//!
//! `comte --list`, `--summary <file>`, `--compare <A> <B>`, `--report <file>`.
//! The desktop app is the separate `comte-logger` program.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    comte_app::run_cli(&args)
}
