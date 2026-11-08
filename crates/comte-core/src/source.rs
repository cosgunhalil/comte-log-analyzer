//! Descriptions of log files that can be opened.

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineId {
    Unity,
}

impl EngineId {
    pub fn label(self) -> &'static str {
        match self {
            EngineId::Unity => "Unity",
        }
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Which program wrote the log. Selects the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogKind {
    Editor,
    Player,
}

impl LogKind {
    pub fn label(self) -> &'static str {
        match self {
            LogKind::Editor => "Editor",
            LogKind::Player => "Player",
        }
    }
}

/// Whether this is the latest log or the one kept from the previous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRole {
    /// `Editor.log`, `Player.log`
    Current,
    /// `Editor-prev.log`, `Player-prev.log`
    Previous,
    /// Opened by hand / dropped onto the window.
    Other,
}

/// A log file the app can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSource {
    pub engine: EngineId,
    pub kind: LogKind,
    pub role: SourceRole,
    /// Display name, e.g. `Editor.log` or `Flanne › MinutesTillDawn`.
    pub title: String,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

impl LogSource {
    /// Builds a source and fills `size` / `modified` from the file system
    /// (left empty if the file cannot be read).
    pub fn from_path(
        engine: EngineId,
        kind: LogKind,
        role: SourceRole,
        title: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        let path = path.into();
        let metadata = std::fs::metadata(&path).ok();
        Self {
            engine,
            kind,
            role,
            title: title.into(),
            size: metadata.as_ref().map(|m| m.len()),
            modified: metadata.and_then(|m| m.modified().ok()),
            path,
        }
    }

    pub fn exists(&self) -> bool {
        self.size.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_has_no_metadata() {
        let source = LogSource::from_path(
            EngineId::Unity,
            LogKind::Editor,
            SourceRole::Current,
            "Editor.log",
            "definitely/not/here/Editor.log",
        );
        assert!(!source.exists());
        assert!(source.modified.is_none());
    }

    #[test]
    fn existing_file_has_metadata() {
        let source = LogSource::from_path(
            EngineId::Unity,
            LogKind::Editor,
            SourceRole::Other,
            "Cargo.toml",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        );
        assert!(source.exists());
        assert!(source.size.unwrap() > 0);
    }
}
