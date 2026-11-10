//! Detects whether a file is a Unity Editor log or a Unity Player log.

use std::io;
use std::path::Path;

use comte_core::{LogKind, read_head};

/// How many leading lines are worth inspecting. Both kinds identify
/// themselves within the first few dozen lines.
pub const DETECT_LINES: usize = comte_core::DETECT_LINES;

/// Reads the start of a file and detects its Unity log kind.
/// Used for files opened by hand or dropped onto the window, whose name
/// says nothing reliable (testers rename them).
pub fn detect_file(path: impl AsRef<Path>) -> io::Result<Option<LogKind>> {
    let head = read_head(path, DETECT_LINES)?;
    let head: Vec<&str> = head.iter().map(String::as_str).collect();
    Ok(detect_kind(&head))
}

/// Returns the Unity log kind, or `None` if the lines don't look like Unity.
///
/// Editor markers (Unity 6000.x and 2019+):
/// - `Built from '6000.3/staging' branch; Version is '…'`
/// - `[Licensing::Module] …` as the first line
/// - `BatchMode: 0, IsHumanControllingUs: 1, …`
///
/// Player markers:
/// - `Initialize engine version: 2019.4.40f1 (…)`
/// - `Loading player data from …/<Game>_Data/data.unity3d`
/// - `Mono path[0] = '…/<Game>_Data/Managed'`
pub fn detect_kind(first_lines: &[&str]) -> Option<LogKind> {
    for line in first_lines.iter().take(DETECT_LINES) {
        // Logs written with `-timestamps` prefix every line.
        let line = crate::common::split_timestamp(line).map_or(*line, |(_, rest)| rest);
        let line = line.trim();
        if is_player_marker(line) {
            return Some(LogKind::Player);
        }
        if is_editor_marker(line) {
            return Some(LogKind::Editor);
        }
    }
    None
}

fn is_player_marker(line: &str) -> bool {
    line.starts_with("Initialize engine version:")
        || line.starts_with("Loading player data from")
        || (line.starts_with("Mono path[0] = '") && line.ends_with("_Data/Managed'"))
}

fn is_editor_marker(line: &str) -> bool {
    (line.starts_with("Built from '") && line.contains("' branch; Version is '"))
        || line.starts_with("[Licensing::")
        || line.starts_with("BatchMode:")
        || (line.starts_with("Mono path[0] = '") && line.contains("/Editor/Data/Managed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamped_logs_are_detected() {
        assert_eq!(
            detect_kind(&[
                "2022-11-29T17:39:41.361Z|0x3598|[Licensing::Module] Trying to connect to existing licensing client channel..."
            ]),
            Some(LogKind::Editor)
        );
        assert_eq!(
            detect_kind(&[
                "2024-01-02T10:00:00.000Z|0x1f|Initialize engine version: 2022.3.11f1 (d00248457e15)"
            ]),
            Some(LogKind::Player)
        );
    }

    #[test]
    fn detects_unity6_editor_log() {
        let lines = [
            "[Licensing::Module] Trying to connect to existing licensing client channel...",
            "Built from '6000.3/staging' branch; Version is '6000.3.11f1 (3000ef702840) revision 3145967'; Using compiler version '194234433'; Build Type 'Release'",
        ];
        assert_eq!(detect_kind(&lines), Some(LogKind::Editor));
    }

    #[test]
    fn detects_editor_log_by_mono_path() {
        let lines =
            ["Mono path[0] = 'C:/Program Files/Unity/Hub/Editor/6000.3.11f1/Editor/Data/Managed'"];
        assert_eq!(detect_kind(&lines), Some(LogKind::Editor));
    }

    #[test]
    fn detects_player_log_by_mono_path_before_engine_version() {
        let lines = [
            "Mono path[0] = 'C:/Program Files/Epic Games/MyGame/MyGame_Data/Managed'",
            "Mono config path = 'C:/Program Files/Epic Games/MyGame/MonoBleedingEdge/etc'",
            "Initialize engine version: 2019.4.40f1 (ffc62b691db5)",
        ];
        assert_eq!(detect_kind(&lines), Some(LogKind::Player));
    }

    #[test]
    fn detects_player_log_by_data_path() {
        let lines = [
            "Loading player data from C:/Games/MyGame/MyGame_Data/data.unity3d",
            "Initialize engine version: 2019.4.16f1 (e05b6e02d63e)",
        ];
        assert_eq!(detect_kind(&lines), Some(LogKind::Player));
    }

    #[test]
    fn unknown_text_is_not_unity() {
        assert_eq!(detect_kind(&["hello", "world"]), None);
        assert_eq!(detect_kind(&[]), None);
    }
}
