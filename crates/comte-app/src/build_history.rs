//! Build history (DESIGN §5.2): builds saved the way sessions are saved, as
//! redacted `.log` files holding only that build's lines, in a `builds`
//! folder next to `sessions`. Reopened, they parse back into the same build,
//! so any two can be compared.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use comte_core::{BuildReport, FactKey, LogDocument};

use crate::loader::{Loaded, load};
use crate::save::{ContentChoice, SaveRequest, prepare, write};
use crate::view::Filter;

/// A build saved to the history.
#[derive(Debug, Clone)]
pub struct SavedBuild {
    pub path: PathBuf,
    /// When it was saved ("2026-11-24 23:40").
    pub saved_at: String,
    pub modified: Option<SystemTime>,
    /// The project's folder name, when the log names the project.
    pub project: Option<String>,
    pub note: String,
    pub build: BuildReport,
}

impl SavedBuild {
    /// "Sample Game · Android build · … · saved 2026-11-24 23:40"
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(project) = &self.project {
            parts.push(project.clone());
        }
        parts.push(self.build.label());
        parts.push(format!("saved {}", self.saved_at));
        parts.join(" · ")
    }
}

/// `…\ComteLogAnalyzer\builds`, beside the sessions folder.
///
/// Dev hook: `COMTE_BUILDS_DIR` points it elsewhere.
pub fn builds_dir(sessions_dir: &Path) -> PathBuf {
    std::env::var_os("COMTE_BUILDS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| sessions_dir.with_file_name("builds"))
}

/// The project's folder name: `C:/Users/<user>/Projects/Sample Game` →
/// `Sample Game`.
pub fn project_name(doc: &LogDocument) -> Option<String> {
    let path = doc.fact(&FactKey::ProjectPath)?;
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// `2026-11-24_23-40_Sample-Game_Android.log`
pub fn file_name(
    project: Option<&str>,
    build: &BuildReport,
    now: &chrono::DateTime<chrono::Local>,
) -> String {
    let title = format!(
        "{}_{}",
        project.unwrap_or("build"),
        build.target.as_deref().unwrap_or("build")
    );
    crate::save::default_file_name(&title, now)
}

/// Saves build `index` of the open log to `dir`, redacted, and returns the
/// new file. The build's identity goes into the tags, so saving the same
/// build again can be noticed.
pub fn save_build(
    loaded: &Loaded,
    index: usize,
    dir: &Path,
    note: &str,
    user_names: &[&str],
    now: &chrono::DateTime<chrono::Local>,
) -> io::Result<PathBuf> {
    let build = loaded
        .doc
        .build_reports()
        .get(index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such build"))?;
    let project = project_name(&loaded.doc);
    let mut tags = vec!["build".to_owned()];
    if let Some(target) = &build.target {
        tags.push(target.to_lowercase());
    }
    let request = SaveRequest {
        note: note.to_owned(),
        tags: tags.join(", "),
        content: ContentChoice::Build(index),
        redact: true,
        pins: Vec::new(),
    };
    let prepared = prepare(
        loaded,
        &Filter::default(),
        &request,
        now.format("%Y-%m-%d %H:%M").to_string(),
        user_names,
    );
    let mut path = dir.join(file_name(project.as_deref(), build, now));
    let mut n = 2;
    while path.exists() {
        path = dir
            .join(file_name(project.as_deref(), build, now).replace(".log", &format!("-{n}.log")));
        n += 1;
    }
    write(&path, &prepared)?;
    Ok(path)
}

/// Reads one saved build.
pub fn read_saved(path: &Path) -> Option<SavedBuild> {
    let title = path.file_name()?.to_string_lossy().into_owned();
    let loaded = load(path, &title).ok()?;
    let header = loaded.session.as_ref()?;
    let build = loaded.doc.build_reports().last()?.clone();
    Some(SavedBuild {
        path: path.to_path_buf(),
        saved_at: header.saved_at.clone(),
        modified: fs::metadata(path).and_then(|m| m.modified()).ok(),
        project: project_name(&loaded.doc),
        note: header.note.clone(),
        build,
    })
}

/// Saved builds in `dir`, newest first.
pub fn discover(dir: &Path) -> Vec<SavedBuild> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut builds: Vec<SavedBuild> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        })
        .filter_map(|p| read_saved(&p))
        .collect();
    builds.sort_by(|a, b| {
        b.saved_at
            .cmp(&a.saved_at)
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.path.cmp(&b.path))
    });
    builds
}

/// The saved copy of `build`, if it was saved before.
pub fn find_saved<'a>(history: &'a [SavedBuild], build: &BuildReport) -> Option<&'a SavedBuild> {
    let identity = build.identity();
    history.iter().find(|s| s.build.identity() == identity)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn fixture() -> Loaded {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../comte-unity/tests/fixtures/editor_build.log");
        load(&path, "Editor.log").unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("comte-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_saved_build_reads_back_as_the_same_build() {
        let loaded = fixture();
        let dir = temp_dir("build-history");
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 24, 23, 40, 0)
            .unwrap();
        let path = save_build(&loaded, 1, &dir, "release 1.4", &[], &now).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "2026-11-24_23-40_Sample-Game_Android.log"
        );
        let again = save_build(&loaded, 0, &dir, "", &[], &now).unwrap();
        assert!(again.to_string_lossy().ends_with("_Android-2.log"));

        let history = discover(&dir);
        assert_eq!(history.len(), 2);
        let saved = history
            .iter()
            .find(|s| s.path == path)
            .expect("the clean build");
        let original = &loaded.doc.build_reports()[1];
        assert_eq!(saved.build.identity(), original.identity());
        assert_eq!(saved.build.assets, original.assets);
        assert_eq!(saved.build.categories, original.categories);
        assert_eq!(saved.build.steps, original.steps);
        assert_eq!(saved.project.as_deref(), Some("Sample Game"));
        assert_eq!(saved.note, "release 1.4");
        assert!(find_saved(&history, original).is_some());

        // Only the build was saved, not the whole log.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.lines().count() < loaded.raw.len());
        assert!(!text.contains("UnityConnectWebRequestException"));
        assert!(
            text.contains("# content:       filtered (build: Android build"),
            "{text}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_builds_are_redacted() {
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../comte-unity/tests/fixtures/editor_build.log"),
        )
        .unwrap()
        .replace(
            "Assets/Game/Textures/item_0001.png",
            "C:/Users/jdoe.smith/Art/item_0001.png",
        );
        let dir = temp_dir("build-redact");
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("Editor.log");
        fs::write(&log, text).unwrap();
        let loaded = load(&log, "Editor.log").unwrap();
        let out = dir.join("builds");
        let now = chrono::Local::now();
        let path = save_build(&loaded, 1, &out, "", &["dev"], &now).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("jdoe"), "{saved}");
        assert!(saved.contains("C:/Users/<user>/Art/item_0001.png"));
        assert!(
            !saved.contains("Users/dev/"),
            "the project path is redacted too"
        );
        let back = read_saved(&path).unwrap();
        assert_eq!(back.build.assets.len(), 50);
        assert_eq!(back.project.as_deref(), Some("Sample Game"));
        let _ = fs::remove_dir_all(&dir);
    }
}
