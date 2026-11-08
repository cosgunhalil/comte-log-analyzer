//! # comte-unity
//!
//! Unity support for Comte Logger.
//!
//! - [`detect`]: tells an Editor log from a Player log by its first lines.
//! - [`frames`]: parses, classifies and cleans up stack frames (both Unity's
//!   `Type:Method (args) (at file:line)` form and Mono's `at Type.Method () in file:line`).
//! - [`EditorParser`] / [`PlayerParser`]: streaming parsers for `Editor.log`
//!   and `Player.log` (and their `-prev` siblings), built on one shared
//!   [`UnityParser`].
//!
//! Log locator: M4 (see `docs/DESIGN.md` §11).

/// Declares a lazily compiled, module-private regex.
macro_rules! regex {
    ($name:ident = $pattern:expr) => {
        static $name: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new($pattern).expect("valid regex"));
    };
}

mod common;
pub mod detect;
pub mod frames;
mod parser;

use comte_core::{LogKind, LogParser};

pub use detect::detect_kind;
pub use parser::{EditorParser, Flavor, PlayerParser, UnityParser};

/// The right parser for a Unity log kind.
pub fn parser_for(kind: LogKind) -> Box<dyn LogParser> {
    match kind {
        LogKind::Editor => Box::new(EditorParser::new()),
        LogKind::Player => Box::new(PlayerParser::new()),
    }
}
