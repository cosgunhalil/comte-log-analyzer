//! Finds Unity log files on this machine (DESIGN §1).
//!
//! | Log | Windows | macOS | Linux |
//! |---|---|---|---|
//! | Editor | `%LOCALAPPDATA%\Unity\Editor\Editor.log` | `~/Library/Logs/Unity/Editor.log` | `~/.config/unity3d/Editor.log` |
//! | Player | `…\AppData\LocalLow\<Company>\<Product>\Player.log` | `~/Library/Logs/<Company>/<Product>/Player.log` | `~/.config/unity3d/<Company>/<Product>/Player.log` |
//!
//! Each location also has a `-prev.log` sibling from the previous run.
//! Only the Windows paths are verified for v1; macOS and Linux are wired up
//! but untested (milestone M12).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use comte_core::{EngineId, LogKind, LogLocator, LogSource, SourceRole};

/// Separator between company and product in Player source titles.
pub const TITLE_SEPARATOR: &str = " › ";

/// Where Unity writes its logs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnityLogRoots {
    /// Folder holding `Editor.log` and `Editor-prev.log`.
    pub editor_dir: Option<PathBuf>,
    /// Folder holding `<Company>/<Product>/Player.log`.
    pub player_root: Option<PathBuf>,
}

impl UnityLogRoots {
    /// The standard locations for the current OS and user.
    pub fn for_this_machine() -> Self {
        platform_roots()
    }
}

#[cfg(windows)]
fn platform_roots() -> UnityLogRoots {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let editor_dir = local.as_ref().map(|l| l.join("Unity").join("Editor"));
    // LocalLow sits next to Local: C:\Users\<user>\AppData\LocalLow
    let player_root = local
        .as_ref()
        .and_then(|l| l.parent().map(|appdata| appdata.join("LocalLow")))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join("AppData").join("LocalLow"))
        });
    UnityLogRoots {
        editor_dir,
        player_root,
    }
}

