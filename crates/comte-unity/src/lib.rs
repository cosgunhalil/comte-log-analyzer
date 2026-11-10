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
//! - [`locator`]: finds `Editor.log`, `Editor-prev.log` and every game's
//!   `Player.log` / `Player-prev.log` on this machine.
//! - [`UnitySupport`]: ties it together as a [`comte_core::EngineSupport`].

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
pub mod hints;
pub mod locator;
mod parser;

use comte_core::{EngineId, EngineSupport, LogKind, LogLocator, LogParser};

pub use detect::{detect_file, detect_kind};
pub use locator::{UnityLocator, UnityLogRoots};
pub use parser::{EditorParser, Flavor, PlayerParser, UnityParser};

/// Unity as an engine Comte Logger understands.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnitySupport;

impl EngineSupport for UnitySupport {
    fn id(&self) -> EngineId {
        EngineId::Unity
    }

    fn locator(&self) -> Box<dyn LogLocator> {
        Box::new(UnityLocator::new())
    }

    fn detect(&self, first_lines: &[&str]) -> Option<LogKind> {
        detect_kind(first_lines)
    }

    fn new_parser(&self, kind: LogKind) -> Box<dyn LogParser> {
        parser_for(kind)
    }
}

/// The right parser for a Unity log kind.
pub fn parser_for(kind: LogKind) -> Box<dyn LogParser> {
    match kind {
        LogKind::Editor => Box::new(EditorParser::new()),
        LogKind::Player => Box::new(PlayerParser::new()),
    }
}
