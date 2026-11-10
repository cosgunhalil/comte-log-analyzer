//! Settings kept between runs, in `settings.toml` under the user's config
//! folder (`%APPDATA%\ComteLogger\config` on Windows). Edited in the
//! Settings window (⚙ in the toolbar).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ide::IdeChoice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow Windows' light/dark setting.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "Same as Windows",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where "open in IDE" sends files.
    pub ide: IdeChoice,
    /// Template for [`IdeChoice::Custom`]: `{file}`, `{line}`, `{column}`.
    pub custom_command: String,
    /// Unity project folder used when a log doesn't name one that exists
    /// (Player logs, logs from another machine, redacted sessions).
    pub project_root: Option<PathBuf>,
    /// Words always redacted in sessions and reports (comma-separated),
    /// e.g. a display name the redaction rules can't recognize.
    pub also_redact: String,
    /// Where sessions are saved; `None` = `Documents\ComteLogger\sessions`.
    pub sessions_dir: Option<PathBuf>,
    /// Starting value of "Collapse duplicates".
    pub collapse_duplicates: bool,
    /// Starting value of "Engine noise".
    pub show_noise: bool,
    /// Turn on live tail whenever `Editor.log` is opened.
    pub follow_editor_log: bool,
    pub theme: ThemeChoice,
    /// Show Unity rich text (`<color=cyan>`, `<b>`, `<i>`) styled instead of as tags.
    pub render_rich_text: bool,
    /// Layers of the timeline strip beside the list.
    pub strip: crate::strip::StripLayers,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ide: IdeChoice::Auto,
            custom_command: String::new(),
            project_root: None,
            also_redact: String::new(),
            sessions_dir: None,
            collapse_duplicates: true,
            show_noise: false,
            follow_editor_log: false,
            theme: ThemeChoice::System,
            render_rich_text: true,
            strip: crate::strip::StripLayers::default(),
        }
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("COMTE_SETTINGS") {
            return Some(PathBuf::from(path)); // tests and dev runs
        }
        directories::ProjectDirs::from("", "", "ComteLogger")
            .map(|dirs| dirs.config_dir().join("settings.toml"))
    }

    /// Saved settings, or defaults when there are none or they can't be read.
    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml_and_tolerates_missing_fields() {
        let settings = Settings {
            ide: IdeChoice::Custom,
            custom_command: r#""C:\Tools\subl.exe" {file}:{line}"#.to_owned(),
            project_root: Some(PathBuf::from(r"C:\Projects\Game")),
            also_redact: "jane.doe, Studio X".to_owned(),
            sessions_dir: Some(PathBuf::from(r"D:\Logs")),
            collapse_duplicates: false,
            show_noise: true,
            follow_editor_log: true,
            theme: ThemeChoice::Dark,
            render_rich_text: false,
            strip: crate::strip::StripLayers {
                matches: false,
                ..Default::default()
            },
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);

        // A file from M9 (only the IDE fields) keeps the new defaults.
        let partial: Settings = toml::from_str("ide = \"Rider\"").unwrap();
        assert_eq!(partial.ide, IdeChoice::Rider);
        assert!(partial.collapse_duplicates, "default stays on");
        assert_eq!(partial.theme, ThemeChoice::System);
        assert!(
            partial.render_rich_text,
            "rich text is on unless turned off"
        );
        assert!(toml::from_str::<Settings>("ide = \"Emacs\"").is_err());
    }
}