#[cfg(target_os = "macos")]
fn platform_roots() -> UnityLogRoots {
    let logs =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Logs"));
    UnityLogRoots {
        editor_dir: logs.as_ref().map(|l| l.join("Unity")),
        player_root: logs,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_roots() -> UnityLogRoots {
    let unity =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("unity3d"));
    UnityLogRoots {
        editor_dir: unity.clone(),
        player_root: unity,
    }
}

#[cfg(not(any(windows, unix)))]
fn platform_roots() -> UnityLogRoots {
    UnityLogRoots::default()
}

/// [`LogLocator`] for Unity.
///
/// `discover` returns, in order:
/// 1. `Editor.log` and `Editor-prev.log` (always listed; missing files have
///    `size: None` so the UI can grey them out),
/// 2. every game with a `Player.log` / `Player-prev.log`, the most recently
///    written game first, each game's current log before its previous one.
#[derive(Debug, Clone)]
pub struct UnityLocator {
    roots: UnityLogRoots,
}

impl UnityLocator {
    pub fn new() -> Self {
        Self::with_roots(UnityLogRoots::for_this_machine())
    }

    pub fn with_roots(roots: UnityLogRoots) -> Self {
        Self { roots }
    }

    pub fn roots(&self) -> &UnityLogRoots {
        &self.roots
    }
}

impl Default for UnityLocator {
    fn default() -> Self {
        Self::new()
    }
}

impl LogLocator for UnityLocator {
    fn discover(&self) -> Vec<LogSource> {
        let mut sources = Vec::new();
        if let Some(dir) = &self.roots.editor_dir {
            sources.extend(editor_sources(dir));
        }
        if let Some(root) = &self.roots.player_root {
            sources.extend(discover_players(root));
        }
        sources
    }
}

/// `Editor.log` and `Editor-prev.log` in `dir`, whether or not they exist.
pub fn editor_sources(dir: &Path) -> Vec<LogSource> {
    [
        ("Editor.log", SourceRole::Current),
        ("Editor-prev.log", SourceRole::Previous),
    ]
    .into_iter()
    .map(|(name, role)| {
        LogSource::from_path(EngineId::Unity, LogKind::Editor, role, name, dir.join(name))
    })
    .collect()
}

/// Scans `root/<Company>/<Product>/` for `Player.log` and `Player-prev.log`.
/// Unreadable folders are skipped silently: this runs at startup and must
/// never fail because of one odd directory.
pub fn discover_players(root: &Path) -> Vec<LogSource> {
    let mut games: Vec<(Option<SystemTime>, String, Vec<LogSource>)> = Vec::new();

    for company in subdirectories(root) {
        let company_name = file_name_lossy(&company);
        for product in subdirectories(&company) {
            let title = format!(
                "{company_name}{TITLE_SEPARATOR}{}",
                file_name_lossy(&product)
            );
            let logs: Vec<LogSource> = [
                ("Player.log", SourceRole::Current),
                ("Player-prev.log", SourceRole::Previous),
            ]
            .into_iter()
            .map(|(name, role)| (product.join(name), role))
            .filter(|(path, _)| path.is_file())
            .map(|(path, role)| {
                LogSource::from_path(EngineId::Unity, LogKind::Player, role, title.clone(), path)
            })
            .collect();
            if !logs.is_empty() {
                let newest = logs.iter().filter_map(|s| s.modified).max();
                games.push((newest, title, logs));
            }
        }
    }

    // Newest first; ties (or unknown times) by title so the order is stable.
    games.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    games.into_iter().flat_map(|(_, _, logs)| logs).collect()
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

fn file_name_lossy(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;

    use super::*;

    /// A temporary directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "comte-locator-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, relative: &str, age_secs: u64) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = File::create(&path).unwrap();
            let modified = SystemTime::now() - Duration::from_secs(age_secs);
            file.set_modified(modified).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn editor_logs_are_always_listed() {
        let tmp = TempDir::new("editor");
        tmp.file("Editor.log", 0);
        let sources = editor_sources(&tmp.0);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].title, "Editor.log");
        assert!(sources[0].exists());
        assert_eq!(sources[1].role, SourceRole::Previous);
        assert!(
            !sources[1].exists(),
            "missing Editor-prev.log is listed greyed out"
        );
    }

    #[test]
    fn players_are_grouped_by_game_newest_first() {
        let tmp = TempDir::new("players");
        tmp.file("Studio A/Game One/Player.log", 3600);
        tmp.file("Studio A/Game One/Player-prev.log", 7200);
        tmp.file("Studio A/Game One/Player-browser.log", 0); // not a Unity player log
        tmp.file("Studio B/Game Two/Player.log", 60);
        tmp.file("Studio B/Empty/readme.txt", 0);
        tmp.file("Loose/Player.log", 0); // too shallow: needs <Company>/<Product>/

        let sources = discover_players(&tmp.0);
        let listed: Vec<_> = sources.iter().map(|s| (s.title.as_str(), s.role)).collect();
        assert_eq!(
            listed,
            [
                ("Studio B › Game Two", SourceRole::Current),
                ("Studio A › Game One", SourceRole::Current),
                ("Studio A › Game One", SourceRole::Previous),
            ]
        );
        assert!(
            sources
                .iter()
                .all(|s| s.kind == LogKind::Player && s.exists())
        );
    }

    #[test]
    fn missing_roots_discover_nothing() {
        let locator = UnityLocator::with_roots(UnityLogRoots {
            editor_dir: None,
            player_root: Some(PathBuf::from("definitely/not/here")),
        });
        assert!(locator.discover().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots_point_at_local_and_locallow() {
        let roots = UnityLogRoots::for_this_machine();
        let editor = roots.editor_dir.unwrap();
        assert!(editor.ends_with("Unity/Editor") || editor.ends_with(r"Unity\Editor"));
        assert!(roots.player_root.unwrap().ends_with("LocalLow"));
    }
}
