//! Keeping the lists in step with the disk, and showing files in the file
//! manager.
//!
//! Logs get deleted, renamed and replaced while the app is open (a game is
//! uninstalled, a tester cleans up, Unity rotates its logs). The left panel
//! re-checks its files every few seconds and drops the ones that are gone, so
//! it never offers a log that can't be opened.

use std::io;
use std::path::Path;

use comte_core::{LogKind, LogSource};

use crate::save::SessionFile;

/// Shows `path` in the system's file manager, selected when possible.
pub fn reveal(path: &Path) -> io::Result<()> {
    // A log opened by a relative path (`comte-log-analyzer.exe Player.log`)
    // must still select the right file.
    let path = &std::path::absolute(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `explorer /select,"C:\…\Player.log"` opens the folder with the file
        // selected. The quotes must wrap only the path, so the argument is
        // passed raw rather than quoted as a whole.
        let native = path.to_string_lossy().replace('/', "\\");
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{native}\""))
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // No portable "select this file": open its folder.
        let folder = path.parent().unwrap_or(path);
        std::process::Command::new("xdg-open").arg(folder).spawn()?;
        Ok(())
    }
}

/// What a refresh of the lists changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Titles of the logs and sessions dropped because their file is gone.
    pub removed: Vec<String>,
}

/// Re-reads each source's size and date, and drops Player logs whose file is
/// gone. The Editor logs stay listed even when missing: they are Unity's
/// fixed locations and come back the next time the Editor runs.
pub fn prune_sources(sources: &mut Vec<LogSource>) -> Pruned {
    let mut pruned = Pruned::default();
    sources.retain_mut(|source| {
        let metadata = std::fs::metadata(&source.path).ok().filter(|m| m.is_file());
        source.size = metadata.as_ref().map(std::fs::Metadata::len);
        source.modified = metadata.and_then(|m| m.modified().ok());
        let keep = source.size.is_some() || source.kind == LogKind::Editor;
        if !keep {
            pruned.removed.push(source.title.clone());
        }
        keep
    });
    pruned
}

/// Drops saved sessions whose file is gone.
pub fn prune_sessions(sessions: &mut Vec<SessionFile>) -> Pruned {
    let mut pruned = Pruned::default();
    sessions.retain(|session| {
        let keep = session.path.is_file();
        if !keep {
            pruned.removed.push(session.title.clone());
        }
        keep
    });
    pruned
}

#[cfg(test)]
mod tests {
    use comte_core::{EngineId, SourceRole};

    use super::*;

    #[test]
    fn deleted_player_logs_and_sessions_leave_the_lists() {
        let dir = std::env::temp_dir().join(format!("comte-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kept = dir.join("Kept.log");
        let gone = dir.join("Gone.log");
        std::fs::write(&kept, "hello").unwrap();
        std::fs::write(&gone, "bye").unwrap();
        let player = |title: &str, path: &Path| {
            LogSource::from_path(
                EngineId::Unity,
                LogKind::Player,
                SourceRole::Current,
                title,
                path,
            )
        };
        let mut sources = vec![
            LogSource::from_path(
                EngineId::Unity,
                LogKind::Editor,
                SourceRole::Current,
                "Editor.log",
                dir.join("Editor.log"),
            ),
            player("Studio › Kept", &kept),
            player("Studio › Gone", &gone),
        ];
        std::fs::remove_file(&gone).unwrap();
        std::fs::write(&kept, "hello again").unwrap();

        let pruned = prune_sources(&mut sources);
        assert_eq!(pruned.removed, ["Studio › Gone"]);
        let titles: Vec<&str> = sources.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Editor.log", "Studio › Kept"],
            "missing Editor.log stays, greyed"
        );
        assert!(!sources[0].exists());
        assert_eq!(sources[1].size, Some(11), "sizes are refreshed");

        let mut sessions = vec![
            SessionFile {
                path: kept.clone(),
                title: "kept session".to_owned(),
                modified: None,
                tags: Vec::new(),
                note: String::new(),
            },
            SessionFile {
                path: gone,
                title: "gone session".to_owned(),
                modified: None,
                tags: Vec::new(),
                note: String::new(),
            },
        ];
        assert_eq!(prune_sessions(&mut sessions).removed, ["gone session"]);
        assert_eq!(sessions.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
