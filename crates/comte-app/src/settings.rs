//! Settings kept between runs, in `settings.toml` under the user's config
//! folder (`%APPDATA%\ComteLogger\config` on Windows).
//!
//! M12 adds a settings window; for now the ⚙ menu edits these.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ide::IdeChoice;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where "open in IDE" sends files.
    pub ide: IdeChoice,
    /// Template for [`IdeChoice::Custom`]: `{file}`, `{line}`, `{column}`.
    pub custom_command: String,
    /// Unity project folder used when a log doesn't name one that exists
    /// (Player logs, logs from another machine, redacted sessions).
    pub project_root: Option<PathBuf>,
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
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);

        let partial: Settings = toml::from_str("ide = \"Rider\"").unwrap();
        assert_eq!(partial.ide, IdeChoice::Rider);
        assert_eq!(partial.project_root, None);
        assert!(toml::from_str::<Settings>("ide = \"Emacs\"").is_err());
    }
}
