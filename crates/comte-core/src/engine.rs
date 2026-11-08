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

/// How many leading lines are read to detect which engine wrote a log.
pub const DETECT_LINES: usize = 64;

/// The first engine that recognizes `head` (the first lines of a file), and
/// the log kind it detected. `None` = no engine knows this file; open it with
/// [`crate::PlainTextParser`].
pub fn detect_engine<'a>(
    engines: &'a [Box<dyn EngineSupport>],
    head: &[&str],
) -> Option<(&'a dyn EngineSupport, LogKind)> {
    engines
        .iter()
        .find_map(|engine| engine.detect(head).map(|kind| (engine.as_ref(), kind)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::PlainTextParser;

    #[derive(Debug)]
    struct Fake {
        marker: &'static str,
        kind: LogKind,
    }

    impl EngineSupport for Fake {
        fn id(&self) -> EngineId {
            EngineId::Unity
        }
        fn locator(&self) -> Box<dyn LogLocator> {
            unimplemented!("not needed")
        }
        fn detect(&self, head: &[&str]) -> Option<LogKind> {
            head.iter()
                .any(|l| l.contains(self.marker))
                .then_some(self.kind)
        }
        fn new_parser(&self, _kind: LogKind) -> Box<dyn LogParser> {
            Box::new(PlainTextParser)
        }
    }

    #[test]
    fn first_matching_engine_wins() {
        let engines: Vec<Box<dyn EngineSupport>> = vec![
            Box::new(Fake {
                marker: "player",
                kind: LogKind::Player,
            }),
            Box::new(Fake {
                marker: "editor",
                kind: LogKind::Editor,
            }),
        ];
        let (_, kind) = detect_engine(&engines, &["x", "editor here"]).unwrap();
        assert_eq!(kind, LogKind::Editor);
        assert!(detect_engine(&engines, &["nothing"]).is_none());
    }
}
