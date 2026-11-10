//! What the engine was doing: assets it imported (and how long each took),
//! an operation still running when the log ends, and build size reports.
//!
//! Parsers emit these as [`crate::ParseEvent`]s; the document keeps them and
//! [`crate::Insights`] turns them into "Stopped while importing X", the
//! slowest imports and the largest assets in a build.

use std::time::Duration;

/// One asset import that finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetImport {
    /// Project-relative path, e.g. `Assets/Models/Ship.fbx`.
    pub path: String,
    pub duration: Duration,
    /// Line the import started on.
    pub line: usize,
}

/// An operation that started but has no end in the log: the engine is stuck
/// on it, still busy with it, or quit (or was killed) before it finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfinished {
    /// "Stopped while importing Assets/Models/Ship.fbx"
    pub headline: String,
    /// What to do about it, in one sentence.
    pub hint: String,
    /// The thing it was working on (an asset path), when there is one.
    pub subject: Option<String>,
    /// Line the operation started on.
    pub line: usize,
    /// Other operations also still running ("importing Assets/b.png").
    pub also: Vec<String>,
}

/// A known problem recognized in the log, explained in plain words: what it
/// most likely is, and what to try. Parsers know their engine's patterns
/// ("device removed", "No space left on device", a crash in a GPU driver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// "The graphics driver reset (device removed)"
    pub title: String,
    /// What it means and what to try, one sentence each.
    pub advice: Vec<String>,
    /// Line of the entry it is about.
    pub line: usize,
    /// A file or folder that helps (a crash report folder).
    pub path: Option<String>,
    /// What it is about ("upm", "gradle", "licensing", "crash", …), so the
    /// app can offer the matching companion logs.
    pub topic: String,
}

/// One step of a domain reload, from the engine's profiling tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadPhase {
    /// The engine's name, e.g. `ProcessInitializeOnLoadAttributes`.
    pub name: String,
    /// Nesting in the tree (1 = top-level step).
    pub depth: u8,
    pub duration: Duration,
    /// What the step does, in plain words, when the parser knows it.
    pub about: Option<String>,
    /// The step runs the project's own code (and its packages').
    pub your_code: bool,
}

/// Where one domain reload spent its time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadProfile {
    pub line: usize,
    pub total: Duration,
    /// In tree order.
    pub phases: Vec<ReloadPhase>,
}

impl ReloadProfile {
    /// Steps with no sub-steps: where the time actually goes.
    pub fn leaves(&self) -> impl Iterator<Item = &ReloadPhase> {
        self.phases.iter().enumerate().filter_map(|(i, phase)| {
            let has_children = self
                .phases
                .get(i + 1)
                .is_some_and(|next| next.depth > phase.depth);
            (!has_children).then_some(phase)
        })
    }
}

/// One row of a size table: a category ("Textures") or an asset path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeRow {
    pub name: String,
    pub bytes: u64,
}

/// A build's size report (Unity prints one after each player build).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildReport {
    /// Line the report starts on.
    pub line: usize,
    /// "Complete build size".
    pub total: Option<u64>,
    /// "Total User Assets": what the percentages are relative to.
    pub user_assets: Option<u64>,
    /// Uncompressed size per category (Textures, Meshes, …), as listed.
    pub categories: Vec<SizeRow>,
    /// Assets by uncompressed size, largest first, as listed.
    pub assets: Vec<SizeRow>,
}

/// `12.3 mb`, `512.0 kb`, `1.2 gb`, `340 b` → bytes (1 kb = 1024 b).
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "b" | "bytes" => 1.0,
        "kb" => 1024.0,
        "mb" => 1024.0 * 1024.0,
        "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * scale).round() as u64)
}

/// Bytes for people: `340 B`, `12.3 KB`, `4.0 MB`, `1.20 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_trip_for_humans() {
        assert_eq!(parse_size("12.0 mb"), Some(12 * 1024 * 1024));
        assert_eq!(parse_size(" 512.0 kb"), Some(512 * 1024));
        assert_eq!(parse_size("340 b"), Some(340));
        assert_eq!(parse_size("1.5 GB"), Some(1_610_612_736));
        assert_eq!(parse_size("12 parsecs"), None);
        assert_eq!(parse_size("mb"), None);
        assert_eq!(format_bytes(340), "340 B");
        assert_eq!(format_bytes(12 * 1024 * 1024 + 300 * 1024), "12.3 MB");
        assert_eq!(format_bytes(1_610_612_736), "1.50 GB");
    }
}
