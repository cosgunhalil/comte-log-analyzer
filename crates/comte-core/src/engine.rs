//! Traits each engine crate (`comte-unity`, later `comte-unreal`, ...) implements.

use crate::parser::LogParser;
use crate::source::{EngineId, LogKind, LogSource};

/// Finds this engine's log files on the local machine.
pub trait LogLocator: Send + Sync {
    /// Known log files (current and previous Editor logs, discovered Player
    /// logs, ...). Missing files may be included with `size: None` so the UI
    /// can show them greyed out.
    fn discover(&self) -> Vec<LogSource>;
}

/// Everything the app needs from one engine.
pub trait EngineSupport: Send + Sync {
    fn id(&self) -> EngineId;

    fn locator(&self) -> Box<dyn LogLocator>;

    /// Guesses the log kind from the first lines of a file, so dropped or
    /// hand-opened files get the right parser. `None` = not this engine.
    fn detect(&self, first_lines: &[&str]) -> Option<LogKind>;

    fn new_parser(&self, kind: LogKind) -> Box<dyn LogParser>;
}
