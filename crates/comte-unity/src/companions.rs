//! Logs and folders that go with a Unity log (DESIGN §1, "Companion logs").
//!
//! Editor.log is where people look first, but the detail is often elsewhere:
//! Package Manager writes `upm.log`, imports run in worker processes with their
//! own `AssetImportWorkerN.log`, Gradle keeps its build output in the project,
//! the licensing client has its own log, and Editor crashes go to a Crashes
//! folder. Only files and folders that exist are returned.

use std::path::{Path, PathBuf};

use comte_core::{Companion, FactKey, LogDocument, LogKind};

/// Most worker logs listed (a project can have one per CPU core).
const MAX_WORKER_LOGS: usize = 8;

/// Machine-wide folders Unity writes to. Overridable for tests.
#[derive(Debug, Clone, Default)]
pub struct CompanionRoots {
    /// Where the global `Editor.log` and `upm.log` are.
    pub editor_dir: Option<PathBuf>,
    /// Where `Unity.Licensing.Client.log` is.
    pub licensing_dir: Option<PathBuf>,
    /// Where Editor crash reports go.
    pub editor_crashes: Option<PathBuf>,
}

impl CompanionRoots {
    pub fn for_this_machine() -> Self {
        let roots = crate::locator::UnityLogRoots::for_this_machine();
        let editor_dir = roots.editor_dir;
        Self {
            // The licensing client logs one level up: `…\Unity\` on Windows,
            // `~/Library/Logs/Unity/` on macOS (same as the Editor there).
            licensing_dir: if cfg!(target_os = "macos") {
                editor_dir.clone()
            } else {
                editor_dir
                    .as_ref()
                    .and_then(|d| d.parent())
                    .map(Path::to_path_buf)
            },
            editor_crashes: Some(
                std::env::temp_dir()
                    .join("Unity")
                    .join("Editor")
                    .join("Crashes"),
            ),
            editor_dir,
        }
    }
}

fn companion(label: &str, about: &str, path: PathBuf, topic: &str) -> Companion {
    Companion {
        label: label.to_owned(),
        about: about.to_owned(),
        is_folder: path.is_dir(),
        path,
        topic: topic.to_owned(),
    }
}

/// Companions of the log at `log`. `doc` gives the project folder.
pub fn find_companions(
    log: &Path,
    kind: LogKind,
    doc: &LogDocument,
    roots: &CompanionRoots,
) -> Vec<Companion> {
    if kind != LogKind::Editor {
        return Vec::new(); // a Player's crash folder comes with the crash hint
    }
    let project = doc.fact(&FactKey::ProjectPath).map(PathBuf::from);
    let log_dir = log.parent().map(Path::to_path_buf);
    let mut found = Vec::new();

    // upm.log sits next to the Editor log: the project's Logs folder, or the
    // global Editor folder.
    let global = roots.editor_dir.as_ref().map(|d| d.join("upm.log"));
    let local = log_dir.as_ref().map(|d| d.join("upm.log"));
    if let Some(path) = local.clone().filter(|p| p.is_file()) {
        found.push(companion(
            "upm.log",
            "Package Manager's own log",
            path,
            "upm",
        ));
    }
    if let Some(path) = global.filter(|p| p.is_file() && Some(p) != local.as_ref()) {
        // With a project-specific Editor.log, the global one is shared by all
        // projects; say so.
        let label = if found.is_empty() {
            "upm.log"
        } else {
            "upm.log (global)"
        };
        found.push(companion(label, "Package Manager's own log", path, "upm"));
    }

    // Import workers write to the project's Logs folder.
    let mut worker_dirs = vec![project.as_ref().map(|p| p.join("Logs")), log_dir];
    worker_dirs.dedup();
    let mut workers: Vec<PathBuf> = worker_dirs
        .into_iter()
        .flatten()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("AssetImportWorker") && n.ends_with(".log"))
        })
        .collect();
    workers.sort();
    workers.dedup();
    for path in workers.into_iter().take(MAX_WORKER_LOGS) {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        found.push(companion(&name, "an import worker's log", path, "import"));
    }

    if let Some(project) = &project {
        // Gradle's output: `Temp/gradleOut` (older Unity) or Bee's Android
        // project (2021.2+).
        for rel in [
            "Temp/gradleOut",
            "Library/Bee/Android/Prj/IL2CPP/Gradle",
            "Library/Bee/Android/Prj/Mono2x/Gradle",
        ] {
            let path = project.join(rel);
            if path.is_dir() {
                found.push(companion(
                    "Gradle build folder",
                    "the Android build's Gradle project and output",
                    path,
                    "gradle",
                ));
                break;
            }
        }
    }

    if let Some(path) = roots
        .licensing_dir
        .as_ref()
        .map(|d| d.join("Unity.Licensing.Client.log"))
        .filter(|p| p.is_file())
    {
        found.push(companion(
            "Unity.Licensing.Client.log",
            "the licensing client's log",
            path,
            "licensing",
        ));
    }

    if let Some(path) = roots.editor_crashes.clone().filter(|p| p.is_dir()) {
        found.push(companion(
            "Editor crash reports",
            "a folder per Editor crash, with a .dmp file",
            path,
            "crash",
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use comte_core::{Fact, ParseEvent};

    use super::*;

    #[test]
    fn finds_what_exists_next_to_the_log_and_in_the_project() {
        let base = std::env::temp_dir().join(format!("comte-companions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("Game");
        let logs = project.join("Logs");
        let global = base.join("Unity").join("Editor");
        for dir in [
            &logs,
            &global,
            &project.join("Library/Bee/Android/Prj/IL2CPP/Gradle"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for file in [
            logs.join("Editor.log"),
            logs.join("upm.log"),
            logs.join("AssetImportWorker1.log"),
            logs.join("AssetImportWorker0.log"),
            logs.join("shadercompiler-UnityShaderCompiler.exe0.log"),
            global.join("upm.log"),
            base.join("Unity").join("Unity.Licensing.Client.log"),
        ] {
            std::fs::write(file, "x").unwrap();
        }
        let mut doc = LogDocument::new();
        doc.apply(ParseEvent::Fact(Fact::new(
            FactKey::ProjectPath,
            project.to_string_lossy(),
            1,
        )));
        let roots = CompanionRoots {
            editor_dir: Some(global.clone()),
            licensing_dir: Some(base.join("Unity")),
            editor_crashes: Some(base.join("missing")),
        };

        let found = find_companions(&logs.join("Editor.log"), LogKind::Editor, &doc, &roots);
        let labels: Vec<(&str, &str)> = found
            .iter()
            .map(|c| (c.label.as_str(), c.topic.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                ("upm.log", "upm"),
                ("upm.log (global)", "upm"),
                ("AssetImportWorker0.log", "import"),
                ("AssetImportWorker1.log", "import"),
                ("Gradle build folder", "gradle"),
                ("Unity.Licensing.Client.log", "licensing"),
            ],
            "the project's and the global upm.log; no crash folder (missing)"
        );
        assert!(found[4].is_folder);
        assert!(
            find_companions(&logs.join("Editor.log"), LogKind::Player, &doc, &roots).is_empty()
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
